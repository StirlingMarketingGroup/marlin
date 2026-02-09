use chrono::{DateTime, Utc};
use image::ImageReader;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
#[cfg(target_family = "unix")]
use std::ffi::CString;
use std::fs;
#[cfg(target_family = "unix")]
use std::os::unix::ffi::OsStrExt;
#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

#[cfg(target_family = "unix")]
use std::io;

#[cfg(target_os = "windows")]
use windows::core::PCWSTR;
#[cfg(target_os = "windows")]
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

#[cfg(target_os = "macos")]
use crate::macos_security;

#[cfg(target_os = "macos")]
pub fn get_fs_info(path: &Path) -> Result<(u64, String), String> {
    use std::mem;

    let path_str = path.to_string_lossy();
    let c_path = CString::new(path_str.as_bytes()).map_err(|e| e.to_string())?;

    unsafe {
        let mut stats: libc::statfs = mem::zeroed();
        if libc::statfs(c_path.as_ptr(), &mut stats) == 0 {
            // fsid_t has a private __fsid_val field, so use transmute to access its bytes
            let fsid_bytes: [i32; 2] = mem::transmute(stats.f_fsid);
            let id = (fsid_bytes[0] as u64) << 32 | (fsid_bytes[1] as u32 as u64);

            let type_name_ptr = stats.f_fstypename.as_ptr();
            let type_name = std::ffi::CStr::from_ptr(type_name_ptr)
                .to_string_lossy()
                .into_owned();

            Ok((id, type_name))
        } else {
            Err("statfs failed".to_string())
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn get_fs_info(_path: &Path) -> Result<(u64, String), String> {
    Ok((0, "unknown".to_string()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileItem {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub modified: DateTime<Utc>,
    pub is_directory: bool,
    pub is_hidden: bool,
    pub is_symlink: bool,
    pub is_git_repo: bool,
    pub extension: Option<String>,
    pub child_count: Option<u64>,
    pub image_width: Option<u32>,
    pub image_height: Option<u32>,
    /// Remote file ID (e.g., Google Drive file ID)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_id: Option<String>,
    /// Remote thumbnail URL (e.g., Google Drive thumbnail link)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    /// Remote download URL (e.g., Google Drive web content link)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymlinkResolution {
    pub parent: String,
    pub target: String,
}

/// A batch of files emitted during streaming directory reads
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryBatch {
    pub session_id: String,
    pub batch_index: u32,
    pub entries: Vec<FileItem>,
    pub is_final: bool,
    pub total_count: Option<u32>,
}

/// Metadata updates for files (sent after initial skeleton batch)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMetadataUpdate {
    pub path: String,
    pub size: u64,
    pub modified: DateTime<Utc>,
    pub is_directory: bool,
    pub is_symlink: bool,
    pub is_git_repo: bool,
    pub child_count: Option<u64>,
    pub image_width: Option<u32>,
    pub image_height: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataBatch {
    pub session_id: String,
    pub updates: Vec<FileMetadataUpdate>,
    pub is_final: bool,
}

/// Files that should be treated as hidden regardless of their name starting with a dot.
/// These are typically system-generated files that users don't want to see.
/// Note: .DS_Store is already covered by the dotfile check.
const HIDDEN_SYSTEM_FILES: &[&str] = &[
    "Thumbs.db",      // Windows thumbnail cache
    "desktop.ini",    // Windows folder settings
    "ehthumbs.db",    // Windows Media Center thumbnails
    "ehthumbs_vista.db", // Vista Media Center thumbnails
    "$RECYCLE.BIN",   // Windows recycle bin folder
    "System Volume Information", // Windows system folder
];

/// Check if a file should be considered hidden.
/// Returns true for dotfiles and known system files.
pub fn is_hidden_file(name: &str) -> bool {
    name.starts_with('.') || HIDDEN_SYSTEM_FILES.iter().any(|&f| name.eq_ignore_ascii_case(f))
}

/// Build a skeleton FileItem from a DirEntry without any stat() calls.
/// Uses only information available from readdir (name, file_type via d_type on Unix).
fn build_file_item_skeleton(entry: &std::fs::DirEntry) -> Option<FileItem> {
    let path = entry.path();
    let file_name = entry.file_name().to_string_lossy().to_string();

    // file_type() uses d_type on Unix (no syscall) but may need stat on some filesystems
    // It's still much faster than full metadata as it's often cached
    let file_type = entry.file_type().ok()?;
    let is_directory = file_type.is_dir();
    let is_hidden = is_hidden_file(&file_name);

    let extension = if !is_directory {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|s| s.to_lowercase())
    } else {
        None
    };

    Some(FileItem {
        name: file_name,
        path: path.to_string_lossy().to_string(),
        size: 0, // Filled in by metadata update
        modified: Utc::now(), // Placeholder, filled in by metadata update
        is_directory,
        is_hidden,
        is_symlink: file_type.is_symlink(),
        is_git_repo: false, // Filled in by metadata update
        extension,
        child_count: None, // Filled in by metadata update
        image_width: None, // Filled in by metadata update
        image_height: None, // Filled in by metadata update
        remote_id: None,
        thumbnail_url: None,
        download_url: None,
    })
}

/// Build full metadata for a file (called in background after skeleton is displayed)
fn build_file_metadata(path: &Path) -> Option<FileMetadataUpdate> {
    let symlink_metadata = fs::symlink_metadata(path).ok()?;
    let is_symlink = symlink_metadata.file_type().is_symlink();

    let target_metadata = if is_symlink {
        fs::metadata(path).ok()
    } else {
        None
    };

    let metadata = target_metadata.as_ref().unwrap_or(&symlink_metadata);
    let is_directory = metadata.is_dir();

    let is_git_repo = if is_directory {
        let git_path = path.join(".git");
        git_path.is_dir() || git_path.is_file()
    } else {
        false
    };

    // Compute shallow child count for directories
    let child_count = if is_directory {
        fs::read_dir(path).ok().map(|entries| entries.count() as u64)
    } else {
        None
    };

    // Extract image dimensions for supported image formats
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|s| s.to_lowercase());

    let (image_width, image_height) = if !is_directory {
        match extension.as_deref() {
            Some(
                "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "tga" | "ico",
            ) => ImageReader::open(path)
                .ok()
                .and_then(|reader| reader.into_dimensions().ok())
                .map(|(w, h)| (Some(w), Some(h)))
                .unwrap_or((None, None)),
            _ => (None, None),
        }
    } else {
        (None, None)
    };

    let modified = metadata
        .modified()
        .map(|time| DateTime::from(time))
        .unwrap_or_else(|_| Utc::now());

    Some(FileMetadataUpdate {
        path: path.to_string_lossy().to_string(),
        size: metadata.len(),
        modified,
        is_directory,
        is_symlink,
        is_git_repo,
        child_count,
        image_width,
        image_height,
    })
}

pub fn resolve_symlink_parent(path: &Path) -> Result<SymlinkResolution, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|e| format!("Failed to get metadata: {}", e))?;

    if !metadata.file_type().is_symlink() {
        return Err("Path is not a symlink".to_string());
    }

    let target =
        fs::canonicalize(path).map_err(|e| format!("Failed to resolve symlink target: {}", e))?;

    let parent = target
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| target.clone());

    Ok(SymlinkResolution {
        parent: parent.to_string_lossy().to_string(),
        target: target.to_string_lossy().to_string(),
    })
}

fn build_file_item(path: &Path) -> Result<FileItem, String> {
    let symlink_metadata =
        fs::symlink_metadata(path).map_err(|e| format!("Failed to get metadata: {}", e))?;

    let is_symlink = symlink_metadata.file_type().is_symlink();
    let target_metadata = if is_symlink {
        fs::metadata(path).ok()
    } else {
        None
    };

    let mut is_directory = target_metadata
        .as_ref()
        .map(|m| m.is_dir())
        .unwrap_or_else(|| symlink_metadata.is_dir());

    if !is_directory && is_symlink {
        if fs::read_dir(path).is_ok() {
            is_directory = true;
        }
    }

    let metadata = target_metadata.as_ref().unwrap_or(&symlink_metadata);

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Unknown")
        .to_string();

    let is_hidden = is_hidden_file(&file_name);

    let extension = if metadata.is_file() {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|s| s.to_lowercase())
    } else {
        None
    };

    let is_git_repo = if is_directory {
        let git_path = path.join(".git");
        git_path.is_dir() || git_path.is_file()
    } else {
        false
    };

    // Compute shallow child count for directories
    let child_count = if is_directory {
        match fs::read_dir(path) {
            Ok(entries) => Some(entries.count() as u64),
            Err(_) => None, // Can't read directory (permissions, etc.)
        }
    } else {
        None
    };

    // Extract image dimensions for supported image formats
    // This only reads file headers, not the full image data
    let (image_width, image_height) = if !is_directory {
        match extension.as_deref() {
            Some(
                "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "tga" | "ico",
            ) => {
                // Use image crate to read dimensions from headers
                ImageReader::open(path)
                    .ok()
                    .and_then(|reader| reader.into_dimensions().ok())
                    .map(|(w, h)| (Some(w), Some(h)))
                    .unwrap_or((None, None))
            }
            _ => (None, None),
        }
    } else {
        (None, None)
    };

    let modified = metadata
        .modified()
        .map(|time| DateTime::from(time))
        .unwrap_or_else(|_| Utc::now());

    Ok(FileItem {
        name: file_name,
        path: path.to_string_lossy().to_string(),
        size: metadata.len(),
        modified,
        is_directory,
        is_hidden,
        is_symlink,
        is_git_repo,
        extension,
        child_count,
        image_width,
        image_height,
        remote_id: None,
        thumbnail_url: None,
        download_url: None,
    })
}

