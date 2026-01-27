use chrono::Utc;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CACHE_DIR_NAME: &str = "marlin-archives";
const CACHE_MAX_BYTES: u64 = 1_000_000_000;
const CACHE_TTL: Duration = Duration::from_secs(60 * 60 * 24);
const STARTUP_TTL: Duration = Duration::from_secs(60 * 60);
/// Minimum interval between cache prune operations (5 minutes)
const PRUNE_DEBOUNCE: Duration = Duration::from_secs(60 * 5);
/// Lock files older than this are considered stale and can be removed
const STALE_LOCK_AGE: Duration = Duration::from_secs(60 * 5);

/// Tracks the last time we ran cache pruning (epoch seconds)
static LAST_PRUNE_TIME: AtomicU64 = AtomicU64::new(0);

pub struct CacheLock {
    path: PathBuf,
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn cache_dir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(CACHE_DIR_NAME);
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create archive cache directory: {e}"))?;
    Ok(dir)
}

fn lock_path(cache_path: &Path) -> PathBuf {
    cache_path.with_extension("lock")
}

pub fn acquire_lock(cache_path: &Path) -> Result<CacheLock, String> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let lock = lock_path(cache_path);

    // Check for stale locks from crashed processes and clean them up
    if lock.exists() {
        if let Ok(metadata) = fs::metadata(&lock) {
            if let Ok(modified) = metadata.modified() {
                if let Ok(age) = SystemTime::now().duration_since(modified) {
                    if age > STALE_LOCK_AGE {
                        // Lock is stale (process likely crashed), remove it
                        let _ = fs::remove_file(&lock);
                    }
                }
            }
        }
    }

    // Use create_new for atomic lock acquisition - fails if file already exists
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                "Cache entry is locked by another process".to_string()
            } else {
                format!("Failed to create archive lock: {e}")
            }
        })?;

    // Write timestamp for debugging purposes
    let _ = file.write_all(Utc::now().to_rfc3339().as_bytes());

    Ok(CacheLock { path: lock })
}

fn hash_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn cached_path_for_key(key: &str, extension: Option<&str>) -> Result<PathBuf, String> {
    let dir = cache_dir()?;
    let mut filename = hash_key(key);
    if let Some(ext) = extension {
        let ext = ext.trim_start_matches('.');
        if !ext.is_empty() {
            filename.push('.');
            filename.push_str(ext);
        }
    }
    Ok(dir.join(filename))
}

fn is_locked(path: &Path) -> bool {
    lock_path(path).exists()
}

fn is_fresh(path: &Path, max_age: Duration) -> bool {
    let metadata = match fs::metadata(path) {
        Ok(value) => value,
        Err(_) => return false,
    };
    let modified = match metadata.modified() {
        Ok(value) => value,
        Err(_) => return false,
    };
    match SystemTime::now().duration_since(modified) {
        Ok(age) => age <= max_age,
        Err(_) => false,
    }
}

/// Returns true if the cache entry exists and is still fresh (within TTL).
/// Note: This only returns true for fresh entries, not stale ones.
/// If fresh, also touches the file to update mtime for LRU behavior.
pub fn is_cache_fresh(path: &Path) -> bool {
    if path.exists() && is_fresh(path, CACHE_TTL) {
        // Touch the file to update mtime - this makes the cache behave like LRU
        // so frequently accessed entries aren't evicted just because they're old
        touch_file(path);
        true
    } else {
        false
    }
}

/// Update the mtime of a file to now (for LRU cache behavior)
fn touch_file(path: &Path) {
    // Use filetime crate to update mtime without reading file contents
    // This avoids loading potentially large cache entries (up to 500MB) into memory
    let now = filetime::FileTime::now();
    let _ = filetime::set_file_mtime(path, now);
}

pub fn prune_cache_dir(max_age: Duration) -> Result<(), String> {
    let dir = cache_dir()?;
    let mut entries: Vec<(PathBuf, SystemTime, u64)> = Vec::new();
    let mut total_size: u64 = 0;

    for entry in fs::read_dir(&dir).map_err(|e| format!("Failed to read cache dir: {e}"))? {
        let entry = entry.map_err(|e| format!("Failed to read cache entry: {e}"))?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("lock") {
            continue;
        }
        if is_locked(&path) {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(value) => value,
            Err(_) => continue,
        };
        if metadata.is_dir() {
            continue;
        }
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let size = metadata.len();
        if !is_fresh(&path, max_age) {
            let _ = fs::remove_file(&path);
            continue;
        }
        total_size = total_size.saturating_add(size);
        entries.push((path, modified, size));
    }

    if total_size > CACHE_MAX_BYTES {
        entries.sort_by_key(|entry| entry.1);
        for (path, _modified, size) in entries {
            if total_size <= CACHE_MAX_BYTES {
                break;
            }
            if is_locked(&path) {
                continue;
            }
            if fs::remove_file(&path).is_ok() {
                total_size = total_size.saturating_sub(size);
            }
        }
    }

    Ok(())
}

pub fn prune_cache_on_startup() -> Result<(), String> {
    prune_cache_dir(STARTUP_TTL)
}

/// Prune the cache directory, but only if we haven't pruned recently.
/// This prevents expensive directory scans when many thumbnails trigger extractions.
pub fn prune_cache_debounced(max_age: Duration) -> Result<(), String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let last = LAST_PRUNE_TIME.load(Ordering::Relaxed);
    let debounce_secs = PRUNE_DEBOUNCE.as_secs();

    // Skip if we pruned recently
    if last > 0 && now.saturating_sub(last) < debounce_secs {
        return Ok(());
    }

    // Try to claim the prune slot (compare-and-swap)
    if LAST_PRUNE_TIME
        .compare_exchange(last, now, Ordering::SeqCst, Ordering::Relaxed)
        .is_err()
    {
        // Another thread is pruning, skip
        return Ok(());
    }

    prune_cache_dir(max_age)
}

pub fn cache_ttl() -> Duration {
    CACHE_TTL
}
