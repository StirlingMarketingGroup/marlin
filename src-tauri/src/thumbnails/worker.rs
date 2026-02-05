use rayon::ThreadPool;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio::task::JoinHandle;

use super::generators::ThumbnailGenerator;
use super::{cache::ThumbnailCache, ThumbnailRequest, ThumbnailResponse};

#[derive(Debug)]
struct PriorityRequest {
    request: ThumbnailRequest,
    response_sender: oneshot::Sender<Result<ThumbnailResponse, String>>,
    timestamp: Instant,
}

impl Eq for PriorityRequest {}

impl PartialEq for PriorityRequest {
    fn eq(&self, other: &Self) -> bool {
        self.request.priority == other.request.priority && self.timestamp == other.timestamp
    }
}

impl Ord for PriorityRequest {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher priority first, then older requests first
        match self.request.priority.cmp(&other.request.priority) {
            Ordering::Equal => other.timestamp.cmp(&self.timestamp),
            other_ord => other_ord.reverse(), // Reverse to make High > Medium > Low
        }
    }
}

impl PartialOrd for PriorityRequest {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

enum WorkerMessage {
    Request(PriorityRequest),
    Cancel(String),
    CancelAll,
}

pub struct ThumbnailWorker {
    sender: mpsc::UnboundedSender<WorkerMessage>,
    worker_handles: Vec<JoinHandle<()>>,
}

impl ThumbnailWorker {
    pub async fn new(cache: Arc<ThumbnailCache>) -> Result<Self, String> {
        let (sender, receiver) = mpsc::unbounded_channel();

        // Create thread pool for CPU-intensive work
        let thread_pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(num_cpus::get().max(4))
                .thread_name(|i| format!("thumbnail-{}", i))
                .build()
                .map_err(|e| format!("Failed to create thread pool: {}", e))?,
        );

        // Allow multiple in-flight requests (bounded)
        // Allow up to the number of logical CPUs in flight
        let max_in_flight = std::cmp::max(1, num_cpus::get());
        let semaphore = Arc::new(Semaphore::new(max_in_flight));

        let mut worker = ThumbnailWorker {
            sender,
            worker_handles: Vec::new(),
        };

        // Start single worker task that handles all requests
        let handle = worker
            .spawn_worker_task(0, receiver, cache.clone(), thread_pool, semaphore)
            .await;
        worker.worker_handles.push(handle);

        Ok(worker)
    }

