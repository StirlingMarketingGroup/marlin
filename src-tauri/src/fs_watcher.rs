use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

#[cfg(target_os = "macos")]
use crate::macos_security;

#[derive(Debug)]
pub struct FsWatcher {
    watchers: Arc<Mutex<HashMap<String, RecommendedWatcher>>>,
    debounce_map: Arc<Mutex<HashMap<String, Instant>>>,
    app_handle: AppHandle,
    #[cfg(target_os = "macos")]
    scope_tokens: Arc<Mutex<HashMap<String, macos_security::AccessToken>>>,
}

impl FsWatcher {
    pub fn new(app_handle: AppHandle) -> Self {
        Self {
            watchers: Arc::new(Mutex::new(HashMap::new())),
            debounce_map: Arc::new(Mutex::new(HashMap::new())),
            app_handle,
            #[cfg(target_os = "macos")]
            scope_tokens: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn start_watching(&self, path: &str) -> Result<(), String> {
        let path_buf = PathBuf::from(path);

        if !path_buf.exists() {
            return Err("Path does not exist".to_string());
        }

        if !path_buf.is_dir() {
            return Err("Path is not a directory".to_string());
        }

        let normalized_path = path_buf.to_string_lossy().to_string();

        #[cfg(target_os = "macos")]
        let scope_token = macos_security::retain_access(&path_buf)?;

        // Check if already watching this path
        {
            let watchers = self.watchers.lock().unwrap();
            if watchers.contains_key(&normalized_path) {
                #[cfg(target_os = "macos")]
                drop(scope_token);
                return Ok(()); // Already watching
            }
        }

        let (tx, rx) = mpsc::channel();
        let app_handle = self.app_handle.clone();
        let _debounce_map = self.debounce_map.clone(); // Keep for potential future use
        let watch_path = normalized_path.clone();

        // Create watcher with custom configuration
        let config = Config::default()
            .with_poll_interval(Duration::from_millis(500))
            .with_compare_contents(false);

        let mut watcher = RecommendedWatcher::new(
            move |result: Result<Event, notify::Error>| {
                if let Ok(event) = result {
                    let _ = tx.send(event);
                }
            },
            config,
        )
        .map_err(|e| format!("Failed to create watcher: {}", e))?;

        // Start watching the directory (non-recursive for performance)
        watcher
            .watch(&path_buf, RecursiveMode::NonRecursive)
            .map_err(|e| format!("Failed to start watching: {}", e))?;

        // Store the watcher
        {
            let mut watchers = self.watchers.lock().unwrap();
            watchers.insert(normalized_path.clone(), watcher);
        }

        #[cfg(target_os = "macos")]
        if let Some(token) = scope_token {
            let mut tokens = self.scope_tokens.lock().unwrap();
            tokens.insert(normalized_path.clone(), token);
        }

        // Use a thread to handle events since we're not in a Tokio context yet
        // This implementation batches events within the debounce window instead of dropping them
        std::thread::spawn(move || {
            use std::collections::HashSet;

            const DEBOUNCE_DURATION: Duration = Duration::from_millis(300);

            // Pending batch of events to emit
            let mut pending_files: HashSet<String> = HashSet::new();
            let mut pending_paths: HashSet<String> = HashSet::new();
            let mut has_creates = false;
            let mut has_modifies = false;
            let mut has_removes = false;
            let mut batch_start: Option<Instant> = None;

            loop {
                // Calculate timeout for recv - either wait for debounce or indefinitely
                let recv_result = if let Some(start) = batch_start {
                    let elapsed = start.elapsed();
                    if elapsed >= DEBOUNCE_DURATION {
                        // Debounce expired, process batch immediately
                        Err(mpsc::RecvTimeoutError::Timeout)
                    } else {
                        // Wait for next event or remaining debounce time
                        rx.recv_timeout(DEBOUNCE_DURATION - elapsed)
                    }
                } else {
                    // No pending batch, wait indefinitely for next event
                    rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                };

                match recv_result {
                    Ok(event) => {
                        // Filter for relevant event types
                        let is_relevant = match event.kind {
                            EventKind::Create(_) => {
                                has_creates = true;
                                true
                            }
                            EventKind::Modify(_) => {
                                has_modifies = true;
                                true
                            }
                            EventKind::Remove(_) => {
                                has_removes = true;
                                true
                            }
                            _ => false,
                        };

                        if is_relevant {
                            // Add paths to pending batch
                            for path in &event.paths {
                                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                                    pending_files.insert(name.to_string());
                                }
                                if let Some(full_path) = path.to_str() {
                                    pending_paths.insert(full_path.to_string());
                                }
                            }

                            // Start debounce timer if not already started
                            if batch_start.is_none() {
                                batch_start = Some(Instant::now());
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // Debounce expired, emit the batch if we have one
                        // Use pending_paths as trigger (pending_files may be empty for non-UTF8 filenames)
                        if !pending_paths.is_empty() {
                            // Determine change type (prioritize removes > modifies > creates)
                            let change_type = if has_removes {
                                "removed"
                            } else if has_modifies {
                                "modified"
                            } else if has_creates {
                                "created"
                            } else {
                                "changed"
                            };

                            let affected_files: Vec<String> = pending_files.drain().collect();
                            let affected_paths: Vec<String> = pending_paths.drain().collect();

                            // Invalidate thumbnail cache for modified/removed files
                            if matches!(change_type, "modified" | "removed")
                                && !affected_paths.is_empty()
                            {
                                let paths_for_invalidation = affected_paths.clone();
                                tauri::async_runtime::spawn(async move {
                                    if let Ok(service) =
                                        crate::commands::get_thumbnail_service().await
                                    {
                                        service.invalidate_paths(&paths_for_invalidation).await;
                                        log::debug!(
                                            "Invalidated thumbnail cache for {} paths",
                                            paths_for_invalidation.len()
                                        );
                                    }
                                });
                            }

                            // Emit batched event to frontend
                            let payload = serde_json::json!({
                                "path": watch_path,
                                "changeType": change_type,
                                "affectedFiles": affected_files,
                                "affectedPaths": affected_paths
                            });

                            if let Err(e) = app_handle.emit("directory-changed", payload) {
                                eprintln!("Failed to emit directory-changed event: {}", e);
                            }

                            // Reset batch state
                            has_creates = false;
                            has_modifies = false;
                            has_removes = false;
                            batch_start = None;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        // Channel closed, exit the loop
                        break;
                    }
                }
            }
        });

        #[cfg(target_os = "macos")]
        macos_security::persist_bookmark(&path_buf, "starting watcher");

        Ok(())
    }

    pub fn stop_watching(&self, path: &str) -> Result<(), String> {
        let normalized_path = PathBuf::from(path).to_string_lossy().to_string();

        let mut watchers = self.watchers.lock().unwrap();
        if watchers.remove(&normalized_path).is_some() {
            // Also clean up debounce entry
            let mut debounce = self.debounce_map.lock().unwrap();
            debounce.remove(&normalized_path);

            #[cfg(target_os = "macos")]
            {
                let mut tokens = self.scope_tokens.lock().unwrap();
                tokens.remove(&normalized_path);
            }
            Ok(())
        } else {
            Err("Path is not being watched".to_string())
        }
    }

    pub fn stop_all_watchers(&self) {
        let mut watchers = self.watchers.lock().unwrap();
        watchers.clear();

        let mut debounce = self.debounce_map.lock().unwrap();
        debounce.clear();

        #[cfg(target_os = "macos")]
        {
            let mut tokens = self.scope_tokens.lock().unwrap();
            tokens.clear();
        }
    }

    pub fn is_watching(&self, path: &str) -> bool {
        let normalized_path = PathBuf::from(path).to_string_lossy().to_string();
        let watchers = self.watchers.lock().unwrap();
        watchers.contains_key(&normalized_path)
    }

    pub fn get_watched_paths(&self) -> Vec<String> {
        let watchers = self.watchers.lock().unwrap();
        watchers.keys().cloned().collect()
    }
}

// Global watcher instance - will be initialized in main.rs
static GLOBAL_WATCHER: OnceLock<Arc<FsWatcher>> = OnceLock::new();

pub fn init_watcher(app_handle: AppHandle) {
    GLOBAL_WATCHER
        .set(Arc::new(FsWatcher::new(app_handle)))
        .expect("Watcher already initialized");
}

pub fn get_watcher() -> Option<Arc<FsWatcher>> {
    GLOBAL_WATCHER.get().cloned()
}