#[derive(Debug, Clone)]
pub struct DiskUsageMetrics {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct DiskUsage {
    pub path: PathBuf,
    pub total_bytes: u64,
    pub available_bytes: u64,
}

pub fn get_disk_usage(path: &Path) -> Result<DiskUsage, String> {
    if !path.exists() {
        return Err("Path does not exist".to_string());
    }

    let metadata = fs::metadata(path).map_err(|e| format!("Failed to retrieve metadata: {}", e))?;

    let effective_path = if metadata.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(|parent| parent.to_path_buf())
            .ok_or_else(|| "Unable to determine parent directory".to_string())?
    };

    #[cfg(target_os = "macos")]
    let _scope_guard = macos_security::retain_access(&effective_path)?;

    let metrics = query_platform_disk_usage(&effective_path)?;

    #[cfg(target_os = "macos")]
    macos_security::persist_bookmark(&effective_path, "disk usage query");

    Ok(DiskUsage {
        path: effective_path,
        total_bytes: metrics.total_bytes,
        available_bytes: metrics.available_bytes,
    })
}

#[cfg(target_os = "windows")]
fn query_platform_disk_usage(path: &Path) -> Result<DiskUsageMetrics, String> {
    let mut wide_path: Vec<u16> = path.as_os_str().encode_wide().collect();
    if !wide_path.ends_with(&[0]) {
        wide_path.push(0);
    }

    let mut free_to_caller: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut total_free: u64 = 0;

    unsafe {
        if let Err(err) = GetDiskFreeSpaceExW(
            PCWSTR(wide_path.as_ptr()),
            Some(&mut free_to_caller),
            Some(&mut total_bytes),
            Some(&mut total_free),
        ) {
            return Err(format!("Failed to query disk usage: {}", err));
        }
    }

    Ok(DiskUsageMetrics {
        total_bytes,
        available_bytes: free_to_caller,
    })
}