    async fn spawn_worker_task(
        &self,
        worker_id: usize,
        mut receiver: mpsc::UnboundedReceiver<WorkerMessage>,
        cache: Arc<ThumbnailCache>,
        thread_pool: Arc<ThreadPool>,
        semaphore: Arc<Semaphore>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut request_queue = BinaryHeap::new();
            // Track spawned tasks so we can abort them on cancellation
            let mut in_flight: HashMap<String, JoinHandle<()>> = HashMap::new();

            log::info!("Thumbnail worker {} started", worker_id);

            loop {
                // Process incoming messages
                while let Ok(message) = receiver.try_recv() {
                    match message {
                        WorkerMessage::Request(priority_request) => {
                            request_queue.push(priority_request);
                        }
                        WorkerMessage::Cancel(request_id) => {
                            // Remove from queue
                            let mut new_queue = BinaryHeap::new();
                            while let Some(req) = request_queue.pop() {
                                if req.request.id != request_id {
                                    new_queue.push(req);
                                }
                            }
                            request_queue = new_queue;

                            // Abort spawned task if still running
                            if let Some(handle) = in_flight.remove(&request_id) {
                                handle.abort();
                            }
                        }
                        WorkerMessage::CancelAll => {
                            // Clear the entire queue
                            request_queue.clear();

                            // Abort all in-flight tasks
                            for (_, handle) in in_flight.drain() {
                                handle.abort();
                            }
                        }
                    }
                }

                // Prune finished tasks from in_flight map
                in_flight.retain(|_, handle| !handle.is_finished());

                // Launch as many tasks as we have permits and queued work
                loop {
                    // Try to acquire a permit; if none available, stop launching
                    let permit = match semaphore.clone().try_acquire_owned() {
                        Ok(p) => p,
                        Err(_) => break,
                    };

                    // Pop highest priority; if none, release permit and break
                    let Some(priority_request) = request_queue.pop() else {
                        drop(permit);
                        break;
                    };

                    let request = priority_request.request;
                    let request_id = request.id.clone();
                    let response_sender = priority_request.response_sender;
                    let cache_clone = cache.clone();
                    let thread_pool_clone = thread_pool.clone();

                    // Spawn an async task to process this request; permit is dropped on completion
                    let handle = tokio::spawn(async move {
                        let start_time = Instant::now();

                        // Phase 1: Async pre-processing (downloads remote files without blocking threads)
                        let prepared = match ThumbnailGenerator::prepare(request.clone()).await {
                            Ok(p) => p,
                            Err(e) => {
                                log::warn!("THUMBNAIL PREPARE FAILED: path={}, error={}", request.path, e);
                                let _ = response_sender.send(Err(format!("Thumbnail preparation failed: {}", e)));
                                drop(permit);
                                return;
                            }
                        };

                        let temp_file = prepared.temp_file.clone();
                        let req_for_pool = prepared.request;

                        // Phase 2: CPU-bound generation in blocking thread pool (fast, no network I/O)
                        let result = tokio::task::spawn_blocking(move || {
                            thread_pool_clone
                                .install(|| ThumbnailGenerator::generate(&req_for_pool))
                        })
                        .await;

                        // Clean up any temp files from remote downloads
                        if let Some(temp) = temp_file {
                            let _ = std::fs::remove_file(&temp);
                        }

                        let response = match result {
                            Ok(Ok(gen_result)) => {
                                let generation_time_ms = start_time.elapsed().as_millis() as u64;
                                log::info!(
                                    "THUMBNAIL GENERATED: path={}, dimensions={:?}x{:?}",
                                    request.path,
                                    gen_result.image_width,
                                    gen_result.image_height
                                );
                                // Store in cache (including dimensions)
                                if let Err(e) = cache_clone
                                    .put(
                                        &request.path,
                                        request.size,
                                        request.accent.as_ref(),
                                        gen_result.data_url.clone(),
                                        generation_time_ms,
                                        gen_result.has_transparency,
                                        gen_result.image_width,
                                        gen_result.image_height,
                                    )
                                    .await
                                {
                                    log::warn!("Failed to cache thumbnail: {}", e);
                                }
                                Ok(ThumbnailResponse {
                                    id: request.id.clone(),
                                    data_url: gen_result.data_url,
                                    cached: false,
                                    generation_time_ms,
                                    has_transparency: gen_result.has_transparency,
                                    image_width: gen_result.image_width,
                                    image_height: gen_result.image_height,
                                })
                            }
                            Ok(Err(e)) => {
                                log::warn!("THUMBNAIL GENERATION FAILED: path={}, error={}", request.path, e);
                                Err(format!("Thumbnail generation failed: {}", e))
                            }
                            Err(e) => {
                                log::warn!(
                                    "THUMBNAIL TASK FAILED: path={}, error={}",
                                    request.path,
                                    e
                                );
                                Err(format!("Task execution failed: {}", e))
                            }
                        };

                        // Send response back
                        let _ = response_sender.send(response);
                        // Release permit at end of task
                        drop(permit);
                    });

                    in_flight.insert(request_id, handle);
                }

                // Idle briefly to avoid tight-looping when queue is empty or saturated
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            }
        })
    }

    pub async fn submit_request(
        &self,
        request: ThumbnailRequest,
    ) -> Result<ThumbnailResponse, String> {
        let (response_sender, response_receiver) = oneshot::channel();

        let priority_request = PriorityRequest {
            request,
            response_sender,
            timestamp: Instant::now(),
        };

        // Send to worker
        self.sender
            .send(WorkerMessage::Request(priority_request))
            .map_err(|e| format!("Failed to send request to worker: {}", e))?;

        // Wait for response
        response_receiver
            .await
            .map_err(|e| format!("Failed to receive response: {}", e))?
    }

    pub async fn cancel_request(&self, request_id: &str) -> bool {
        // Send cancellation message to workers
        self.sender
            .send(WorkerMessage::Cancel(request_id.to_string()))
            .is_ok()
    }

    pub fn cancel_all(&self) -> bool {
        self.sender.send(WorkerMessage::CancelAll).is_ok()
    }
}
