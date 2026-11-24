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

/// What the filesystem that holds a path is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsInfo {
    /// The same number for two paths on one filesystem, and a different one for paths on two.
    pub volume: u64,
    /// The filesystem's type name: `apfs`, `smbfs`, or `macfuse` on macOS; `ext4`, `nfs4`, `cifs`, or `fuse.marlin` on Linux. Empty when Linux's mount table has no line for the path.
    pub fs_type: String,
    /// A network share or a FUSE filesystem, where each call may go to another machine or process. Marlin's own mounts are FUSE.
    pub is_network_or_fuse: bool,
}

#[cfg(target_os = "macos")]
pub fn get_fs_info(path: &Path) -> Result<FsInfo, String> {
    let metadata = fs::metadata(path).map_err(|e| format!("Failed to get metadata: {e}"))?;
    let stats = rustix::fs::statfs(path).map_err(|e| format!("statfs failed: {e}"))?;
    // SAFETY: the kernel ends `f_fstypename` with a NUL inside the array.
    let type_name = unsafe { std::ffi::CStr::from_ptr(stats.f_fstypename.as_ptr()) }
        .to_string_lossy()
        .into_owned();

    // macOS marks a network volume (smbfs, nfs, afpfs, webdav) and a macFUSE volume as not local, and Finder treats a volume that is not local as a network volume. A macFUSE volume mounted with `-o local` keeps the type name `macfuse`. Mount table on macOS 27.2 with macFUSE 5.5.0: a Marlin mount is `(macfuse, nodev, nosuid, noowners, noatime, fskit, ...)` and an SMB share is `(smbfs, nobrowse)`. macOS answers `statfs` on a macFUSE volume from what it learned at mount time, without a call to the mount.
    let is_network_or_fuse = stats.f_flags & libc::MNT_LOCAL as u32 == 0 || type_name == "macfuse";

    Ok(FsInfo {
        volume: device(&metadata),
        fs_type: type_name,
        is_network_or_fuse,
    })
}

