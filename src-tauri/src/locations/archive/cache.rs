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

/// Lock one cache entry with `<entry>.lock` beside it.
fn lock_entry(entry: &Path) -> Result<CacheLock, String> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let lock = entry.with_extension("lock");

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

/// Lock the entry that holds the extracted file `cache_path`.
pub fn acquire_lock(cache_path: &Path) -> Result<CacheLock, String> {
    lock_entry(cache_path.parent().ok_or("Invalid cache path")?)
}

fn hash_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}

/// The path of an extracted entry: `<cache dir>/<hash of key>/<entry name>`, so an open or a copy shows the entry's own name.
pub fn cached_path_for_key(key: &str, name: &str) -> Result<PathBuf, String> {
    crate::locations::temp_copy_path(CACHE_DIR_NAME, &hash_key(key), name)
}

/// The cache's age rule: an entry is fresh while its file was modified within `max_age`.
fn is_recent(modified: SystemTime, max_age: Duration) -> bool {
    SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age <= max_age)
}

/// Returns true if the cache entry exists and is still fresh (within TTL).
/// If fresh, also touches the file to update mtime for LRU behavior.
pub fn is_cache_fresh(path: &Path) -> bool {
    let fresh = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| is_recent(modified, CACHE_TTL));
    if fresh {
        // A hit counts as a use: the age prune and the size prune both read this mtime.
        let _ = fs::set_times(path, fs::FileTimes::new().set_modified(SystemTime::now()));
    }
    fresh
}

/// Every file in a cache entry, links not followed: the entry itself when it is a file at the top of the cache.
fn entry_files(entry: &Path) -> Vec<fs::Metadata> {
    match fs::symlink_metadata(entry) {
        Ok(metadata) if metadata.is_dir() => fs::read_dir(entry)
            .into_iter()
            .flatten()
            .flatten()
            .flat_map(|child| entry_files(&child.path()))
            .collect(),
        Ok(metadata) => vec![metadata],
        Err(_) => Vec::new(),
    }
}

/// Remove a cache entry under its lock, so the prune never cuts off an extraction into it.
/// An entry with a lock file of any age stays: an extraction can hold its lock longer than `STALE_LOCK_AGE`.
/// Returns whether the entry was removed.
fn remove_entry(entry: &Path) -> bool {
    if entry.with_extension("lock").exists() {
        return false;
    }
    let Ok(_lock) = lock_entry(entry) else {
        return false;
    };
    if entry.is_dir() {
        fs::remove_dir_all(entry).is_ok()
    } else {
        fs::remove_file(entry).is_ok()
    }
}

/// Remove what extractions made and no one used within `max_age`, then the oldest entries past `CACHE_MAX_BYTES`.
pub fn prune_cache_dir(dir: &Path, max_age: Duration) -> Result<(), String> {
    let mut entries: Vec<(PathBuf, SystemTime, u64)> = Vec::new();
    let mut total_size: u64 = 0;

    let listing = match fs::read_dir(dir) {
        Ok(listing) => listing,
        // Nothing has been extracted yet.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("Failed to read cache dir: {e}")),
    };
    for entry in listing {
        let entry = entry.map_err(|e| format!("Failed to read cache entry: {e}"))?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("lock") {
            continue;
        }
        match entry_files(&path).as_slice() {
            // A thumbnail removed its copy, or an extraction stopped.
            [] => {
                remove_entry(&path);
            }
            [file] => {
                let modified = file.modified().unwrap_or(UNIX_EPOCH);
                if is_recent(modified, max_age) {
                    total_size = total_size.saturating_add(file.len());
                    entries.push((path, modified, file.len()));
                } else {
                    remove_entry(&path);
                }
            }
            // An extraction makes one file. More means the user saved something here, which the archive cannot give back.
            _ => {}
        }
    }

    if total_size > CACHE_MAX_BYTES {
        entries.sort_by_key(|entry| entry.1);
        for (path, _modified, size) in entries {
            if total_size <= CACHE_MAX_BYTES {
                break;
            }
            if remove_entry(&path) {
                total_size = total_size.saturating_sub(size);
            }
        }
    }

    Ok(())
}

