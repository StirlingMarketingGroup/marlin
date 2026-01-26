#![allow(dead_code)]

#[cfg(target_os = "macos")]
mod imp {
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};

    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    use objc2::class;
    use objc2::msg_send;
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::runtime::{AnyObject, Bool};
    use objc2_foundation::{NSData, NSString, NSURL};
    use dirs;
    use log::warn;
    use serde::{Deserialize, Serialize};

    const NS_URL_BOOKMARK_CREATION_WITH_SECURITY_SCOPE: u64 = 1 << 11;
    const NS_URL_BOOKMARK_RESOLUTION_WITH_SECURITY_SCOPE: u64 = 1 << 10;

    #[derive(Debug, Clone, Serialize, Deserialize, Default)]
    struct BookmarkStore {
        entries: Vec<BookmarkEntry>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct BookmarkEntry {
        path: String,
        data: String,
    }

    impl BookmarkStore {
        fn load() -> Self {
            if let Ok(path) = bookmarks_path() {
                if let Ok(contents) = fs::read_to_string(&path) {
                    if let Ok(store) = serde_json::from_str::<BookmarkStore>(&contents) {
                        return store;
                    }
                }
            }
            BookmarkStore::default()
        }

        fn save(&self) -> Result<(), String> {
            let path = bookmarks_path()?;
            let data = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
            fs::write(path, data).map_err(|e| e.to_string())
        }

        fn best_match(&self, target: &str) -> Option<&BookmarkEntry> {
            self.entries
                .iter()
                .filter(|entry| target.starts_with(&entry.path))
                .max_by_key(|entry| entry.path.len())
        }

        fn contains_parent(&self, target: &str) -> bool {
            self.entries
                .iter()
                .any(|entry| target.starts_with(&entry.path))
        }

        fn upsert(&mut self, path: String, data: Vec<u8>) {
            if let Some(existing) = self.entries.iter_mut().find(|entry| entry.path == path) {
                existing.data = BASE64.encode(data);
            } else {
                self.entries.push(BookmarkEntry {
                    path,
                    data: BASE64.encode(data),
                });
                // Keep entries sorted longest path first so prefix checks hit earlier entries.
                self.entries.sort_by(|a, b| b.path.len().cmp(&a.path.len()));
            }
        }

        fn remove(&mut self, path: &str) -> bool {
            let len_before = self.entries.len();
            self.entries.retain(|entry| entry.path != path);
            self.entries.len() != len_before
        }

        fn decode_data(entry: &BookmarkEntry) -> Option<Vec<u8>> {
            BASE64.decode(&entry.data).ok()
        }
    }

    fn bookmarks_path() -> Result<PathBuf, String> {
        let mut base =
            dirs::config_dir().ok_or_else(|| "Could not resolve config directory".to_string())?;
        base.push("Marlin");
        if !base.exists() {
            fs::create_dir_all(&base).map_err(|e| format!("Failed to create config dir: {}", e))?;
        }
        Ok(base.join("security_bookmarks.json"))
    }

    fn canonical_string(path: &Path) -> String {
        path.canonicalize()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string())
    }

    fn sensitive_root(path: &Path) -> Option<String> {
        // Reduce path to root directory we care about so that bookmarks cover descendants.
        // For example, ~/Downloads/foo -> ~/Downloads
        let mut current = PathBuf::from(path);
        if !current.exists() {
            current.pop();
        }
        while current.parent().is_some() {
            let candidate = canonical_string(&current);
            if SENSITIVE_DIR_SET
                .get_or_init(build_sensitive_dirs)
                .contains(&candidate)
            {
                return Some(candidate);
            }
            current.pop();
        }
        None
    }

    fn build_sensitive_dirs() -> Vec<String> {
        let mut dirs_vec = Vec::new();
        if let Some(home) = dirs::home_dir() {
            let mut push_dir = |suffix: &str| {
                let p = home.join(suffix);
                if p.exists() {
                    dirs_vec.push(canonical_string(&p));
                }
            };
            push_dir("Downloads");
            push_dir("Documents");
            push_dir("Desktop");
            push_dir("Pictures");
            push_dir("Movies");
            push_dir("Music");
            push_dir(".Trash");
            push_dir("iCloud Drive");
        }
        dirs_vec
    }

    static STORE: OnceLock<Mutex<BookmarkStore>> = OnceLock::new();
    static ACTIVE_SCOPES: OnceLock<Mutex<HashMap<String, ActiveScope>>> = OnceLock::new();
    static SENSITIVE_DIR_SET: OnceLock<Vec<String>> = OnceLock::new();

    struct ActiveScope {
        url: Retained<NSURL>,
        count: usize,
    }

    impl ActiveScope {
        fn increment(&mut self) {
            self.count += 1;
        }

        fn decrement(&mut self) -> bool {
            if self.count > 0 {
                self.count -= 1;
            }
            self.count == 0
        }
    }

    unsafe impl Send for ActiveScope {}
    unsafe impl Sync for ActiveScope {}

    fn load_store() -> Mutex<BookmarkStore> {
        Mutex::new(BookmarkStore::load())
    }

    fn store_mutex() -> &'static Mutex<BookmarkStore> {
        STORE.get_or_init(load_store)
    }

    fn active_scopes_mutex() -> &'static Mutex<HashMap<String, ActiveScope>> {
        ACTIVE_SCOPES.get_or_init(|| Mutex::new(HashMap::new()))
    }

    unsafe fn url_for_path(path: &str) -> Result<Retained<NSURL>, String> {
        autoreleasepool(|_| unsafe {
            let ns_path = NSString::from_str(path);
            let url: Option<Retained<NSURL>> =
                msg_send![class!(NSURL), fileURLWithPath: &*ns_path];
            url.ok_or_else(|| "Failed to create NSURL".to_string())
        })
    }

    unsafe fn bookmark_from_url(url: &Retained<NSURL>) -> Result<Vec<u8>, String> {
        autoreleasepool(|_| unsafe {
            let mut error: *mut AnyObject = std::ptr::null_mut();
            let bookmark: Option<Retained<NSData>> = msg_send![
                &*url,
                bookmarkDataWithOptions: NS_URL_BOOKMARK_CREATION_WITH_SECURITY_SCOPE,
                includingResourceValuesForKeys: std::ptr::null_mut::<AnyObject>(),
                relativeToURL: std::ptr::null_mut::<AnyObject>(),
                error: &mut error
            ];
            let bookmark = bookmark.ok_or_else(|| "Failed to create bookmark data".to_string())?;
            let length: usize = msg_send![&*bookmark, length];
            if length == 0 {
                return Err("Bookmark data is empty".to_string());
            }
            let bytes_ptr: *const std::ffi::c_void = msg_send![&*bookmark, bytes];
            if bytes_ptr.is_null() {
                return Err("Bookmark bytes pointer is null".to_string());
            }
            let slice = std::slice::from_raw_parts(bytes_ptr as *const u8, length);
            Ok(slice.to_vec())
        })
    }

    unsafe fn resolve_bookmark(data: &[u8]) -> Result<(Retained<NSURL>, bool), String> {
        autoreleasepool(|_| unsafe {
            if data.is_empty() {
                return Err("Bookmark data is empty".to_string());
            }
            let nsdata: Option<Retained<NSData>> =
                msg_send![class!(NSData), dataWithBytes: data.as_ptr(), length: data.len()];
            let nsdata = nsdata.ok_or_else(|| "Failed to build NSData from bookmark".to_string())?;
            let mut error: *mut AnyObject = std::ptr::null_mut();
            let mut is_stale = Bool::NO;
            let resolved: Option<Retained<NSURL>> = msg_send![
                class!(NSURL),
                URLByResolvingBookmarkData: &*nsdata,
                options: NS_URL_BOOKMARK_RESOLUTION_WITH_SECURITY_SCOPE,
                relativeToURL: std::ptr::null_mut::<AnyObject>(),
                bookmarkDataIsStale: &mut is_stale,
                error: &mut error
            ];
            let resolved = resolved.ok_or_else(|| "Failed to resolve bookmark".to_string())?;
            let started: Bool = msg_send![&*resolved, startAccessingSecurityScopedResource];
            if started.is_false() {
                return Err("startAccessingSecurityScopedResource returned false".to_string());
            }
            Ok((resolved, is_stale.is_true()))
        })
    }

    fn longest_matching_key<'a>(store: &'a BookmarkStore, path: &str) -> Option<&'a BookmarkEntry> {
        store.best_match(path)
    }

    fn discover_scope_key(path: &Path) -> String {
        // Prefer canonical string but fall back to normal path when necessary.
        canonical_string(path)
    }

    #[derive(Debug)]
    pub struct AccessToken {
        key: String,
    }

    impl Drop for AccessToken {
        fn drop(&mut self) {
            let mut active = active_scopes_mutex().lock().unwrap();
            if let Some(entry) = active.get_mut(&self.key) {
                if entry.decrement() {
                    unsafe {
                        let _: () = msg_send![&*entry.url, stopAccessingSecurityScopedResource];
                    }
                    active.remove(&self.key);
                }
            }
        }
    }

    fn forget_invalid_bookmark(store_lock: &Mutex<BookmarkStore>, path: &str, reason: &str) {
        let mut store = match store_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                warn!(
                    "BookmarkStore mutex poisoned while removing invalid bookmark for {} ({}); recovering with prior data. Error: {}",
                    path, reason, poisoned
                );
                poisoned.into_inner()
            }
        };
        if store.remove(path) {
            match store.save() {
                Ok(_) => warn!(
                    "Removed invalid security bookmark for {} after {}",
                    path, reason
                ),
                Err(err) => warn!(
                    "Failed to persist removal of invalid security bookmark for {} after {}: {}",
                    path, reason, err
                ),
            }
        }
    }

    pub fn retain_access(path: &Path) -> Result<Option<AccessToken>, String> {
        retain_access_internal(path, true)
    }

    fn retain_access_internal(
        path: &Path,
        allow_retry: bool,
    ) -> Result<Option<AccessToken>, String> {
        let store_lock = store_mutex();
        let path_key = discover_scope_key(path);
        let maybe_entry = {
            let store = store_lock.lock().unwrap();
            longest_matching_key(&store, &path_key).cloned()
        };

        let entry = match maybe_entry {
            Some(entry) => entry,
            None => return Ok(None),
        };

        let data = match BookmarkStore::decode_data(&entry) {
            Some(data) => data,
            None => {
                warn!(
                    "Failed to decode bookmark data for path '{}'; attempting automatic refresh",
                    entry.path
                );
                forget_invalid_bookmark(&store_lock, &entry.path, "decode failure");
                return recover_revoked_access(path, &entry.path, allow_retry, "decode failure");
            }
        };

        {
            let mut active = active_scopes_mutex().lock().unwrap();
            if let Some(scope) = active.get_mut(&entry.path) {
                scope.increment();
                return Ok(Some(AccessToken {
                    key: entry.path.clone(),
                }));
            }
        }

        let (url, stale) = match unsafe { resolve_bookmark(&data) } {
            Ok(result) => result,
            Err(err) => {
                warn!(
                    "Failed to resolve bookmark for path '{}': {}. Attempting automatic refresh",
                    entry.path, err
                );
                forget_invalid_bookmark(&store_lock, &entry.path, "resolve failure");
                return recover_revoked_access(path, &entry.path, allow_retry, "resolve failure");
            }
        };

        if stale {
            if let Ok(new_data) = unsafe { bookmark_from_url(&url) } {
                let mut store = store_lock.lock().unwrap();
                store.upsert(entry.path.clone(), new_data);
                if let Err(err) = store.save() {
                    warn!(
                        "Failed to persist refreshed security bookmark for {}: {}",
                        entry.path, err
                    );
                }
            }
        }

        {
            let mut active = active_scopes_mutex().lock().unwrap();
            active.insert(entry.path.clone(), ActiveScope { url, count: 1 });
        }

        Ok(Some(AccessToken {
            key: entry.path.clone(),
        }))
    }

    fn recover_revoked_access(
        original_path: &Path,
        entry_path: &str,
        allow_retry: bool,
        reason: &str,
    ) -> Result<Option<AccessToken>, String> {
        if !allow_retry {
            return Err(
                "macOS could not restore saved access to this folder automatically. Please open it again in Marlin to re-authorize."
                    .to_string(),
            );
        }

        let entry_path_buf = PathBuf::from(entry_path);
        match store_bookmark_if_needed(&entry_path_buf) {
            Ok(_) => retain_access_internal(original_path, false),
            Err(store_err) => {
                warn!(
                    "Failed to refresh bookmark for {} after {}: {}",
                    entry_path, reason, store_err
                );
                Err(
                    "macOS could not restore saved access to this folder automatically. Please open it again in Marlin to re-authorize."
                        .to_string(),
                )
            }
        }
    }

    pub fn store_bookmark_if_needed(path: &Path) -> Result<(), String> {
        let key = discover_scope_key(path);
        if key.is_empty() {
            return Ok(());
        }

        let target_key = if let Some(root) = sensitive_root(Path::new(&key)) {
            root
        } else {
            key.clone()
        };

        let target_path = Path::new(&target_key);
        if target_path.components().count() <= 1 {
            // Security scoped bookmarks cannot be created for filesystem roots like "/".
            return Ok(());
        }

        let store_lock = store_mutex();
        {
            let store = store_lock.lock().unwrap();
            if store.contains_parent(&target_key) {
                return Ok(());
            }
        }

        let url = unsafe { url_for_path(&target_key)? };
        let data = unsafe { bookmark_from_url(&url)? };

        let mut store = store_lock.lock().unwrap();
        store.upsert(target_key.clone(), data);
        store.save()
    }

    pub fn persist_bookmark(path: &Path, context: &str) {
        if let Err(err) = store_bookmark_if_needed(path) {
            warn!(
                "Failed to persist security bookmark after {} for {}: {}",
                context,
                path.display(),
                err
            );
        }
    }

    pub fn has_bookmark(path: &Path) -> bool {
        let key = discover_scope_key(path);
        let store = store_mutex().lock().unwrap();
        store.contains_parent(&key)
    }
}

#[cfg(target_os = "macos")]
#[allow(unused_imports)]
pub use imp::store_bookmark_if_needed;
#[cfg(target_os = "macos")]
pub use imp::{persist_bookmark, retain_access, AccessToken};

#[cfg(not(target_os = "macos"))]
mod imp_stub {
    use std::path::Path;

    #[derive(Debug, Clone)]
    pub struct AccessToken;

    pub fn retain_access(_path: &Path) -> Result<Option<AccessToken>, String> {
        Ok(None)
    }

    pub fn store_bookmark_if_needed(_path: &Path) -> Result<(), String> {
        Ok(())
    }

    pub fn persist_bookmark(_path: &Path, _context: &str) {}

    pub fn has_bookmark(_path: &Path) -> bool {
        false
    }
}

#[cfg(not(target_os = "macos"))]
#[allow(unused_imports)]
pub use imp_stub::{persist_bookmark, retain_access, AccessToken};