#[cfg(target_os = "linux")]
pub fn get_fs_info(path: &Path) -> Result<FsInfo, String> {
    let metadata = fs::metadata(path).map_err(|e| format!("Failed to get metadata: {e}"))?;
    let dev = device(&metadata);
    let major_minor = format!("{}:{}", rustix::fs::major(dev), rustix::fs::minor(dev));

    // On btrfs a subvolume's device ID is not in the mount table, and btrfs is local; a network or FUSE filesystem always has its own line.
    // A mount point's name may be any bytes, so the table is read as bytes: one name that is not UTF-8 must not hide every other line.
    let fs_type = fs::read("/proc/self/mountinfo")
        .ok()
        .and_then(|table| {
            String::from_utf8_lossy(&table).lines().find_map(|line| {
                let (before, after) = line.split_once(" - ")?;
                if before.split(' ').nth(2) != Some(major_minor.as_str()) {
                    return None;
                }
                after.split(' ').next().map(str::to_string)
            })
        })
        .unwrap_or_default();

    // Type names as /proc/self/mountinfo shows them on real mounts (Linux 7.0, 2026-11-24): `nfs`, `nfs4`, `cifs`, `smb3`, and `fuse.sshfs`. Marlin's mounts are `fuse.marlin`, or `fuse` when mounted as root. `9p` (a VM's shared folder, WSL2's `/mnt/c`) and `ceph` are the kernel's type names, not checked on a real mount.
    let is_network_or_fuse = matches!(
        fs_type.as_str(),
        "nfs" | "nfs4" | "cifs" | "smb3" | "9p" | "ceph" | "fuse" | "fuseblk"
    ) || fs_type.starts_with("fuse.");

    Ok(FsInfo {
        volume: dev,
        fs_type,
        is_network_or_fuse,
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn get_fs_info(_path: &Path) -> Result<FsInfo, String> {
    Ok(FsInfo {
        volume: 0,
        fs_type: "unknown".to_string(),
        is_network_or_fuse: false,
    })
}

/// Whether reading inside a path is cheap: its filesystem is local. On a network or FUSE filesystem each read is a call to the remote, so a listing there reads only the folder itself, as Finder does on a network volume, and Marlin runs no `git status` (#677). A path whose filesystem cannot be read counts as remote.
pub fn is_on_local_fs(path: &Path) -> bool {
    get_fs_info(path).is_ok_and(|info| !info.is_network_or_fuse)
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
    "Thumbs.db",                 // Windows thumbnail cache
    "desktop.ini",               // Windows folder settings
    "ehthumbs.db",               // Windows Media Center thumbnails
    "ehthumbs_vista.db",         // Vista Media Center thumbnails
    "$RECYCLE.BIN",              // Windows recycle bin folder
    "System Volume Information", // Windows system folder
];

/// Check if a file should be considered hidden.
/// Returns true for dotfiles and known system files.
pub fn is_hidden_file(name: &str) -> bool {
    name.starts_with('.')
        || HIDDEN_SYSTEM_FILES
            .iter()
            .any(|&f| name.eq_ignore_ascii_case(f))
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
        size: 0,              // Filled in by metadata update
        modified: Utc::now(), // Placeholder, filled in by metadata update
        is_directory,
        is_hidden,
        is_symlink: file_type.is_symlink(),
        is_git_repo: false, // Filled in by metadata update
        extension,
        child_count: None,  // Filled in by metadata update
        image_width: None,  // Filled in by metadata update
        image_height: None, // Filled in by metadata update
        remote_id: None,
        thumbnail_url: None,
        download_url: None,
    })
}

/// What a listing learns by reading inside one item: whether a folder holds a `.git`, how many entries it holds, and an image's size from its header.
#[derive(Default)]
struct InsideItem {
    is_git_repo: bool,
    child_count: Option<u64>,
    image_width: Option<u32>,
    image_height: Option<u32>,
}

/// The device of `folder` when its filesystem is local, which lets a listing of it read inside its entries. `None` on a network or FUSE filesystem, where each read inside an entry is a call to the remote, so a listing there reads only the folder itself, as Finder does on a network volume (#677). An entry on the folder's device shares the folder's answer, so a listing checks the filesystem once. An entry on another device, such as a mount point in `~/Marlin` or `/Volumes`, gets its own check.
fn local_device_of(folder: &Path) -> Option<u64> {
    get_fs_info(folder)
        .ok()
        .filter(|info| !info.is_network_or_fuse)
        .map(|info| info.volume)
}

#[cfg(unix)]
fn device(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.dev()
}

/// Windows has no device ID here, so every entry of a local folder is read inside (#706).
#[cfg(not(unix))]
fn device(_metadata: &fs::Metadata) -> u64 {
    0
}

/// Reads inside one entry of a listing when the entry's filesystem is local: see `local_device_of`.
fn read_inside_item(path: &Path, metadata: &fs::Metadata, local_device: Option<u64>) -> InsideItem {
    let is_local =
        local_device.is_some_and(|folder| device(metadata) == folder || is_on_local_fs(path));
    if !is_local {
        return InsideItem::default();
    }

    if metadata.is_dir() {
        let git_path = path.join(".git");
        return InsideItem {
            is_git_repo: git_path.is_dir() || git_path.is_file(),
            child_count: fs::read_dir(path)
                .ok()
                .map(|entries| entries.count() as u64),
            ..InsideItem::default()
        };
    }

    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|s| s.to_lowercase());

    // This only reads file headers, not the full image data
    let (image_width, image_height) = match extension.as_deref() {
        Some("jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "tga" | "ico") => {
            ImageReader::open(path)
                .ok()
                .and_then(|reader| reader.into_dimensions().ok())
                .map(|(w, h)| (Some(w), Some(h)))
                .unwrap_or((None, None))
        }
        _ => (None, None),
    };

    InsideItem {
        image_width,
        image_height,
        ..InsideItem::default()
    }
}

