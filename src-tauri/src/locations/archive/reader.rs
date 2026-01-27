use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use tar::Archive as TarArchive;
use unrar::Archive as RarArchive;
use xz2::read::XzDecoder;
use zip::ZipArchive;
use zstd::stream::read::Decoder as ZstdDecoder;
use uuid::Uuid;

const MAX_ENTRIES: usize = 100_000;
const MAX_TOTAL_SIZE: u64 = 2 * 1024 * 1024 * 1024;
const MAX_SINGLE_FILE: u64 = 500 * 1024 * 1024;
/// Maximum number of archives to cache directory structure for
const STRUCTURE_CACHE_MAX: usize = 50;

/// Cached archive structure with all entries (wrapped in Arc for cheap cloning)
struct CachedArchiveStructure {
    /// Archive file modification time when cached
    mtime: SystemTime,
    /// All entries in the archive (flattened), wrapped in Arc for cheap sharing
    entries: Arc<Vec<CachedEntry>>,
}

#[derive(Clone)]
struct CachedEntry {
    /// Normalized path (e.g., "foo/bar/baz.txt")
    path: String,
    is_directory: bool,
    size: u64,
    modified: DateTime<Utc>,
}

/// Global cache for archive directory structures
static STRUCTURE_CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedArchiveStructure>>> = OnceLock::new();

fn get_structure_cache() -> &'static Mutex<HashMap<PathBuf, CachedArchiveStructure>> {
    STRUCTURE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Get cached entries if valid, or None if cache miss/stale
/// Returns Arc to avoid cloning the entire entry list
fn get_cached_entries(archive_path: &Path) -> Option<Arc<Vec<CachedEntry>>> {
    let current_mtime = fs::metadata(archive_path).ok()?.modified().ok()?;
    let cache = get_structure_cache().lock().ok()?;
    let cached = cache.get(archive_path)?;
    if cached.mtime == current_mtime {
        Some(Arc::clone(&cached.entries))
    } else {
        None
    }
}

/// Store structure in cache, evicting oldest if needed
fn cache_structure(archive_path: &Path, mtime: SystemTime, entries: Vec<CachedEntry>) {
    if let Ok(mut cache) = get_structure_cache().lock() {
        // Simple eviction: if at capacity, remove an entry
        if cache.len() >= STRUCTURE_CACHE_MAX && !cache.contains_key(archive_path) {
            if let Some(key) = cache.keys().next().cloned() {
                cache.remove(&key);
            }
        }
        cache.insert(
            archive_path.to_path_buf(),
            CachedArchiveStructure {
                mtime,
                entries: Arc::new(entries),
            },
        );
    }
}

/// Build directory listing from cached entries
fn list_from_cached_entries(entries: &[CachedEntry], internal_path: &str) -> Vec<ArchiveEntry> {
    let normalized = normalize_internal_path(internal_path).unwrap_or_else(|_| "/".to_string());
    let parent_rel = normalized.trim_start_matches('/');

    let mut children: HashMap<String, EntryInfo> = HashMap::new();

    for entry in entries {
        push_entry(
            &mut children,
            parent_rel,
            &entry.path,
            entry.is_directory,
            entry.size,
            entry.modified,
        );
    }

    build_entries(parent_rel, children)
}

#[derive(Debug, Clone, Copy)]
pub enum ArchiveFormat {
    Zip,
    Rar,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    TarZst,
}

fn infer_archive_format_from_name(name: &str) -> Option<ArchiveFormat> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        Some(ArchiveFormat::TarGz)
    } else if lower.ends_with(".tar.bz2") || lower.ends_with(".tbz2") || lower.ends_with(".tbz") {
        Some(ArchiveFormat::TarBz2)
    } else if lower.ends_with(".tar.xz") || lower.ends_with(".txz") {
        Some(ArchiveFormat::TarXz)
    } else if lower.ends_with(".tar.zst") || lower.ends_with(".tzst") {
        Some(ArchiveFormat::TarZst)
    } else if lower.ends_with(".tar") {
        Some(ArchiveFormat::Tar)
    } else if lower.ends_with(".zip") {
        Some(ArchiveFormat::Zip)
    } else if lower.ends_with(".rar") {
        Some(ArchiveFormat::Rar)
    } else {
        None
    }
}

