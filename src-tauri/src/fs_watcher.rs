use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

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

        let (tx, mut rx) = mpsc::unbounded_channel();
        let app_handle = self.app_handle.clone();
        let debounce_map = self.debounce_map.clone();
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
        std::thread::spawn(move || {
            const DEBOUNCE_DURATION: Duration = Duration::from_millis(300);

            // Create a simple receiver loop
            while let Some(event) = rx.blocking_recv() {
                let should_emit = match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => true,
                    _ => false,
                };

                if !should_emit {
                    continue;
                }

                // Debounce rapid changes
                let now = Instant::now();
                let should_process = {
                    let mut debounce = debounce_map.lock().unwrap();
                    if let Some(last_event) = debounce.get(&watch_path) {
                        if now.duration_since(*last_event) < DEBOUNCE_DURATION {
                            false
                        } else {
                            debounce.insert(watch_path.clone(), now);
                            true
                        }
                    } else {
                        debounce.insert(watch_path.clone(), now);
                        true
                    }
                };

                if should_process {
                    // Small additional delay to catch rapid consecutive changes
                    std::thread::sleep(Duration::from_millis(100));

                    // Get change details
                    let change_type = match event.kind {
                        EventKind::Create(_) => "created",
                        EventKind::Modify(_) => "modified",
                        EventKind::Remove(_) => "removed",
                        _ => "changed",
                    };

                    let affected_files: Vec<String> = event
                        .paths
                        .iter()
                        .filter_map(|p| p.file_name())
                        .filter_map(|name| name.to_str())
                        .map(|s| s.to_string())
                        .collect();

                    // Emit event to frontend
                    let payload = serde_json::json!({
                        "path": watch_path,
                        "changeType": change_type,
                        "affectedFiles": affected_files
                    });

                    if let Err(e) = app_handle.emit("directory-changed", payload) {
                        eprintln!("Failed to emit directory-changed event: {}", e);
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