/// Build full metadata for a file (called in background after skeleton is displayed)
fn build_file_metadata(path: &Path, local_device: Option<u64>) -> Option<FileMetadataUpdate> {
    let symlink_metadata = fs::symlink_metadata(path).ok()?;
    let is_symlink = symlink_metadata.file_type().is_symlink();

    let target_metadata = if is_symlink {
        fs::metadata(path).ok()
    } else {
        None
    };

    let metadata = target_metadata.as_ref().unwrap_or(&symlink_metadata);
    let is_directory = metadata.is_dir();
    let inside = read_inside_item(path, metadata, local_device);

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
        is_git_repo: inside.is_git_repo,
        child_count: inside.child_count,
        image_width: inside.image_width,
        image_height: inside.image_height,
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

fn build_file_item(path: &Path, local_device: Option<u64>) -> Result<FileItem, String> {
    let symlink_metadata =
        fs::symlink_metadata(path).map_err(|e| format!("Failed to get metadata: {}", e))?;

    let is_symlink = symlink_metadata.file_type().is_symlink();
    let target_metadata = if is_symlink {
        fs::metadata(path).ok()
    } else {
        None
    };

    let metadata = target_metadata.as_ref().unwrap_or(&symlink_metadata);
    let is_directory = metadata.is_dir();

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

    let inside = read_inside_item(path, metadata, local_device);

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
        is_git_repo: inside.is_git_repo,
        extension,
        child_count: inside.child_count,
        image_width: inside.image_width,
        image_height: inside.image_height,
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
    let local_device = local_device_of(path);

    let mut files = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let file_path = entry.path();

        match build_file_item(&file_path, local_device) {
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

    let item = build_file_item(path, local_device_of(path));

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

    crate::delete_entry::delete_entry(path).map_err(|e| {
        let name = path.file_name().unwrap_or(path.as_os_str());
        format!("Failed to delete {}: {}", name.to_string_lossy(), e)
    })
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

/// Reads a folder in two steps and sends each step as one batch: first the names and types,
/// then the metadata of each entry it can read. Every batch makes the page sort the whole
/// list again, so a step split into smaller batches makes the time a folder takes to show
/// grow with the square of its size (#654).
pub fn read_directory_streaming<F, M>(
    path: &Path,
    session_id: String,
    cancel_flag: Arc<AtomicBool>,
    emit_batch: F,
    emit_metadata: M,
) -> Result<(), String>
where
    F: FnOnce(DirectoryBatch) + Send,
    M: FnOnce(MetadataBatch) + Send,
{
    #[cfg(target_os = "macos")]
    let _scope_guard = crate::macos_security::retain_access(path)?;

    let dir_entries: Vec<std::fs::DirEntry> = fs::read_dir(path)
        .map_err(|e| format!("Failed to read directory: {}", e))?
        .filter_map(|entry| entry.ok())
        .collect();

    let skeleton_items: Vec<FileItem> = dir_entries
        .iter()
        .take_while(|_| !cancel_flag.load(Ordering::Relaxed))
        .filter_map(build_file_item_skeleton)
        .collect();

    if cancel_flag.load(Ordering::Relaxed) {
        return Ok(());
    }

    emit_batch(DirectoryBatch {
        session_id: session_id.clone(),
        batch_index: 0,
        entries: skeleton_items,
        is_final: true,
        total_count: Some(dir_entries.len() as u32),
    });

    let entry_paths: Vec<PathBuf> = dir_entries.iter().map(|e| e.path()).collect();
    let local_device = local_device_of(path);
    let updates: Vec<FileMetadataUpdate> = entry_paths
        .par_iter()
        .filter_map(|entry_path| {
            if cancel_flag.load(Ordering::Relaxed) {
                return None;
            }
            build_file_metadata(entry_path, local_device)
        })
        .collect();

    if cancel_flag.load(Ordering::Relaxed) {
        return Ok(());
    }

    emit_metadata(MetadataBatch {
        session_id,
        updates,
        is_final: true,
    });

    #[cfg(target_os = "macos")]
    crate::macos_security::persist_bookmark(path, "reading directory contents streaming");

    Ok(())
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

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use super::{
        build_file_item, build_file_metadata, delete_file_or_directory, local_device_of,
        read_directory_streaming, FileMetadataUpdate,
    };

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    use super::get_fs_info;

    #[cfg(target_os = "linux")]
    use std::sync::Mutex;

    #[cfg(target_os = "linux")]
    use marlin_vfs::{Item, ItemId, MemoryProvider, Provider, Space, VfsError};

    /// The metadata batch of one listing of `folder`.
    fn listed_metadata(folder: &Path) -> Vec<FileMetadataUpdate> {
        let mut metadata = None;
        read_directory_streaming(
            folder,
            "session".to_string(),
            Arc::new(AtomicBool::new(false)),
            |_| {},
            |batch| metadata = Some(batch),
        )
        .unwrap();
        metadata.unwrap().updates
    }

    fn folder_with_a_repo_and_a_photo() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join("a.txt"), b"a").unwrap();
        image::RgbImage::new(3, 2)
            .save(temp.path().join("photo.png"))
            .unwrap();
        temp
    }

    #[test]
    fn a_failed_delete_names_the_selected_entry() {
        let temp = tempfile::tempdir().unwrap();

        let err = delete_file_or_directory(&temp.path().join("missing")).unwrap_err();

        assert!(err.starts_with("Failed to delete missing: "), "{err}");
    }

    #[test]
    fn an_entry_is_read_inside_only_on_the_local_device_given() {
        let temp = folder_with_a_repo_and_a_photo();
        let repo = temp.path().join("repo");
        let photo = temp.path().join("photo.png");
        let photo_size = std::fs::metadata(&photo).unwrap().len();
        let local = local_device_of(temp.path());
        assert!(local.is_some());

        let metadata = build_file_metadata(&repo, local).unwrap();
        assert!(metadata.is_git_repo);
        assert_eq!(metadata.child_count, Some(2));

        let metadata = build_file_metadata(&photo, local).unwrap();
        assert_eq!(metadata.image_width, Some(3));
        assert_eq!(metadata.image_height, Some(2));

        let metadata = build_file_metadata(&repo, None).unwrap();
        assert!(!metadata.is_git_repo);
        assert_eq!(metadata.child_count, None);
        assert_eq!(metadata.image_width, None);
        assert_eq!(metadata.image_height, None);
        assert!(metadata.is_directory);

        let metadata = build_file_metadata(&photo, None).unwrap();
        assert!(!metadata.is_git_repo);
        assert_eq!(metadata.child_count, None);
        assert_eq!(metadata.image_width, None);
        assert_eq!(metadata.image_height, None);
        assert!(!metadata.is_directory);
        assert_eq!(metadata.size, photo_size);

        let item = build_file_item(&repo, local).unwrap();
        assert!(item.is_git_repo);
        assert_eq!(item.child_count, Some(2));

        let item = build_file_item(&photo, local).unwrap();
        assert_eq!(item.image_width, Some(3));
        assert_eq!(item.image_height, Some(2));

        let item = build_file_item(&repo, None).unwrap();
        assert!(!item.is_git_repo);
        assert_eq!(item.child_count, None);
        assert_eq!(item.image_width, None);
        assert_eq!(item.image_height, None);
        assert!(item.is_directory);

        let item = build_file_item(&photo, None).unwrap();
        assert!(!item.is_git_repo);
        assert_eq!(item.child_count, None);
        assert_eq!(item.image_width, None);
        assert_eq!(item.image_height, None);
        assert!(!item.is_directory);
        assert_eq!(item.size, photo_size);
    }

    #[test]
    fn a_local_listing_reads_inside_its_local_entries() {
        let temp = folder_with_a_repo_and_a_photo();
        // A link to a folder on another local device gets its own check, as a mount point does.
        #[cfg(unix)]
        std::os::unix::fs::symlink("/dev", temp.path().join("other-device")).unwrap();

        let updates = listed_metadata(temp.path());
        let entry = |name: &str| {
            let path = temp.path().join(name).to_string_lossy().to_string();
            updates.iter().find(|update| update.path == path).unwrap()
        };
        assert_eq!(entry("repo").child_count, Some(2));
        assert!(entry("repo").is_git_repo);
        assert_eq!(entry("photo.png").image_width, Some(3));
        assert_eq!(entry("photo.png").image_height, Some(2));
        #[cfg(unix)]
        assert!(entry("other-device").child_count.is_some());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn get_fs_info_on_a_temp_folder_is_local_and_proc_is_another_volume() {
        use std::os::unix::fs::MetadataExt;

        let temp = tempfile::tempdir().unwrap();
        let info = get_fs_info(temp.path()).unwrap();
        assert_eq!(info.volume, std::fs::metadata(temp.path()).unwrap().dev());
        assert!(!info.is_network_or_fuse);

        let proc_info = get_fs_info(std::path::Path::new("/proc")).unwrap();
        assert_eq!(proc_info.volume, std::fs::metadata("/proc").unwrap().dev());
        assert_ne!(proc_info.volume, info.volume);
        assert_eq!(proc_info.fs_type, "proc");
        assert!(!proc_info.is_network_or_fuse);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn get_fs_info_shares_a_volume_inside_one_folder_and_splits_devfs() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("note.txt");
        std::fs::write(&file, b"note").unwrap();

        let folder = get_fs_info(temp.path()).unwrap();
        let file = get_fs_info(&file).unwrap();
        assert_eq!(folder.volume, file.volume);
        assert!(!folder.is_network_or_fuse);
        assert!(!file.is_network_or_fuse);

        let dev = get_fs_info(std::path::Path::new("/dev")).unwrap();
        assert_ne!(dev.volume, folder.volume);
        assert_eq!(dev.fs_type, "devfs");
        assert!(!dev.is_network_or_fuse);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_drop_between_two_filesystems_copies_by_default_and_cmd_makes_it_a_move() {
        let source_dir = tempfile::tempdir_in("/dev/shm").unwrap();
        let source = source_dir.path().join("file.txt");
        std::fs::write(&source, b"x").unwrap();
        let destination = tempfile::tempdir().unwrap();
        let source = source.to_str().unwrap().to_string();
        let destination_path = destination.path().to_str().unwrap().to_string();

        let copied = crate::commands::resolve_drop_operation(
            vec![source.clone()],
            destination_path.clone(),
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(copied.operation, "copy");

        let moved = crate::commands::resolve_drop_operation(
            vec![source],
            destination_path.clone(),
            false,
            true,
        )
        .await
        .unwrap();
        assert_eq!(moved.operation, "move");

        let same_dir = tempfile::tempdir().unwrap();
        let same = same_dir.path().join("file.txt");
        std::fs::write(&same, b"x").unwrap();
        let same = crate::commands::resolve_drop_operation(
            vec![same.to_str().unwrap().to_string()],
            destination_path,
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(same.operation, "move");
    }

    /// Records each folder the mount lists and each file it reads, then answers from memory.
    #[cfg(target_os = "linux")]
    struct RecordingProvider {
        memory: Arc<MemoryProvider>,
        listed: Mutex<Vec<ItemId>>,
        reads: Mutex<Vec<ItemId>>,
        spaces: std::sync::atomic::AtomicUsize,
    }

    #[cfg(target_os = "linux")]
    #[async_trait::async_trait]
    impl Provider for RecordingProvider {
        fn root(&self) -> ItemId {
            self.memory.root()
        }

        async fn item(&self, id: &ItemId) -> Result<Item, VfsError> {
            self.memory.item(id).await
        }

        async fn list(&self, folder: &ItemId) -> Result<Vec<Item>, VfsError> {
            self.listed.lock().unwrap().push(folder.clone());
            self.memory.list(folder).await
        }

        async fn read(&self, id: &ItemId, offset: u64, length: u32) -> Result<Vec<u8>, VfsError> {
            self.reads.lock().unwrap().push(id.clone());
            self.memory.read(id, offset, length).await
        }

        async fn space(&self) -> Result<Space, VfsError> {
            self.spaces
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.memory.space().await
        }
    }

    #[cfg(target_os = "linux")]
    #[ignore = "mounts for real: run src-tauri/test-in-docker.sh"]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_fuse_folder_lists_without_reading_inside_items() {
        let memory = Arc::new(MemoryProvider::new());
        let folders: Vec<ItemId> = (0..200)
            .map(|number| {
                let folder = memory.add_folder(&memory.root(), &format!("folder-{number:03}"));
                memory.add_file(&folder, "inside.txt", b"inside");
                folder
            })
            .collect();
        let mut png = Vec::new();
        image::RgbImage::new(3, 2)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        memory.add_file(&memory.root(), "photo.png", &png);
        let repo = memory.add_folder(&memory.root(), "repo");
        let git = memory.add_folder(&repo, ".git");
        memory.add_file(&git, "HEAD", b"ref: refs/heads/main\n");
        memory.add_folder(&git, "objects");
        memory.add_folder(&git, "refs");
        memory.add_file(&repo, "README.md", b"");

        let recorder = Arc::new(RecordingProvider {
            memory,
            listed: Mutex::new(Vec::new()),
            reads: Mutex::new(Vec::new()),
            spaces: std::sync::atomic::AtomicUsize::new(0),
        });
        let vfs = Arc::new(marlin_vfs::Vfs::new(
            recorder.clone() as Arc<dyn Provider>,
            marlin_vfs::POSIX,
            marlin_vfs::Options::default(),
        ));
        let dir = tempfile::tempdir().unwrap();
        let point = dir.path().join("mnt");
        std::fs::create_dir(&point).unwrap();
        let mut mount = marlin_fuse::mount(vfs, &point).await.unwrap();

        // Telling a FUSE mount apart must not ask the mount anything: `statfs` there asks the provider for its free space.
        let spaces = recorder.spaces.load(std::sync::atomic::Ordering::Relaxed);
        let host = get_fs_info(dir.path()).unwrap();
        let mounted = get_fs_info(&point).unwrap();
        assert_eq!(mounted.fs_type, "fuse.marlin");
        assert!(mounted.is_network_or_fuse);
        assert_ne!(mounted.volume, host.volume);

        recorder.listed.lock().unwrap().clear();
        recorder.reads.lock().unwrap().clear();

        let point_for_list = point.clone();
        let updates = tokio::task::spawn_blocking(move || listed_metadata(&point_for_list))
            .await
            .unwrap();

        let listed = recorder.listed.lock().unwrap().clone();
        let root = recorder.root();
        assert!(listed.iter().all(|id| id == &root), "listed: {listed:?}");
        assert!(recorder.reads.lock().unwrap().is_empty());
        assert_eq!(
            recorder.spaces.load(std::sync::atomic::Ordering::Relaxed),
            spaces
        );

        assert_eq!(updates.len(), 202);
        assert!(updates.iter().all(|update| update.child_count.is_none()));
        let repo_path = point.join("repo");
        let repo_update = updates
            .iter()
            .find(|update| update.path == repo_path.to_string_lossy().to_string())
            .unwrap();
        assert!(!repo_update.is_git_repo);
        let photo = updates
            .iter()
            .find(|update| update.path.ends_with("/photo.png"))
            .unwrap();
        assert_eq!(photo.image_width, None);
        assert_eq!(photo.image_height, None);

        let repo_for_git = repo_path.clone();
        tokio::task::spawn_blocking(move || {
            git2::Repository::discover(&repo_for_git).unwrap();
        })
        .await
        .unwrap();
        let status = crate::commands::get_git_status(repo_path.to_str().unwrap().to_string())
            .await
            .unwrap();
        assert!(status.is_none());

        // The local folder that holds the mount point lists it without reading inside it.
        recorder.listed.lock().unwrap().clear();
        let parent = dir.path().to_path_buf();
        let updates = tokio::task::spawn_blocking(move || listed_metadata(&parent))
            .await
            .unwrap();
        let mount_point = updates
            .iter()
            .find(|update| update.path == point.to_string_lossy())
            .unwrap();
        assert!(mount_point.is_directory);
        assert_eq!(mount_point.child_count, None);
        let listed = recorder.listed.lock().unwrap().clone();
        assert!(listed.is_empty(), "listed: {listed:?}");

        // A read inside one subfolder does reach the provider, so the checks above would see one.
        let folder = point.join("folder-000");
        tokio::task::spawn_blocking(move || {
            let device = super::device(&std::fs::metadata(&folder).unwrap());
            build_file_metadata(&folder, Some(device)).unwrap();
        })
        .await
        .unwrap();
        assert!(recorder.listed.lock().unwrap().contains(&folders[0]));

        mount.unmount().await.unwrap();
    }
}