pub fn determine_archive_format(path: &Path) -> Result<ArchiveFormat, String> {
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    infer_archive_format_from_name(file_name)
        .ok_or_else(|| format!("Unsupported archive format: {}", file_name))
}

fn normalize_entry_path(raw: &str) -> Result<String, String> {
    if raw.contains('\0') {
        return Err("Archive entry contains NUL byte".to_string());
    }

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }

    let mut value = trimmed.replace('\\', "/");
    if value.starts_with('/') || value.starts_with("//") {
        return Err(format!("Refusing absolute archive entry path: {raw}"));
    }
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            return Err(format!("Refusing Windows drive path in archive entry: {raw}"));
        }
    }

    while value.starts_with("./") {
        value = value.trim_start_matches("./").to_string();
    }

    let parts: Vec<&str> = value
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();

    if parts.iter().any(|part| *part == "..") {
        return Err(format!("Refusing path traversal in archive entry: {raw}"));
    }

    Ok(parts.join("/"))
}

pub fn normalize_internal_path(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok("/".to_string());
    }
    if trimmed.contains('\0') {
        return Err("Archive path contains NUL byte".to_string());
    }

    let value = trimmed.replace('\\', "/");
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            return Err(format!("Invalid archive path: {raw}"));
        }
    }

    let parts: Vec<&str> = value
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();

    if parts.iter().any(|part| *part == "..") {
        return Err(format!("Invalid archive path traversal: {raw}"));
    }

    if parts.is_empty() {
        return Ok("/".to_string());
    }

    Ok(format!("/{}", parts.join("/")))
}

fn zip_datetime_to_chrono(value: zip::DateTime) -> Option<DateTime<Utc>> {
    let date = NaiveDate::from_ymd_opt(
        value.year().into(),
        value.month().into(),
        value.day().into(),
    )?;
    let time = NaiveTime::from_hms_opt(
        value.hour().into(),
        value.minute().into(),
        value.second().into(),
    )?;
    Some(DateTime::<Utc>::from_naive_utc_and_offset(
        NaiveDateTime::new(date, time),
        Utc,
    ))
}

fn mtime_to_datetime(secs: u64) -> DateTime<Utc> {
    UNIX_EPOCH
        .checked_add(Duration::from_secs(secs))
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(Utc::now)
}