#[cfg(target_family = "unix")]
fn query_platform_disk_usage(path: &Path) -> Result<DiskUsageMetrics, String> {
    let bytes = path.as_os_str().as_bytes().to_vec();
    let c_path = CString::new(bytes).map_err(|_| "Path contains null bytes".to_string())?;

    #[cfg(target_os = "macos")]
    unsafe {
        let mut stats: libc::statfs = std::mem::zeroed();
        if libc::statfs(c_path.as_ptr(), &mut stats) != 0 {
            return Err(format!(
                "Failed to query disk usage: {}",
                io::Error::last_os_error()
            ));
        }

        let block_size = stats.f_bsize as u128;
        let total = (stats.f_blocks as u128).saturating_mul(block_size);
        let available = (stats.f_bavail as u128).saturating_mul(block_size);

        return Ok(DiskUsageMetrics {
            total_bytes: total.min(u128::from(u64::MAX)) as u64,
            available_bytes: available.min(u128::from(u64::MAX)) as u64,
        });
    }

    #[cfg(not(target_os = "macos"))]
    unsafe {
        let mut stats: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut stats) != 0 {
            return Err(format!(
                "Failed to query disk usage: {}",
                io::Error::last_os_error()
            ));
        }

        let block_size = if stats.f_frsize > 0 {
            stats.f_frsize as u128
        } else {
            stats.f_bsize as u128
        };

        let total = (stats.f_blocks as u128).saturating_mul(block_size);
        let available = (stats.f_bavail as u128).saturating_mul(block_size);

        Ok(DiskUsageMetrics {
            total_bytes: total.min(u128::from(u64::MAX)) as u64,
            available_bytes: available.min(u128::from(u64::MAX)) as u64,
        })
    }
}

pub fn read_directory_contents(path: &Path) -> Result<Vec<FileItem>, String> {
    #[cfg(target_os = "macos")]
    let _scope_guard = macos_security::retain_access(path)?;

    let entries = fs::read_dir(path).map_err(|e| format!("Failed to read directory: {}", e))?;

    let mut files = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let file_path = entry.path();

        match build_file_item(&file_path) {
            Ok(file_item) => files.push(file_item),
            Err(_) => continue,
        }
    }

    #[cfg(target_os = "macos")]
    macos_security::persist_bookmark(path, "reading directory contents");

    Ok(files)
}

