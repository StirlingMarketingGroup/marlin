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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg(target_family = "unix")]
use std::io;

#[cfg(target_os = "windows")]
use windows::core::PCWSTR;
#[cfg(target_os = "windows")]
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

#[cfg(target_os = "macos")]
use crate::macos_security;

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

    let is_hidden = file_name.starts_with('.');

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

/// Build a FileItem from a path, used by streaming directory reads.
/// Includes child_count for directories and image dimensions for supported formats.
fn build_file_item_fast(path: &Path) -> Result<FileItem, String> {
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

    let is_hidden = file_name.starts_with('.');

    let extension = if metadata.is_file() {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|s| s.to_lowercase())
    } else {
        None
    };

    // Git repo detection is fast enough to keep
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
    })
}

/// Batch size for streaming directory reads
const STREAMING_BATCH_SIZE: usize = 100;

/// Read directory contents in streaming fashion using parallel processing.
/// Returns batches via the callback, which should emit events to the frontend.
/// Returns the total number of files processed.
pub fn read_directory_streaming<F>(
    path: &Path,
    session_id: String,
    cancel_flag: Arc<AtomicBool>,
    mut emit_batch: F,
) -> Result<u32, String>
where
    F: FnMut(DirectoryBatch) + Send,
{
    #[cfg(target_os = "macos")]
    let _scope_guard = crate::macos_security::retain_access(path)?;

    // First pass: collect all entry paths (fast - just readdir, no stat)
    let entries: Vec<PathBuf> = fs::read_dir(path)
        .map_err(|e| format!("Failed to read directory: {}", e))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();

    let total_count = entries.len() as u32;

    // Check for cancellation
    if cancel_flag.load(Ordering::Relaxed) {
        return Ok(0);
    }

    // If empty directory, emit a single empty final batch
    if entries.is_empty() {
        emit_batch(DirectoryBatch {
            session_id,
            batch_index: 0,
            entries: vec![],
            is_final: true,
            total_count: Some(0),
        });
        return Ok(0);
    }

    // Process entries in parallel using rayon
    // We'll collect results and emit in batches
    let file_items: Vec<FileItem> = entries
        .par_iter()
        .filter_map(|entry_path| {
            // Check cancellation periodically
            if cancel_flag.load(Ordering::Relaxed) {
                return None;
            }
            build_file_item_fast(entry_path).ok()
        })
        .collect();

    // Check for cancellation after processing
    if cancel_flag.load(Ordering::Relaxed) {
        return Ok(0);
    }

    // Emit results in batches
    let mut batch_index = 0u32;
    for chunk in file_items.chunks(STREAMING_BATCH_SIZE) {
        if cancel_flag.load(Ordering::Relaxed) {
            return Ok(0);
        }

        let is_final = batch_index as usize * STREAMING_BATCH_SIZE + chunk.len() >= file_items.len();

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

    #[cfg(target_os = "macos")]
    crate::macos_security::persist_bookmark(path, "reading directory contents streaming");

    Ok(total_count)
}