#[derive(Debug, Clone)]
pub struct ArchiveEntry {
    pub name: String,
    pub internal_path: String,
    pub is_directory: bool,
    pub size: u64,
    pub modified: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct EntryInfo {
    is_directory: bool,
    size: u64,
    modified: DateTime<Utc>,
}

fn push_entry(
    map: &mut HashMap<String, EntryInfo>,
    parent_prefix: &str,
    entry_path: &str,
    entry_is_dir: bool,
    entry_size: u64,
    entry_modified: DateTime<Utc>,
) {
    let (rest, has_prefix) = if parent_prefix.is_empty() {
        (entry_path, true)
    } else if entry_path == parent_prefix {
        ("", true)
    } else if entry_path.starts_with(parent_prefix) {
        let trimmed = entry_path.trim_start_matches(parent_prefix);
        (trimmed.trim_start_matches('/'), true)
    } else {
        ("", false)
    };

    if !has_prefix || rest.is_empty() {
        return;
    }

    let mut parts = rest.split('/');
    let first = match parts.next() {
        Some(value) if !value.is_empty() => value,
        _ => return,
    };
    let has_more = parts.next().is_some();
    let child_is_dir = entry_is_dir || has_more;

    let entry = map.entry(first.to_string()).or_insert(EntryInfo {
        is_directory: child_is_dir,
        size: entry_size,
        modified: entry_modified,
    });

    if child_is_dir {
        entry.is_directory = true;
        entry.size = 0;
    }
}

fn build_entries(parent_path: &str, map: HashMap<String, EntryInfo>) -> Vec<ArchiveEntry> {
    let mut entries: Vec<ArchiveEntry> = Vec::new();
    for (name, info) in map {
        let internal_path = if parent_path.is_empty() {
            format!("/{}", name)
        } else {
            format!("/{}/{}", parent_path, name)
        };
        entries.push(ArchiveEntry {
            name,
            internal_path,
            is_directory: info.is_directory,
            size: info.size,
            modified: info.modified,
        });
    }
    entries
}

pub fn list_directory(archive_path: &Path, internal_path: &str) -> Result<Vec<ArchiveEntry>, String> {
    // Check cache first - avoids O(n) scan for repeated listings
    // Uses Arc for cheap cloning of the entry list
    if let Some(cached_entries) = get_cached_entries(archive_path) {
        return Ok(list_from_cached_entries(&cached_entries, internal_path));
    }

    let format = determine_archive_format(archive_path)?;
    let normalized_path = normalize_internal_path(internal_path)?;
    let parent_rel = normalized_path.trim_start_matches('/');

    let mut children: HashMap<String, EntryInfo> = HashMap::new();
    // Collect all entries for caching
    let mut all_entries: Vec<CachedEntry> = Vec::new();
    let mut seen_entries: usize = 0;
    let mut total_size: u64 = 0;

    match format {
        ArchiveFormat::Zip => {
            let file = File::open(archive_path)
                .map_err(|e| format!("Failed to open archive {}: {e}", archive_path.display()))?;
            let mut archive = ZipArchive::new(file)
                .map_err(|e| format!("Failed to read zip archive: {e}"))?;

            if archive.len() > MAX_ENTRIES {
                return Err("Archive contains too many entries to list".to_string());
            }

            for i in 0..archive.len() {
                let file = archive
                    .by_index(i)
                    .map_err(|e| format!("Failed to read zip entry: {e}"))?;

                seen_entries += 1;
                if seen_entries > MAX_ENTRIES {
                    return Err("Archive contains too many entries to list".to_string());
                }

                let entry_name = file.name();
                let normalized = match normalize_entry_path(entry_name) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                if normalized.is_empty() {
                    continue;
                }

                let size = file.size();
                if size > MAX_SINGLE_FILE {
                    return Err(format!("Archive entry too large: {entry_name}"));
                }
                total_size = total_size.saturating_add(size);
                if total_size > MAX_TOTAL_SIZE {
                    return Err("Archive exceeds size limit".to_string());
                }

                let modified = file
                    .last_modified()
                    .and_then(zip_datetime_to_chrono)
                    .unwrap_or_else(Utc::now);

                let is_dir = file.is_dir();

                // Collect for cache
                all_entries.push(CachedEntry {
                    path: normalized.clone(),
                    is_directory: is_dir,
                    size,
                    modified,
                });

                push_entry(
                    &mut children,
                    parent_rel,
                    &normalized,
                    is_dir,
                    size,
                    modified,
                );
            }
        }
        ArchiveFormat::Rar => {
            let archive = RarArchive::new(archive_path)
                .open_for_listing()
                .map_err(|e| format!("Failed to open RAR archive: {e}"))?;

            for header_result in archive {
                let header = header_result
                    .map_err(|e| format!("Failed to read RAR entry: {e}"))?;

                seen_entries += 1;
                if seen_entries > MAX_ENTRIES {
                    return Err("Archive contains too many entries to list".to_string());
                }

                let entry_name = header
                    .filename
                    .to_string_lossy()
                    .to_string();
                let normalized = match normalize_entry_path(&entry_name) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                if normalized.is_empty() {
                    continue;
                }

                let size = header.unpacked_size;
                if size > MAX_SINGLE_FILE {
                    return Err(format!("Archive entry too large: {entry_name}"));
                }
                total_size = total_size.saturating_add(size);
                if total_size > MAX_TOTAL_SIZE {
                    return Err("Archive exceeds size limit".to_string());
                }

                let is_dir = header.is_directory();
                let modified = Utc::now();

                // Collect for cache
                all_entries.push(CachedEntry {
                    path: normalized.clone(),
                    is_directory: is_dir,
                    size,
                    modified,
                });

                push_entry(
                    &mut children,
                    parent_rel,
                    &normalized,
                    is_dir,
                    size,
                    modified,
                );
            }
        }
        ArchiveFormat::Tar
        | ArchiveFormat::TarGz
        | ArchiveFormat::TarBz2
        | ArchiveFormat::TarXz
        | ArchiveFormat::TarZst => {
            let reader = create_tar_reader(format, archive_path)?;
            let mut archive = TarArchive::new(reader);
            let entries = archive
                .entries()
                .map_err(|e| format!("Failed to read tar entries: {e}"))?;

            for entry_result in entries {
                let entry = entry_result.map_err(|e| format!("Failed to read tar entry: {e}"))?;
                let path = entry
                    .path()
                    .map_err(|e| format!("Failed to read tar entry path: {e}"))?;
                let entry_name = path.to_string_lossy().to_string();

                seen_entries += 1;
                if seen_entries > MAX_ENTRIES {
                    return Err("Archive contains too many entries to list".to_string());
                }

                let normalized = match normalize_entry_path(&entry_name) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                if normalized.is_empty() {
                    continue;
                }

                let header = entry.header();
                let size = header.size().unwrap_or(0);
                if size > MAX_SINGLE_FILE {
                    return Err(format!("Archive entry too large: {entry_name}"));
                }
                total_size = total_size.saturating_add(size);
                if total_size > MAX_TOTAL_SIZE {
                    return Err("Archive exceeds size limit".to_string());
                }

                let modified = header
                    .mtime()
                    .map(mtime_to_datetime)
                    .unwrap_or_else(|_| Utc::now());

                let entry_type = header.entry_type();
                let is_dir = entry_type.is_dir();

                // Collect for cache
                all_entries.push(CachedEntry {
                    path: normalized.clone(),
                    is_directory: is_dir,
                    size,
                    modified,
                });

                push_entry(&mut children, parent_rel, &normalized, is_dir, size, modified);
            }
        }
    }

    // Cache the full structure for future lookups
    if let Ok(mtime) = fs::metadata(archive_path).and_then(|m| m.modified()) {
        cache_structure(archive_path, mtime, all_entries);
    }

    Ok(build_entries(parent_rel, children))
}

pub fn get_entry_metadata(
    archive_path: &Path,
    internal_path: &str,
) -> Result<ArchiveEntry, String> {
    let normalized = normalize_internal_path(internal_path)?;
    let target_rel = normalized.trim_start_matches('/');
    if target_rel.is_empty() {
        return Ok(ArchiveEntry {
            name: archive_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("Archive")
                .to_string(),
            internal_path: "/".to_string(),
            is_directory: true,
            size: 0,
            modified: Utc::now(),
        });
    }

    // Check cache first - avoids O(n) archive scan for repeated metadata lookups
    if let Some(cached_entries) = get_cached_entries(archive_path) {
        // Look for exact match
        for entry in cached_entries.iter() {
            if entry.path == target_rel {
                return Ok(ArchiveEntry {
                    name: Path::new(target_rel)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(target_rel)
                        .to_string(),
                    internal_path: normalized.clone(),
                    is_directory: entry.is_directory,
                    size: entry.size,
                    modified: entry.modified,
                });
            }
        }
        // Check if it's an implicit directory (has children but no explicit entry)
        let prefix = format!("{}/", target_rel);
        if cached_entries.iter().any(|e| e.path.starts_with(&prefix)) {
            return Ok(ArchiveEntry {
                name: Path::new(target_rel)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(target_rel)
                    .to_string(),
                internal_path: normalized,
                is_directory: true,
                size: 0,
                modified: Utc::now(),
            });
        }
        return Err("Archive entry not found".to_string());
    }

    // Cache miss - fall back to scanning the archive
    let format = determine_archive_format(archive_path)?;
    let mut found_dir = false;

    match format {
        ArchiveFormat::Zip => {
            let file = File::open(archive_path)
                .map_err(|e| format!("Failed to open archive {}: {e}", archive_path.display()))?;
            let mut archive = ZipArchive::new(file)
                .map_err(|e| format!("Failed to read zip archive: {e}"))?;

            for i in 0..archive.len() {
                let file = archive
                    .by_index(i)
                    .map_err(|e| format!("Failed to read zip entry: {e}"))?;
                let entry_name = file.name();
                let normalized_entry = match normalize_entry_path(entry_name) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                if normalized_entry == target_rel {
                    return Ok(ArchiveEntry {
                        name: Path::new(target_rel)
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(target_rel)
                            .to_string(),
                        internal_path: normalized.clone(),
                        is_directory: file.is_dir(),
                        size: file.size(),
                        modified: file
                            .last_modified()
                            .and_then(zip_datetime_to_chrono)
                            .unwrap_or_else(Utc::now),
                    });
                }
                if normalized_entry.starts_with(&format!("{}/", target_rel)) {
                    found_dir = true;
                }
            }
        }
        ArchiveFormat::Rar => {
            let archive = RarArchive::new(archive_path)
                .open_for_listing()
                .map_err(|e| format!("Failed to open RAR archive: {e}"))?;

            for header_result in archive {
                let header = header_result
                    .map_err(|e| format!("Failed to read RAR entry: {e}"))?;
                let entry_name = header.filename.to_string_lossy().to_string();
                let normalized_entry = match normalize_entry_path(&entry_name) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                if normalized_entry == target_rel {
                    return Ok(ArchiveEntry {
                        name: Path::new(target_rel)
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(target_rel)
                            .to_string(),
                        internal_path: normalized.clone(),
                        is_directory: header.is_directory(),
                        size: header.unpacked_size,
                        modified: Utc::now(),
                    });
                }
                if normalized_entry.starts_with(&format!("{}/", target_rel)) {
                    found_dir = true;
                }
            }
        }
        ArchiveFormat::Tar
        | ArchiveFormat::TarGz
        | ArchiveFormat::TarBz2
        | ArchiveFormat::TarXz
        | ArchiveFormat::TarZst => {
            let reader = create_tar_reader(format, archive_path)?;
            let mut archive = TarArchive::new(reader);
            let entries = archive
                .entries()
                .map_err(|e| format!("Failed to read tar entries: {e}"))?;

            for entry_result in entries {
                let entry = entry_result.map_err(|e| format!("Failed to read tar entry: {e}"))?;
                let path = entry
                    .path()
                    .map_err(|e| format!("Failed to read tar entry path: {e}"))?;
                let entry_name = path.to_string_lossy().to_string();
                let normalized_entry = match normalize_entry_path(&entry_name) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                if normalized_entry == target_rel {
                    let header = entry.header();
                    let entry_type = header.entry_type();
                    let size = header.size().unwrap_or(0);
                    let modified = header
                        .mtime()
                        .map(mtime_to_datetime)
                        .unwrap_or_else(|_| Utc::now());
                    return Ok(ArchiveEntry {
                        name: Path::new(target_rel)
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(target_rel)
                            .to_string(),
                        internal_path: normalized.clone(),
                        is_directory: entry_type.is_dir(),
                        size,
                        modified,
                    });
                }
                if normalized_entry.starts_with(&format!("{}/", target_rel)) {
                    found_dir = true;
                }
            }
        }
    }

    if found_dir {
        return Ok(ArchiveEntry {
            name: Path::new(target_rel)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(target_rel)
                .to_string(),
            internal_path: normalized,
            is_directory: true,
            size: 0,
            modified: Utc::now(),
        });
    }

    Err("Archive entry not found".to_string())
}

pub fn extract_entry_to_dir(
    archive_path: &Path,
    internal_path: &str,
    output_dir: &Path,
) -> Result<PathBuf, String> {
    let format = determine_archive_format(archive_path)?;
    let normalized = normalize_internal_path(internal_path)?;
    let target_rel = normalized.trim_start_matches('/');
    if target_rel.is_empty() {
        return Err("Cannot extract archive root".to_string());
    }

    match format {
        ArchiveFormat::Zip => {
            let file = File::open(archive_path)
                .map_err(|e| format!("Failed to open archive {}: {e}", archive_path.display()))?;
            let mut archive = ZipArchive::new(file)
                .map_err(|e| format!("Failed to read zip archive: {e}"))?;

            // Find the entry by normalized path (handles ./foo.txt, backslashes, etc.)
            let mut found_index: Option<usize> = None;
            for i in 0..archive.len() {
                if let Ok(entry) = archive.by_index(i) {
                    if let Ok(normalized) = normalize_entry_path(entry.name()) {
                        if normalized == target_rel {
                            found_index = Some(i);
                            break;
                        }
                    }
                }
            }

            let entry_index = found_index.ok_or_else(|| "Archive entry not found".to_string())?;
            let mut entry = archive
                .by_index(entry_index)
                .map_err(|e| format!("Failed to read zip entry: {e}"))?;

            if entry.is_dir() {
                return Err("Cannot extract directory entry".to_string());
            }
            // Security: reject symlinks in ZIP files
            if entry.is_symlink() {
                return Err("Cannot extract symlink entries".to_string());
            }
            if entry.size() > MAX_SINGLE_FILE {
                return Err("Archive entry too large".to_string());
            }
            // Security: canonicalize output_dir first, then use it for all path operations
            // This ensures consistent paths even when symlinks are involved (e.g., /var/folders on macOS)
            let canonical_output = output_dir
                .canonicalize()
                .unwrap_or_else(|_| output_dir.to_path_buf());
            // Lexically check the path first (before any filesystem operations)
            // This catches obvious traversal attempts like "../../../etc/passwd"
            for component in Path::new(target_rel).components() {
                if let std::path::Component::ParentDir = component {
                    return Err("Archive entry escapes output directory".to_string());
                }
            }
            // Use canonical path for joining to ensure consistent paths
            let out_path = canonical_output.join(target_rel);
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create directory: {e}"))?;
            }
            // Post-creation canonical check (handles symlink attacks on created directories)
            let canonical_out = out_path
                .parent()
                .and_then(|p| p.canonicalize().ok())
                .map(|p| p.join(out_path.file_name().unwrap_or_default()))
                .unwrap_or_else(|| out_path.clone());
            if !canonical_out.starts_with(&canonical_output) {
                return Err("Archive entry escapes output directory".to_string());
            }
            let mut outfile = File::create(&out_path)
                .map_err(|e| format!("Failed to create output file: {e}"))?;
            std::io::copy(&mut entry, &mut outfile)
                .map_err(|e| format!("Failed to write output file: {e}"))?;
            Ok(out_path)
        }
        ArchiveFormat::Rar => {
            let mut archive = RarArchive::new(archive_path)
                .open_for_processing()
                .map_err(|e| format!("Failed to open RAR archive: {e}"))?;

            loop {
                match archive.read_header() {
                    Ok(Some(header)) => {
                        let entry = header.entry();
                        let entry_name = entry.filename.to_string_lossy().to_string();
                        let normalized_entry = match normalize_entry_path(&entry_name) {
                            Ok(value) => value,
                            Err(_) => {
                                archive = header.skip().map_err(|e| e.to_string())?;
                                continue;
                            }
                        };
                        if normalized_entry == target_rel {
                            if entry.is_directory() {
                                return Err("Cannot extract directory entry".to_string());
                            }
                            if entry.unpacked_size > MAX_SINGLE_FILE {
                                return Err("Archive entry too large".to_string());
                            }
                            let target_dir = output_dir.to_path_buf();
                            header
                                .extract_with_base(&target_dir)
                                .map_err(|e| format!("Failed to extract RAR entry: {e}"))?;
                            return Ok(target_dir.join(target_rel));
                        }
                        archive = header.skip().map_err(|e| e.to_string())?;
                    }
                    Ok(None) => break,
                    Err(err) => return Err(format!("Failed to read RAR entry: {err}")),
                }
            }

            Err("Archive entry not found".to_string())
        }
        ArchiveFormat::Tar
        | ArchiveFormat::TarGz
        | ArchiveFormat::TarBz2
        | ArchiveFormat::TarXz
        | ArchiveFormat::TarZst => {
            let reader = create_tar_reader(format, archive_path)?;
            let mut archive = TarArchive::new(reader);
            let entries = archive
                .entries()
                .map_err(|e| format!("Failed to read tar entries: {e}"))?;

            for entry_result in entries {
                let mut entry = entry_result.map_err(|e| format!("Failed to read tar entry: {e}"))?;
                let path = entry
                    .path()
                    .map_err(|e| format!("Failed to read tar entry path: {e}"))?;
                let entry_name = path.to_string_lossy().to_string();
                let normalized_entry = match normalize_entry_path(&entry_name) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                if normalized_entry == target_rel {
                    let header = entry.header();
                    let entry_type = header.entry_type();

                    // Security: reject symlinks to prevent directory traversal attacks
                    if entry_type.is_symlink() || entry_type.is_hard_link() {
                        return Err("Cannot extract symlink or hard link entries".to_string());
                    }

                    if entry_type.is_dir() {
                        return Err("Cannot extract directory entry".to_string());
                    }
                    let size = header.size().unwrap_or(0);
                    if size > MAX_SINGLE_FILE {
                        return Err("Archive entry too large".to_string());
                    }
                    // Security: canonicalize output_dir first, then use it for all path operations
                    let canonical_output = output_dir
                        .canonicalize()
                        .unwrap_or_else(|_| output_dir.to_path_buf());
                    // Lexical check first (before any filesystem operations)
                    for component in Path::new(target_rel).components() {
                        if let std::path::Component::ParentDir = component {
                            return Err("Archive entry escapes output directory".to_string());
                        }
                    }
                    // Use canonical path for joining to ensure consistent paths
                    let out_path = canonical_output.join(target_rel);
                    if let Some(parent) = out_path.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| format!("Failed to create directory: {e}"))?;
                    }
                    // Post-creation canonical check (handles symlink attacks on created directories)
                    let canonical_out = out_path
                        .parent()
                        .and_then(|p| p.canonicalize().ok())
                        .map(|p| p.join(out_path.file_name().unwrap_or_default()))
                        .unwrap_or_else(|| out_path.clone());
                    if !canonical_out.starts_with(&canonical_output) {
                        return Err("Archive entry escapes output directory".to_string());
                    }
                    let mut outfile = File::create(&out_path)
                        .map_err(|e| format!("Failed to create output file: {e}"))?;
                    std::io::copy(&mut entry, &mut outfile)
                        .map_err(|e| format!("Failed to write output file: {e}"))?;
                    return Ok(out_path);
                }
            }