pub fn get_file_info(path: &Path) -> Result<FileItem, String> {
    #[cfg(target_os = "macos")]
    let _scope_guard = macos_security::retain_access(path)?;

    let item = build_file_item(path);

    #[cfg(target_os = "macos")]
    if item.is_ok() {
        macos_security::persist_bookmark(path, "reading file metadata");
    }

    item
}

pub fn create_directory(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let _scope_guard = macos_security::retain_access(path)?;

    fs::create_dir_all(path).map_err(|e| format!("Failed to create directory: {}", e))?;

    #[cfg(target_os = "macos")]
    macos_security::persist_bookmark(path, "creating directory");

    Ok(())
}

pub fn delete_file_or_directory(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let _scope_guard = macos_security::retain_access(path)?;

    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|e| format!("Failed to delete directory: {}", e))
    } else {
        fs::remove_file(path).map_err(|e| format!("Failed to delete file: {}", e))
    }
}

pub fn rename_file_or_directory(from: &Path, to: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let _from_scope = macos_security::retain_access(from)?;
    #[cfg(target_os = "macos")]
    let _to_scope = macos_security::retain_access(to)?;

    fs::rename(from, to).map_err(|e| format!("Failed to rename: {}", e))?;

    #[cfg(target_os = "macos")]
    macos_security::persist_bookmark(to, "renaming");

    Ok(())
}

pub fn copy_file_or_directory(from: &Path, to: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let _from_scope = macos_security::retain_access(from)?;
    #[cfg(target_os = "macos")]
    let _to_scope = macos_security::retain_access(to)?;

    if from.is_dir() {
        let result = copy_dir_recursive(from, to);

        #[cfg(target_os = "macos")]
        if result.is_ok() {
            macos_security::persist_bookmark(to, "copying directory");
        }

        result
    } else {
        let result = fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| format!("Failed to copy file: {}", e));

        #[cfg(target_os = "macos")]
        if result.is_ok() {
            macos_security::persist_bookmark(to, "copying file");
        }

        result
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst)
        .map_err(|e| format!("Failed to create destination directory: {}", e))?;

    for entry in fs::read_dir(src).map_err(|e| format!("Failed to read source directory: {}", e))? {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path).map_err(|e| format!("Failed to copy file: {}", e))?;
        }
    }

    Ok(())
}

pub fn expand_path(path: &str) -> Result<PathBuf, String> {
    if path.starts_with('~') {
        let home =
            dirs::home_dir().ok_or_else(|| "Could not determine home directory".to_string())?;

        if path == "~" {
            Ok(home)
        } else if path.starts_with("~/") {
            Ok(home.join(&path[2..]))
        } else {
            Err("Invalid path format".to_string())
        }
    } else {
        Ok(PathBuf::from(path))
    }
}

/// Batch size for streaming directory reads
const STREAMING_BATCH_SIZE: usize = 100;
/// Batch size for metadata updates (smaller for more responsive updates)
const METADATA_BATCH_SIZE: usize = 50;

