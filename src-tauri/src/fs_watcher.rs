use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

#[cfg(target_os = "macos")]
use crate::macos_security;
#[cfg(target_os = "macos")]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(target_os = "macos")]
use std::os::unix::fs::OpenOptionsExt;

/// What the batch thread hears about one watched folder.
#[derive(Debug, PartialEq)]
enum FolderSignal {
    /// A file event from `notify`.
    FileEvent(Event),
    /// The kernel reports an entry added to, removed from, or renamed in the folder.
    #[cfg(target_os = "macos")]
    EntriesChanged,
}

#[derive(Debug)]
pub struct FsWatcher {
    watchers: Arc<Mutex<HashMap<String, WatchRegistration>>>,
    app_handle: AppHandle,
    #[cfg(target_os = "macos")]
    scope_tokens: Arc<Mutex<HashMap<String, macos_security::AccessToken>>>,
}

#[derive(Debug)]
struct WatchRegistration {
    _watcher: RecommendedWatcher,
    #[cfg(target_os = "macos")]
    _entries_watch: Option<FolderEntriesWatch>,
    subscribers: usize,
}

impl FsWatcher {
    pub fn new(app_handle: AppHandle) -> Self {
        Self {
            watchers: Arc::new(Mutex::new(HashMap::new())),
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

        // React effects can briefly overlap during navigation or Strict Mode. Keep a
        // reference count so an older cleanup cannot tear down a newer registration.
        {
            let mut watchers = self.watchers.lock().unwrap();
            if let Some(registration) = watchers.get_mut(&normalized_path) {
                registration.subscribers += 1;
                #[cfg(target_os = "macos")]
                drop(scope_token);
                return Ok(());
            }
        }

        let (tx, rx) = mpsc::channel();
        #[cfg(target_os = "macos")]
        let entries_tx = tx.clone();
        let app_handle = self.app_handle.clone();
        let watch_path = normalized_path.clone();

        // Create watcher with custom configuration
        let config = Config::default()
            .with_poll_interval(Duration::from_millis(500))
            .with_compare_contents(false);

        let mut watcher = RecommendedWatcher::new(
            move |result: Result<Event, notify::Error>| {
                if let Ok(event) = result {
                    let _ = tx.send(FolderSignal::FileEvent(event));
                }
            },
            config,
        )
        .map_err(|e| format!("Failed to create watcher: {}", e))?;

        // Start watching the directory (non-recursive for performance)
        watcher
            .watch(&path_buf, RecursiveMode::NonRecursive)
            .map_err(|e| format!("Failed to start watching: {}", e))?;

        // FSEvents alone still works, so a failure here does not fail the watch.
        #[cfg(target_os = "macos")]
        let entries_watch = FolderEntriesWatch::start(&path_buf, entries_tx)
            .inspect_err(|err| log::warn!("Failed to watch folder entries: {err}"))
            .ok();

        // Store the watcher. Check again under the insertion lock because another
        // start may have completed while this watcher was being constructed.
        let joined_existing_registration = {
            let mut watchers = self.watchers.lock().unwrap();
            if let Some(registration) = watchers.get_mut(&normalized_path) {
                registration.subscribers += 1;
                true
            } else {
                watchers.insert(
                    normalized_path.clone(),
                    WatchRegistration {
                        _watcher: watcher,
                        #[cfg(target_os = "macos")]
                        _entries_watch: entries_watch,
                        subscribers: 1,
                    },
                );
                false
            }
        };

        if joined_existing_registration {
            #[cfg(target_os = "macos")]
            drop(scope_token);
            return Ok(());
        }

        #[cfg(target_os = "macos")]
        if let Some(token) = scope_token {
            let mut tokens = self.scope_tokens.lock().unwrap();
            tokens.insert(normalized_path.clone(), token);
        }

        // One thread per watched folder collects its signals for 300 ms and emits one
        // `directory-changed`.
        std::thread::spawn(move || {
            use std::collections::HashSet;

            const DEBOUNCE_DURATION: Duration = Duration::from_millis(300);

            #[derive(Default)]
            struct PendingBatch {
                files: HashSet<String>,
                paths: HashSet<String>,
                has_creates: bool,
                has_modifies: bool,
                has_removes: bool,
            }

            let mut pending = PendingBatch::default();
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
                    Ok(FolderSignal::FileEvent(event)) => {
                        // Filter for relevant event types
                        let is_relevant = match event.kind {
                            EventKind::Create(_) => {
                                pending.has_creates = true;
                                true
                            }
                            EventKind::Modify(_) => {
                                pending.has_modifies = true;
                                true
                            }
                            EventKind::Remove(_) => {
                                pending.has_removes = true;
                                true
                            }
                            _ => false,
                        };

                        if is_relevant {
                            // Add paths to pending batch
                            for path in &event.paths {
                                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                                    pending.files.insert(name.to_string());
                                }
                                if let Some(full_path) = path.to_str() {
                                    pending.paths.insert(full_path.to_string());
                                }
                            }

                            // Start debounce timer if not already started
                            if batch_start.is_none() {
                                batch_start = Some(Instant::now());
                            }
                        }
                    }
                    #[cfg(target_os = "macos")]
                    Ok(FolderSignal::EntriesChanged) => {
                        if batch_start.is_none() {
                            batch_start = Some(Instant::now());
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // Debounce expired. Emit whatever this batch collected, including a
                        // batch that only knows the folder's entries changed.
                        let batch = std::mem::take(&mut pending);
                        batch_start = None;

                        // Determine change type (prioritize removes > modifies > creates)
                        let change_type = if batch.has_removes {
                            "removed"
                        } else if batch.has_modifies {
                            "modified"
                        } else if batch.has_creates {
                            "created"
                        } else {
                            "changed"
                        };

                        let affected_files: Vec<String> = batch.files.into_iter().collect();
                        let affected_paths: Vec<String> = batch.paths.into_iter().collect();

                        // Invalidate thumbnail cache for modified/removed files
                        if matches!(change_type, "modified" | "removed")
                            && !affected_paths.is_empty()
                        {
                            let paths_for_invalidation = affected_paths.clone();
                            tauri::async_runtime::spawn(async move {
                                if let Ok(service) = crate::commands::get_thumbnail_service().await
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
        let should_remove = match watchers.get_mut(&normalized_path) {
            Some(registration) if registration.subscribers > 1 => {
                registration.subscribers -= 1;
                false
            }
            Some(_) => true,
            None => return Err("Path is not being watched".to_string()),
        };

        if should_remove {
            watchers.remove(&normalized_path);
            #[cfg(target_os = "macos")]
            {
                let mut tokens = self.scope_tokens.lock().unwrap();
                tokens.remove(&normalized_path);
            }
        }

        Ok(())
    }

    pub fn stop_all_watchers(&self) {
        let mut watchers = self.watchers.lock().unwrap();
        watchers.clear();

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

// Global watcher instance. The setup hook in lib.rs sets it once with init_watcher.
static GLOBAL_WATCHER: OnceLock<Arc<FsWatcher>> = OnceLock::new();

pub fn init_watcher(app_handle: AppHandle) {
    GLOBAL_WATCHER
        .set(Arc::new(FsWatcher::new(app_handle)))
        .expect("Watcher already initialized");
}

pub fn get_watcher() -> Option<Arc<FsWatcher>> {
    GLOBAL_WATCHER.get().cloned()
}

/// A kernel watch on the entries of one folder. macOS can deliver an FSEvents
/// event seconds to minutes late when `fseventsd` is busy (#577). A kqueue
/// event comes straight from the kernel.
#[cfg(target_os = "macos")]
#[derive(Debug)]
struct FolderEntriesWatch {
    kq: Arc<OwnedFd>,
}

#[cfg(target_os = "macos")]
impl FolderEntriesWatch {
    fn start(folder: &std::path::Path, tx: mpsc::Sender<FolderSignal>) -> std::io::Result<Self> {
        // O_EVTONLY: a plain open would stop the user from ejecting the volume.
        let folder = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_EVTONLY)
            .open(folder)?;

        let kq = unsafe { libc::kqueue() };
        if kq == -1 {
            return Err(std::io::Error::last_os_error());
        }
        let kq = Arc::new(unsafe { OwnedFd::from_raw_fd(kq) });

        let changes = [
            libc::kevent {
                ident: folder.as_raw_fd() as libc::uintptr_t,
                filter: libc::EVFILT_VNODE,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: libc::NOTE_WRITE,
                data: 0,
                udata: std::ptr::null_mut(),
            },
            // `drop` triggers this one to wake the thread.
            libc::kevent {
                ident: 0,
                filter: libc::EVFILT_USER,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: 0,
                data: 0,
                udata: std::ptr::null_mut(),
            },
        ];
        let registered = unsafe {
            libc::kevent(
                kq.as_raw_fd(),
                changes.as_ptr(),
                changes.len() as libc::c_int,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if registered == -1 {
            return Err(std::io::Error::last_os_error());
        }

        let thread_kq = Arc::clone(&kq);
        std::thread::spawn(move || {
            // The watch ends when the folder closes, so hold it until the thread exits.
            let _folder = folder;
            loop {
                let mut event: libc::kevent = unsafe { std::mem::zeroed() };
                let got = unsafe {
                    libc::kevent(
                        thread_kq.as_raw_fd(),
                        std::ptr::null(),
                        0,
                        &mut event,
                        1,
                        std::ptr::null(),
                    )
                };
                if got == -1 {
                    let err = std::io::Error::last_os_error();
                    if err.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    log::warn!("Folder entries watch failed: {err}");
                    break;
                }
                if event.filter == libc::EVFILT_USER {
                    break;
                }
                if tx.send(FolderSignal::EntriesChanged).is_err() {
                    break;
                }
            }
        });

        Ok(Self { kq })
    }
}

#[cfg(target_os = "macos")]
impl Drop for FolderEntriesWatch {
    fn drop(&mut self) {
        let trigger = libc::kevent {
            ident: 0,
            filter: libc::EVFILT_USER,
            flags: 0,
            fflags: libc::NOTE_TRIGGER,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        unsafe {
            libc::kevent(
                self.kq.as_raw_fd(),
                &trigger,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            );
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{FolderEntriesWatch, FolderSignal};
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn folder_entries_watch_reports_entry_changes_and_stops_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let watch = FolderEntriesWatch::start(dir.path(), tx).unwrap();

        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"hi").unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(FolderSignal::EntriesChanged)
        );
        std::thread::sleep(Duration::from_millis(100));
        while rx.try_recv().is_ok() {}

        let renamed = dir.path().join("b.txt");
        std::fs::rename(&file, &renamed).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(FolderSignal::EntriesChanged)
        );
        std::thread::sleep(Duration::from_millis(100));
        while rx.try_recv().is_ok() {}

        std::fs::remove_file(&renamed).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(FolderSignal::EntriesChanged)
        );
        std::thread::sleep(Duration::from_millis(100));
        while rx.try_recv().is_ok() {}

        drop(watch);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }
}