            Err("Archive entry not found".to_string())
        }
    }
}

pub fn extract_entry_to_path(
    archive_path: &Path,
    internal_path: &str,
    output_path: &Path,
) -> Result<PathBuf, String> {
    let parent = output_path
        .parent()
        .ok_or_else(|| "Invalid output path".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create output directory: {e}"))?;

    let temp_dir = parent.join(format!(".__marlin_extract_{}", Uuid::new_v4()));
    fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("Failed to create temp directory: {e}"))?;

    let extracted = extract_entry_to_dir(archive_path, internal_path, &temp_dir)?;

    // On Windows, fs::rename fails if target exists, so remove it first
    if output_path.exists() {
        let _ = fs::remove_file(output_path);
    }

    fs::rename(&extracted, output_path)
        .map_err(|e| format!("Failed to finalize extracted entry: {e}"))?;
    let _ = fs::remove_dir_all(&temp_dir);
    Ok(output_path.to_path_buf())
}

fn create_tar_reader(
    format: ArchiveFormat,
    archive_path: &Path,
) -> Result<Box<dyn Read>, String> {
    let file = File::open(archive_path)
        .map_err(|e| format!("Failed to open archive {}: {e}", archive_path.display()))?;
    let reader: Box<dyn Read> = match format {
        ArchiveFormat::Tar => Box::new(file),
        ArchiveFormat::TarGz => Box::new(GzDecoder::new(file)),
        ArchiveFormat::TarBz2 => Box::new(BzDecoder::new(file)),
        ArchiveFormat::TarXz => Box::new(XzDecoder::new(file)),
        ArchiveFormat::TarZst => Box::new(
            ZstdDecoder::new(file)
                .map_err(|e| format!("Failed to read zst archive: {e}"))?,
        ),
        ArchiveFormat::Zip | ArchiveFormat::Rar => {
            return Err("Invalid tar archive format".to_string())
        }
    };
    Ok(reader)
}