/// Read directory contents in streaming fashion with instant UI display.
///
/// Phase 1: Emits skeleton FileItems immediately (just names, no stat calls)
/// Phase 2: Processes metadata in parallel and emits updates via emit_metadata
///
/// Returns the total number of files processed.
pub fn read_directory_streaming<F, M>(
    path: &Path,
    session_id: String,
    cancel_flag: Arc<AtomicBool>,
    mut emit_batch: F,
    mut emit_metadata: M,
) -> Result<u32, String>
where
    F: FnMut(DirectoryBatch) + Send,
    M: FnMut(MetadataBatch) + Send,
{
    #[cfg(target_os = "macos")]
    let _scope_guard = crate::macos_security::retain_access(path)?;

    // Phase 1: Read directory entries and emit skeleton items IMMEDIATELY
    // This uses readdir which is very fast - just reads directory entries without stat
    let dir_entries: Vec<std::fs::DirEntry> = fs::read_dir(path)
        .map_err(|e| format!("Failed to read directory: {}", e))?
        .filter_map(|entry| entry.ok())
        .collect();

    let total_count = dir_entries.len() as u32;

    // Check for cancellation
    if cancel_flag.load(Ordering::Relaxed) {
        return Ok(0);
    }

    // If empty directory, emit a single empty final batch
    if dir_entries.is_empty() {
        emit_batch(DirectoryBatch {
            session_id: session_id.clone(),
            batch_index: 0,
            entries: vec![],
            is_final: true,
            total_count: Some(0),
        });
        emit_metadata(MetadataBatch {
            session_id,
            updates: vec![],
            is_final: true,
        });
        return Ok(0);
    }

    // Build skeleton items (very fast - no stat calls, just uses DirEntry info)
    let skeleton_items: Vec<FileItem> = dir_entries
        .iter()
        .filter_map(build_file_item_skeleton)
        .collect();

    // Collect paths for metadata processing
    let entry_paths: Vec<PathBuf> = dir_entries.iter().map(|e| e.path()).collect();

    // Emit skeleton batches IMMEDIATELY - this is what makes the UI instant
    let mut batch_index = 0u32;
    for chunk in skeleton_items.chunks(STREAMING_BATCH_SIZE) {
        if cancel_flag.load(Ordering::Relaxed) {
            return Ok(0);
        }

        let is_final =
            batch_index as usize * STREAMING_BATCH_SIZE + chunk.len() >= skeleton_items.len();

        emit_batch(DirectoryBatch {
            session_id: session_id.clone(),
            batch_index,
            entries: chunk.to_vec(),
            is_final,
            total_count: if batch_index == 0 {
                Some(total_count)
            } else {
                None
            },
        });

        batch_index += 1;
    }

    // Check for cancellation before metadata phase
    if cancel_flag.load(Ordering::Relaxed) {
        return Ok(total_count);
    }

    // Phase 2: Process metadata in parallel and emit updates
    // This runs after skeletons are displayed, so UI is already responsive
    let metadata_updates: Vec<FileMetadataUpdate> = entry_paths
        .par_iter()
        .filter_map(|entry_path| {
            if cancel_flag.load(Ordering::Relaxed) {
                return None;
            }
            build_file_metadata(entry_path)
        })
        .collect();

    // Emit metadata updates in batches
    let mut meta_batch_index = 0;
    for chunk in metadata_updates.chunks(METADATA_BATCH_SIZE) {
        if cancel_flag.load(Ordering::Relaxed) {
            return Ok(total_count);
        }

        let is_final =
            meta_batch_index * METADATA_BATCH_SIZE + chunk.len() >= metadata_updates.len();

        emit_metadata(MetadataBatch {
            session_id: session_id.clone(),
            updates: chunk.to_vec(),
            is_final,
        });

        meta_batch_index += 1;
    }

    #[cfg(target_os = "macos")]
    crate::macos_security::persist_bookmark(path, "reading directory contents streaming");

    Ok(total_count)
}

/// Generate a unique path for a file in a directory, avoiding collisions.
///
/// If the target path doesn't exist, returns it as-is.
/// Otherwise, appends " (2)", " (3)", etc. up to 999.
/// If all numbered variants are taken, falls back to timestamp + counter suffix.
///
/// # Arguments
/// * `dir` - The directory where the file will be placed
/// * `desired_name` - The desired filename (just the name, not the full path)
///
/// # Returns
/// * `Ok(PathBuf)` - A unique path that doesn't exist
/// * `Err(String)` - If unable to allocate a unique name after all attempts
pub fn allocate_unique_path(dir: &Path, desired_name: &str) -> Result<PathBuf, String> {
    static FALLBACK_COUNTER: AtomicU64 = AtomicU64::new(0);

    // Sanitize the desired name to just the filename component
    let desired_name = Path::new(desired_name)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "Invalid file name".to_string())?;

    let base = dir.join(desired_name);
    if !base.exists() {
        return Ok(base);
    }

    // Extract stem and extension for generating variants
    let stem = Path::new(desired_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(desired_name);
    let ext = Path::new(desired_name).extension().and_then(|e| e.to_str());

    // Try numbered variants: "name (2).ext", "name (3).ext", etc.
    for i in 2..1000usize {
        let candidate = if let Some(e) = ext {
            format!("{stem} ({i}).{e}")
        } else {
            format!("{stem} ({i})")
        };
        let p = dir.join(candidate);
        if !p.exists() {
            return Ok(p);
        }
    }

    // Fallback: use timestamp + atomic counter for guaranteed uniqueness
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    for _ in 0..128u32 {
        let counter = FALLBACK_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = if let Some(e) = ext {
            format!("{stem}_{nanos}_{counter}.{e}")
        } else {
            format!("{stem}_{nanos}_{counter}")
        };
        let p = dir.join(candidate);
        if !p.exists() {
            return Ok(p);
        }
    }

    Err("Unable to allocate unique destination name".to_string())
}
