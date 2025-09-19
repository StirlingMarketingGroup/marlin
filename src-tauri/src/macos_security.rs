#![allow(dead_code)]

#[cfg(target_os = "macos")]
mod imp {
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};

    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    use cocoa::base::{id, nil, NO, YES};
    use cocoa::foundation::{NSAutoreleasePool, NSString};
    use dirs;
    use log::warn;
    use objc::rc::StrongPtr;
    use objc::runtime::BOOL;
    use objc::{class, msg_send, sel, sel_impl};
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
            push_dir("iCloud Drive");
        }
        dirs_vec
    }

    static STORE: OnceLock<Mutex<BookmarkStore>> = OnceLock::new();
    static ACTIVE_SCOPES: OnceLock<Mutex<HashMap<String, ActiveScope>>> = OnceLock::new();
    static SENSITIVE_DIR_SET: OnceLock<Vec<String>> = OnceLock::new();

    struct ActiveScope {
        url: StrongPtr,
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

    unsafe fn url_for_path(path: &str) -> Result<StrongPtr, String> {
        let _pool: id = NSAutoreleasePool::new(nil);
        let ns_path: id = NSString::alloc(nil).init_str(path);
        let url: id = msg_send![class!(NSURL), fileURLWithPath: ns_path];
        if url == nil {
            return Err("Failed to create NSURL".to_string());
        }
        // `fileURLWithPath:` returns an autoreleased NSURL; retain it so the
        // StrongPtr owns a stable +1 reference regardless of the surrounding
        // autorelease pool lifetime.
        Ok(StrongPtr::retain(url))
    }

    unsafe fn bookmark_from_url(url: &StrongPtr) -> Result<Vec<u8>, String> {
        let _pool: id = NSAutoreleasePool::new(nil);
        let mut error: id = nil;
        let raw_url: id = **url;
        let bookmark: id = msg_send![raw_url,
            bookmarkDataWithOptions:NS_URL_BOOKMARK_CREATION_WITH_SECURITY_SCOPE
            includingResourceValuesForKeys:nil
            relativeToURL:nil
            error:&mut error
        ];
        if bookmark == nil {
            return Err("Failed to create bookmark data".to_string());
        }
        let length: usize = msg_send![bookmark, length];
        if length == 0 {
            return Err("Bookmark data is empty".to_string());
        }
        let bytes_ptr: *const std::ffi::c_void = msg_send![bookmark, bytes];
        if bytes_ptr.is_null() {
            return Err("Bookmark bytes pointer is null".to_string());
        }
        let slice = std::slice::from_raw_parts(bytes_ptr as *const u8, length);
        Ok(slice.to_vec())
    }

    unsafe fn resolve_bookmark(data: &[u8]) -> Result<(StrongPtr, bool), String> {
        let _pool: id = NSAutoreleasePool::new(nil);
        if data.is_empty() {
            return Err("Bookmark data is empty".to_string());
        }
        let nsdata: id = msg_send![class!(NSData), dataWithBytes:data.as_ptr() length:data.len()];
        if nsdata == nil {
            return Err("Failed to build NSData from bookmark".to_string());
        }
        let mut error: id = nil;
        let mut is_stale: BOOL = NO;
        let resolved: id = msg_send![class!(NSURL),
            URLByResolvingBookmarkData: nsdata
            options: NS_URL_BOOKMARK_RESOLUTION_WITH_SECURITY_SCOPE
            relativeToURL: nil
            bookmarkDataIsStale: &mut is_stale
            error: &mut error
        ];
        if resolved == nil {
            return Err("Failed to resolve bookmark".to_string());
        }
        let started: BOOL = msg_send![resolved, startAccessingSecurityScopedResource];
        if started == NO {
            return Err("startAccessingSecurityScopedResource returned false".to_string());
        }
        Ok((StrongPtr::retain(resolved), is_stale == YES))
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
                        let raw_url: id = *entry.url;
                        let _: () = msg_send![raw_url, stopAccessingSecurityScopedResource];
                    }
                    active.remove(&self.key);
                }
            }
        }
    }

    pub fn retain_access(path: &Path) -> Result<Option<AccessToken>, String> {
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

        let data = BookmarkStore::decode_data(&entry)
            .ok_or_else(|| "Failed to decode stored bookmark".to_string())?;

        let mut active = active_scopes_mutex().lock().unwrap();
        if let Some(scope) = active.get_mut(&entry.path) {
            scope.increment();
            return Ok(Some(AccessToken {
                key: entry.path.clone(),
            }));
        }

        let (url, stale) = unsafe { resolve_bookmark(&data)? };

        if stale {
            if let Ok(new_data) = unsafe { bookmark_from_url(&url) } {
                let mut store = store_lock.lock().unwrap();
                store.upsert(entry.path.clone(), new_data);
                if let Err(err) = store.save() {
                    warn!(
                        "Failed to persist refreshed security bookmark for {}: {}",
                        entry.path,
                        err
                    );
                }
            }
        }

        active.insert(entry.path.clone(), ActiveScope { url, count: 1 });

        Ok(Some(AccessToken {
            key: entry.path.clone(),
        }))
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
pub use imp::{persist_bookmark, retain_access, AccessToken};
#[cfg(target_os = "macos")]
#[allow(unused_imports)]
pub use imp::store_bookmark_if_needed;

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