pub fn prune_cache_on_startup() -> Result<(), String> {
    prune_cache_dir(&std::env::temp_dir().join(CACHE_DIR_NAME), STARTUP_TTL)
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

    prune_cache_dir(&std::env::temp_dir().join(CACHE_DIR_NAME), max_age)
}

pub fn cache_ttl() -> Duration {
    CACHE_TTL
}

#[cfg(test)]
mod tests {
    use super::{cache_ttl, cached_path_for_key, prune_cache_dir};
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    fn backdate(path: &Path) {
        let old = SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }

    #[test]
    fn cached_path_for_key_keeps_the_entry_name() {
        let key_a = format!("a-{}", uuid::Uuid::new_v4());
        let key_b = format!("b-{}", uuid::Uuid::new_v4());
        let a = cached_path_for_key(&key_a, ".env").unwrap();
        let b = cached_path_for_key(&key_b, ".env").unwrap();
        assert_eq!(a.file_name().and_then(|name| name.to_str()), Some(".env"));
        assert_eq!(b.file_name().and_then(|name| name.to_str()), Some(".env"));
        assert_ne!(a.parent(), b.parent());
        let _ = fs::remove_dir_all(a.parent().unwrap());
        let _ = fs::remove_dir_all(b.parent().unwrap());
    }

    #[test]
    fn prune_cache_dir_drops_stale_entries_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let stale_dir = root.join("stale");
        fs::create_dir(&stale_dir).unwrap();
        let stale_file = stale_dir.join("old.txt");
        fs::write(&stale_file, b"old").unwrap();
        backdate(&stale_file);

        let fresh_dir = root.join("fresh");
        fs::create_dir(&fresh_dir).unwrap();
        fs::write(fresh_dir.join("new.txt"), b"new").unwrap();

        // A file the user saved in the folder keeps the entry, however old.
        let saved_dir = root.join("saved");
        fs::create_dir(&saved_dir).unwrap();
        let saved_old = saved_dir.join("notes.txt");
        fs::write(&saved_old, b"old").unwrap();
        backdate(&saved_old);
        fs::create_dir(saved_dir.join("revisions")).unwrap();
        fs::write(saved_dir.join("revisions").join("edited.txt"), b"edited").unwrap();
        backdate(&saved_dir.join("revisions").join("edited.txt"));

        let empty_dir = root.join("empty");
        fs::create_dir(&empty_dir).unwrap();

        let held_dir = root.join("held");
        fs::create_dir(&held_dir).unwrap();
        let held_file = held_dir.join("old.txt");
        fs::write(&held_file, b"held").unwrap();
        backdate(&held_file);
        // A lock older than STALE_LOCK_AGE may still belong to a long extraction.
        fs::write(root.join("held.lock"), b"locked").unwrap();
        backdate(&root.join("held.lock"));

        let flat = root.join("flat.zip");
        fs::write(&flat, b"zip").unwrap();
        backdate(&flat);

        let staging = root.join(".__marlin_extract_x");
        fs::create_dir(&staging).unwrap();
        let staging_file = staging.join("left.txt");
        fs::write(&staging_file, b"left").unwrap();
        backdate(&staging_file);

        prune_cache_dir(root, cache_ttl()).unwrap();
        prune_cache_dir(&root.join("never made"), cache_ttl()).unwrap();

        assert!(!stale_dir.exists());
        assert!(fresh_dir.join("new.txt").is_file());
        assert!(saved_old.is_file());
        assert!(saved_dir.join("revisions").join("edited.txt").is_file());
        assert!(!empty_dir.exists());
        assert!(held_file.is_file());
        assert!(root.join("held.lock").is_file());
        assert!(!flat.exists());
        assert!(!staging.exists());
    }
}
