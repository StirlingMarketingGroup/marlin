use chrono::{DateTime, Utc};
use image::ImageReader;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
#[cfg(target_family = "unix")]
use std::ffi::CString;
use std::fs;
use std::io;
#[cfg(target_family = "unix")]
use std::os::unix::ffi::OsStrExt;
#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

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

    // Type names as /proc/self/mountinfo shows them on real mounts (Linux 7.0, 2026-11-24): `nfs`, `nfs4`, `cifs`, `smb3`, and `fuse.sshfs`. Marlin's mounts are `fuse.marlin`, or `fuse` when mounted as root. `9p` (a VM's shared folder, WSL2's `/mnt/c`), `virtiofs` and `vboxsf` (a VM's shared folder), and the network and cluster filesystems `ceph`, `afs`, `coda`, `lustre`, `beegfs`, `gpfs`, and `pvfs2` (OrangeFS) are the type names their kernel modules register, not checked on a real mount.
    let is_network_or_fuse = matches!(
        fs_type.as_str(),
        "nfs"
            | "nfs4"
            | "cifs"
            | "smb3"
            | "9p"
            | "virtiofs"
            | "vboxsf"
            | "ceph"
            | "afs"
            | "coda"
            | "lustre"
            | "beegfs"
            | "gpfs"
            | "pvfs2"
            | "fuse"
            | "fuseblk"
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
pub(crate) fn device(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.dev()
}

/// Windows has no device ID here, so every entry of a local folder is read inside (#706), a move does not see another volume mounted inside the source, and Move to Trash does not see a volume mounted in the folder.
#[cfg(not(unix))]
pub(crate) fn device(_metadata: &fs::Metadata) -> u64 {
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

/// What a paste, a drop, a merge, or an undo does with one item.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    Copy,
    Move,
}

impl Transfer {
    pub(crate) fn verb(self) -> &'static str {
        match self {
            Transfer::Copy => "copy",
            Transfer::Move => "move",
        }
    }
}

/// Copies or moves `from` to `to` (decision 0009, choice 8). It replaces nothing, but a macOS `._name` file inside a folder it made (see `copy_file`).
///
/// A move inside one filesystem is a rename. A move the OS refuses as crossing filesystems, and every copy, copies `from` entry by entry. A move then deletes exactly the source entries it copied, unchanged, once the whole copy is in place. A failed copy removes what it made and leaves `from` as it was. A copy is always a filesystem copy, inside one mount too (#682).
pub fn transfer(from: &Path, to: &Path, kind: Transfer) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let _from_scope = macos_security::retain_access(from)?;
    #[cfg(target_os = "macos")]
    let _to_scope = macos_security::retain_access(to)?;

    let metadata = fs::symlink_metadata(from)
        .map_err(|err| format!("Failed to read {}: {err}", from.display()))?;
    let name = from
        .file_name()
        .map(|file_name| file_name.to_string_lossy().into_owned())
        .unwrap_or_else(|| from.display().to_string());

    let into_itself = || {
        to.parent()
            .and_then(|parent| fs::canonicalize(parent).ok())
            .zip(fs::canonicalize(from).ok())
            .is_some_and(|(parent, from)| parent.starts_with(from))
    };
    if metadata.is_dir() && into_itself() {
        return Err(format!("Cannot {} {name} into itself", kind.verb()));
    }

    if kind == Transfer::Move {
        match rename_no_replace(from, to) {
            Ok(()) => {
                #[cfg(target_os = "macos")]
                macos_security::persist_bookmark(to, "moving");
                return Ok(());
            }
            // The rename's own answer is the test for one filesystem. On Linux two bind mounts of one filesystem share a device ID and still refuse a rename with EXDEV.
            Err(err) if err.kind() == io::ErrorKind::CrossesDevices => {}
            // A mount answers "not supported" (ENOTSUP, which std maps to `Unsupported` only where it equals EOPNOTSUPP, as on Linux) when its remote cannot make the move. A copy and delete would replace the remote item with a new one, so the move stops here.
            Err(err)
                if err.kind() == io::ErrorKind::Unsupported
                    || cfg!(unix) && err.raw_os_error() == Some(libc::ENOTSUP) =>
            {
                return Err(format!(
                    "Cannot move {name} there: the volume it is on does not allow that move"
                ));
            }
            Err(err) => return Err(format!("Failed to move {name}: {err}")),
        }
    }

    let mut copying = Copying {
        made: Vec::new(),
        move_from_device: (kind == Transfer::Move).then(|| device(&metadata)),
    };
    if let Err(err) = copying.entry(from, to) {
        return Err(match copying.roll_back() {
            Some(removal_error) => format!(
                "{err}; the partial copy at {} could not be fully removed: {removal_error}",
                to.display()
            ),
            None => err,
        });
    }
    copying.finish();
    if kind == Transfer::Move {
        if let Some((path, reason)) = copying.remove_sources() {
            return Err(format!(
                "Moved {name} to {}, but part of the original stayed: {}: {reason}",
                to.display(),
                path.display()
            ));
        }
    }

    #[cfg(target_os = "macos")]
    macos_security::persist_bookmark(
        to,
        if kind == Transfer::Move {
            "moving"
        } else {
            "copying"
        },
    );

    Ok(())
}

/// A filesystem copy of one item, and the entries it has made, top first.
///
/// It fills with `entry`. After an error, `roll_back` removes what it made. Otherwise `finish` gives each entry what it held back while filling: a folder stays owner-only and a file unlocked until then. A move then calls `remove_sources`.
struct Copying {
    made: Vec<Copied>,
    /// For a move, the source's device: see `refuse_other_volume`.
    move_from_device: Option<u64>,
}

/// One entry a copy made under its final name, and the source entry it came from. A folder comes before its entries, so walking the list backwards reaches a folder after them.
struct Copied {
    source: PathBuf,
    destination: PathBuf,
    /// The source as the copy read it.
    metadata: fs::Metadata,
}

impl Copying {
    /// Copies `from` to `to` without following a link, and records each entry it makes.
    fn entry(&mut self, from: &Path, to: &Path) -> Result<(), String> {
        let metadata = fs::symlink_metadata(from)
            .map_err(|err| format!("Failed to copy {}: {err}", from.display()))?;
        let file_type = metadata.file_type();

        if file_type.is_dir() {
            if let Some(top) = self.move_from_device {
                refuse_other_volume(from, &metadata, top)?;
            }
            // Owner-only until `finish`, so a private folder is never open to others while it fills.
            #[cfg(unix)]
            let created = {
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new().mode(0o700).create(to)
            };
            #[cfg(not(unix))]
            let created = fs::create_dir(to);
            created
                .map_err(|err| match err.kind() {
                    io::ErrorKind::AlreadyExists => name_taken(),
                    _ => err,
                })
                .map_err(|err| format!("Failed to copy {}: {err}", to.display()))?;
            self.made.push(Copied {
                source: from.to_path_buf(),
                destination: to.to_path_buf(),
                metadata,
            });

            let entries = fs::read_dir(from)
                .map_err(|err| format!("Failed to copy {}: {err}", from.display()))?;
            for entry in entries {
                let entry =
                    entry.map_err(|err| format!("Failed to copy {}: {err}", from.display()))?;
                self.entry(&entry.path(), &to.join(entry.file_name()))?;
            }
            return Ok(());
        }

        if file_type.is_symlink() {
            copy_symlink(from, to)
                .map_err(|err| format!("Failed to copy {}: {err}", from.display()))?;
        } else if file_type.is_file() {
            // Only the top entry comes before anything is made, so any other is in a folder this copy made.
            copy_file(from, to, !self.made.is_empty())?;
            // `fs::copy` keeps the modified time on macOS and Windows, not on Linux.
            #[cfg(target_os = "linux")]
            copy_modified_time(&metadata, to);
        } else {
            return Err(format!(
                "Cannot copy {}: it is not a file, a folder, or a link",
                from.display()
            ));
        }
        self.made.push(Copied {
            source: from.to_path_buf(),
            destination: to.to_path_buf(),
            metadata,
        });
        Ok(())
    }

    /// Removes what the copy made, newest first, and returns the first failure.
    fn roll_back(&self) -> Option<io::Error> {
        let mut first_failure = None;
        for made in self.made.iter().rev() {
            // Gone already counts as removed: on a volume without native extended attributes, macOS deletes `._name` with `name`.
            let failure = made
                .remove(&made.destination)
                .err()
                .filter(|err| err.kind() != io::ErrorKind::NotFound);
            first_failure = first_failure.or(failure);
        }
        first_failure
    }

    /// Gives each entry what it held back while the copy filled: a folder its extended attributes, permissions, and modified time, and any item its file flags, such as Finder's lock or hidden flag. A read-only folder would have stopped its entries and the roll back, each new entry changes a folder's time, and a locked item takes no other change, so these come last. Inner entries go first, so a parent's mode cannot stop them. A mount answers some of these calls with "not supported", so they only warn.
    fn finish(&self) {
        for made in self.made.iter().rev() {
            if made.metadata.is_dir() {
                #[cfg(target_os = "macos")]
                copy_folder_attributes(&made.source, &made.destination);
                #[cfg(unix)]
                copy_modified_time(&made.metadata, &made.destination);
                if let Err(err) =
                    fs::set_permissions(&made.destination, made.metadata.permissions())
                {
                    log::warn!(
                        "Failed to copy the permissions of {}: {err}",
                        made.destination.display()
                    );
                }
            }
            // Only Finder's lock and hidden flag, added to the flags the copy already has: other flags, such as `UF_COMPRESSED`, describe how the source's own bytes are stored. A file has the hidden flag from `fs::copy`, and the lock that `copy_file` took off; a folder has neither.
            #[cfg(target_os = "macos")]
            {
                use std::os::macos::fs::MetadataExt;
                let flags = made.metadata.st_flags() & (FINDER_LOCK | libc::UF_HIDDEN);
                if flags != 0 && !made.metadata.is_symlink() {
                    let current = fs::symlink_metadata(&made.destination)
                        .map_or(0, |metadata| metadata.st_flags());
                    if let Err(err) = set_flags(&made.destination, current | flags) {
                        log::warn!(
                            "Failed to copy the flags of {}: {err}",
                            made.destination.display()
                        );
                    }
                }
            }
        }
    }

    /// Removes the source entries the copy read, newest first, and returns the first that stayed and why. Only these entries: a file another program added during the copy was never copied, and it stays, and so does a source that changed. The folders above an entry that stays stay too.
    fn remove_sources(&self) -> Option<(&Path, String)> {
        let mut first_kept = None;
        for made in self.made.iter().rev() {
            let kept = if made.source_changed() {
                Some("it changed during the move".to_string())
            } else {
                made.remove(&made.source)
                    .err()
                    .filter(|err| err.kind() != io::ErrorKind::NotFound)
                    .map(|err| err.to_string())
            };
            if let Some(reason) = kept {
                first_kept.get_or_insert((made.source.as_path(), reason));
            }
        }
        first_kept
    }
}

impl Copied {
    /// Removes this entry at `path`, its source or its copy. Windows removes a link to a folder as a folder.
    fn remove(&self, path: &Path) -> io::Result<()> {
        #[cfg(windows)]
        let folder_link =
            std::os::windows::fs::FileTypeExt::is_symlink_dir(&self.metadata.file_type());
        #[cfg(not(windows))]
        let folder_link = false;
        if self.metadata.is_dir() || folder_link {
            fs::remove_dir(path)
        } else {
            fs::remove_file(path)
        }
    }

    /// Whether the source is no longer what the copy read: a file with another length or modified time, or a link with another target. A source that is gone has not changed: on a volume without native extended attributes, macOS deletes `._name` with `name`.
    fn source_changed(&self) -> bool {
        let changed_if_there = |err: io::Error| err.kind() != io::ErrorKind::NotFound;
        if self.metadata.is_symlink() {
            fs::read_link(&self.source).map_or_else(changed_if_there, |target| {
                Some(target) != fs::read_link(&self.destination).ok()
            })
        } else if self.metadata.is_file() {
            fs::symlink_metadata(&self.source).map_or_else(changed_if_there, |now| {
                now.len() != self.metadata.len()
                    || now.modified().ok() != self.metadata.modified().ok()
            })
        } else {
            false
        }
    }
}

/// Stops a move at `folder` when it is on another device than `top`, the device of the folder being moved: a move never empties another drive mounted inside its source, as a delete never does (decision 0007). `transfer` and a merge both ask.
pub(crate) fn refuse_other_volume(
    folder: &Path,
    metadata: &fs::Metadata,
    top: u64,
) -> Result<(), String> {
    if device(metadata) == top {
        Ok(())
    } else {
        Err(format!(
            "{} is on another drive or volume, so it cannot move with its folder",
            folder.display()
        ))
    }
}

/// The error for a name that is taken, worded the same on every filesystem.
fn name_taken() -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        "an item with that name is already there",
    )
}

/// Renames `from` to `to` and never replaces an item at `to`.
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(not(target_os = "windows"))]
    {
        match rustix::fs::renameat_with(
            rustix::fs::CWD,
            from,
            rustix::fs::CWD,
            to,
            rustix::fs::RenameFlags::NOREPLACE,
        ) {
            Ok(()) => return Ok(()),
            Err(rustix::io::Errno::EXIST) => return Err(name_taken()),
            // A filesystem without the flag answers this. macFUSE answers ENOTSUP for RENAME_EXCL onto a free name. Linux answers EINVAL.
            Err(rustix::io::Errno::NOTSUP | rustix::io::Errno::INVAL) => {}
            Err(err) => return Err(err.into()),
        }
    }

    if fs::symlink_metadata(to).is_ok() {
        return Err(name_taken());
    }
    fs::rename(from, to)
}

/// Writes `from` to a temporary name in `to`'s folder, then renames it to `to`. `in_new_folder` says that `to` is in a folder this copy made.
fn copy_file(from: &Path, to: &Path, in_new_folder: bool) -> Result<(), String> {
    // The random name keeps this file apart from others. Opening it with `create_new` first would stop `fs::copy` from cloning on APFS. A dot name is hidden in the views. A mount uploads a new file 2 seconds after its last writer closes, so the rename almost always comes first; when it does not, the mount renames the uploaded item on the remote.
    let temp = to.with_file_name(format!(".marlin-copy-{}", uuid::Uuid::new_v4().simple()));
    if let Err(err) = fs::copy(from, &temp) {
        let _ = fs::remove_file(&temp);
        return Err(format!("Failed to copy {}: {err}", from.display()));
    }

    // `fs::copy` brings Finder's lock along, and a locked file can be neither renamed nor removed. `Copying::finish` locks the copy again once the whole copy is in place.
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        let flags = fs::symlink_metadata(&temp).map_or(0, |metadata| metadata.st_flags());
        if flags & FINDER_LOCK != 0 {
            set_flags(&temp, flags & !FINDER_LOCK)
                .map_err(|err| format!("Failed to copy {}: {err}", from.display()))?;
        }
    }

    // On a volume without native extended attributes, macOS keeps those of `name` in `._name`, and writes that file itself when the copy of `name` gets them. In a folder this copy made, the source's own `._name` holds the same attributes, so it may replace that file. Nothing else is ever replaced: a destination that ignores case can fold two source names into one.
    let attributes_file = in_new_folder
        && cfg!(target_os = "macos")
        && to
            .file_name()
            .is_some_and(|name| name.as_encoded_bytes().starts_with(b"._"));
    let published = if attributes_file {
        fs::rename(&temp, to)
    } else {
        rename_no_replace(&temp, to)
    };
    if let Err(err) = published {
        let _ = fs::remove_file(&temp);
        return Err(format!("Failed to copy {}: {err}", to.display()));
    }
    Ok(())
}

/// Gives `to` the modified time in `metadata`, or logs why not. A read-only handle is enough: setting a time needs ownership, not write access.
#[cfg(unix)]
fn copy_modified_time(metadata: &fs::Metadata, to: &Path) {
    let set = metadata
        .modified()
        .and_then(|modified| fs::File::open(to)?.set_modified(modified));
    if let Err(err) = set {
        log::warn!(
            "Failed to copy the modified time of {}: {err}",
            to.display()
        );
    }
}

/// The flags behind Finder's Locked checkbox: no change and no removal (`uchg`), and append only (`uappnd`).
#[cfg(target_os = "macos")]
const FINDER_LOCK: u32 = libc::UF_IMMUTABLE | libc::UF_APPEND;

/// Sets the file flags of `path` to `flags`, all of them at once.
#[cfg(target_os = "macos")]
fn set_flags(path: &Path, flags: u32) -> io::Result<()> {
    let path = CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: a NUL-terminated path that outlives the call.
    if unsafe { libc::chflags(path.as_ptr(), flags) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Copies a folder's extended attributes, such as Finder tags, and its access list, or logs why not. `fs::copy` does this for a file.
#[cfg(target_os = "macos")]
fn copy_folder_attributes(from: &Path, to: &Path) {
    let (Ok(from_c), Ok(to_c)) = (
        CString::new(from.as_os_str().as_bytes()),
        CString::new(to.as_os_str().as_bytes()),
    ) else {
        return;
    };
    // SAFETY: both paths are NUL-terminated and outlive the call, and copyfile takes a null state.
    let copied = unsafe {
        libc::copyfile(
            from_c.as_ptr(),
            to_c.as_ptr(),
            std::ptr::null_mut(),
            libc::COPYFILE_XATTR | libc::COPYFILE_ACL,
        )
    };
    if copied != 0 {
        log::warn!(
            "Failed to copy the extended attributes of {}: {}",
            to.display(),
            io::Error::last_os_error()
        );
    }
}

fn copy_symlink(src: &Path, dst: &Path) -> io::Result<()> {
    let target = fs::read_link(src)?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, dst)
    }
    #[cfg(windows)]
    {
        // The link's own type: a link to a folder that is gone is still a folder link.
        use std::os::windows::fs::FileTypeExt;
        let folder_link =
            fs::symlink_metadata(src).is_ok_and(|metadata| metadata.file_type().is_symlink_dir());
        if folder_link {
            std::os::windows::fs::symlink_dir(target, dst)
        } else {
            std::os::windows::fs::symlink_file(target, dst)
        }
    }
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
    if fs::symlink_metadata(&base).is_err() {
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
        if fs::symlink_metadata(&p).is_err() {
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
        if fs::symlink_metadata(&p).is_err() {
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
        read_directory_streaming, transfer, FileMetadataUpdate, Transfer,
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

    #[cfg(unix)]
    fn assert_no_marlin_copy_names(dir: &Path) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            assert!(
                !name.to_string_lossy().starts_with(".marlin-copy-"),
                "{}",
                name.to_string_lossy()
            );
            if entry.file_type().unwrap().is_dir() {
                assert_no_marlin_copy_names(&entry.path());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_copied_folder_keeps_its_links_and_modified_times() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir_all(source.join("sub")).unwrap();
        std::fs::write(source.join("a.txt"), b"aaa").unwrap();
        std::fs::write(source.join("sub").join("b.txt"), b"bbb").unwrap();
        std::os::unix::fs::symlink("a.txt", source.join("link")).unwrap();
        std::os::unix::fs::symlink("missing", source.join("gone")).unwrap();
        let past = std::time::UNIX_EPOCH + std::time::Duration::from_secs(981_158_400);
        for folder in [source.join("sub"), source.clone()] {
            std::fs::File::open(&folder)
                .unwrap()
                .set_modified(past)
                .unwrap();
        }

        let dest = temp.path().join("dest");
        transfer(&source, &dest, Transfer::Copy).unwrap();

        for folder in [dest.join("sub"), dest.clone()] {
            assert_eq!(
                std::fs::metadata(&folder).unwrap().modified().unwrap(),
                past
            );
        }

        assert!(std::fs::symlink_metadata(dest.join("link"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_link(dest.join("link")).unwrap(),
            Path::new("a.txt")
        );
        assert!(std::fs::symlink_metadata(dest.join("gone"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_link(dest.join("gone")).unwrap(),
            Path::new("missing")
        );
        assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), b"aaa");
        assert_eq!(
            std::fs::read(dest.join("sub").join("b.txt")).unwrap(),
            b"bbb"
        );
        assert_no_marlin_copy_names(&dest);
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_copy_leaves_no_partial_copy() {
        let parent = tempfile::tempdir_in("/tmp").unwrap();
        let source = parent.path().join("source");
        std::fs::create_dir_all(source.join("sub")).unwrap();
        std::fs::write(source.join("a.txt"), b"a").unwrap();
        std::fs::write(source.join("b.txt"), b"b").unwrap();
        std::fs::write(source.join("sub").join("c.txt"), b"c").unwrap();
        // Most entries are files beside `sub`, so a file is almost always copied before the socket fails.
        for number in 0..16 {
            std::fs::write(source.join(format!("{number}.txt")), b"n").unwrap();
        }
        let _listener =
            std::os::unix::net::UnixListener::bind(source.join("sub").join("socket")).unwrap();

        let err = transfer(&source, &parent.path().join("copy"), Transfer::Copy).unwrap_err();
        assert!(err.contains("sub/socket"), "{err}");

        let names: Vec<_> = std::fs::read_dir(parent.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(names.iter().all(|name| {
            let name = name.to_string_lossy();
            name != "copy" && !name.starts_with(".marlin-copy-")
        }));
        assert_eq!(std::fs::read(source.join("a.txt")).unwrap(), b"a");
        assert_eq!(std::fs::read(source.join("b.txt")).unwrap(), b"b");
        assert_eq!(
            std::fs::read(source.join("sub").join("c.txt")).unwrap(),
            b"c"
        );
        assert!(std::fs::symlink_metadata(source.join("sub").join("socket")).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn a_copied_folder_keeps_its_mode_even_when_read_only() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir_all(source.join("private")).unwrap();
        std::fs::create_dir_all(source.join("read-only")).unwrap();
        std::fs::write(source.join("read-only").join("a.txt"), b"a").unwrap();
        let set_mode = |path: &Path, mode| {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap()
        };
        set_mode(&source.join("private"), 0o750);
        set_mode(&source.join("read-only"), 0o555);

        let dest = temp.path().join("dest");
        let copied = transfer(&source, &dest, Transfer::Copy);
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let modes = (mode(&dest.join("private")), mode(&dest.join("read-only")));
        // Writable again, so the temporary folder can be removed.
        set_mode(&source.join("read-only"), 0o755);
        set_mode(&dest.join("read-only"), 0o755);

        copied.unwrap();
        assert_eq!(modes, (0o750, 0o555));
        assert_eq!(
            std::fs::read(dest.join("read-only").join("a.txt")).unwrap(),
            b"a"
        );
    }

    #[test]
    fn a_transfer_never_replaces_what_is_at_the_destination() {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("from.txt");
        std::fs::write(&from, b"new").unwrap();
        let to = temp.path().join("to.txt");
        std::fs::write(&to, b"old").unwrap();
        for kind in [Transfer::Copy, Transfer::Move] {
            assert!(transfer(&from, &to, kind).is_err());
            assert_eq!(std::fs::read(&to).unwrap(), b"old");
            assert_eq!(std::fs::read(&from).unwrap(), b"new");
        }
        assert!(std::fs::read_dir(temp.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".marlin-copy-")));

        let from_dir = temp.path().join("from-dir");
        std::fs::create_dir(&from_dir).unwrap();
        std::fs::write(from_dir.join("a.txt"), b"a").unwrap();
        let to_dir = temp.path().join("to-dir");
        std::fs::create_dir(&to_dir).unwrap();
        std::fs::write(to_dir.join("keep.txt"), b"keep").unwrap();
        for kind in [Transfer::Copy, Transfer::Move] {
            assert!(transfer(&from_dir, &to_dir, kind).is_err());
            assert_eq!(std::fs::read(to_dir.join("keep.txt")).unwrap(), b"keep");
            assert!(std::fs::symlink_metadata(to_dir.join("a.txt")).is_err());
            assert_eq!(std::fs::read(from_dir.join("a.txt")).unwrap(), b"a");
        }
    }

    #[test]
    fn a_folder_is_not_copied_into_itself() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("folder");
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        std::fs::write(folder.join("a.txt"), b"a").unwrap();

        for kind in [Transfer::Copy, Transfer::Move] {
            let err = transfer(&folder, &folder.join("sub").join("folder"), kind).unwrap_err();
            assert!(err.contains("into itself"), "{err}");
            assert!(std::fs::symlink_metadata(folder.join("sub").join("folder")).is_err());
            assert_eq!(std::fs::read(folder.join("a.txt")).unwrap(), b"a");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_dangling_link_moves_and_copies() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("gone");
        std::os::unix::fs::symlink("missing", &source).unwrap();
        let moved = temp.path().join("moved");

        let (provider, from) = crate::locations::resolve_location(
            crate::locations::LocationInput::Raw(source.to_string_lossy().into_owned()),
        )
        .unwrap();
        let (_, to) = crate::locations::resolve_location(crate::locations::LocationInput::Raw(
            moved.to_string_lossy().into_owned(),
        ))
        .unwrap();
        provider.move_item(&from, &to).await.unwrap();

        assert!(std::fs::symlink_metadata(&source).is_err());
        assert!(std::fs::symlink_metadata(&moved)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_link(&moved).unwrap(), Path::new("missing"));

        let copied = temp.path().join("copied");
        let (provider, from) = crate::locations::resolve_location(
            crate::locations::LocationInput::Raw(moved.to_string_lossy().into_owned()),
        )
        .unwrap();
        let (_, to) = crate::locations::resolve_location(crate::locations::LocationInput::Raw(
            copied.to_string_lossy().into_owned(),
        ))
        .unwrap();
        provider.copy(&from, &to).await.unwrap();

        assert!(std::fs::symlink_metadata(&moved)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(std::fs::symlink_metadata(&copied)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_link(&copied).unwrap(), Path::new("missing"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_locked_file_copies_and_stays_locked() {
        use std::os::macos::fs::MetadataExt;
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("locked.txt"), b"locked").unwrap();
        let locked = |path: &Path| {
            std::fs::symlink_metadata(path).unwrap().st_flags() & super::FINDER_LOCK != 0
        };
        super::set_flags(&source.join("locked.txt"), libc::UF_IMMUTABLE).unwrap();

        let dest = temp.path().join("copy");
        let copied = transfer(&source, &dest, Transfer::Copy);
        let copy_locked = locked(&dest.join("locked.txt"));
        // Unlocked again, so the temporary folder can be removed.
        super::set_flags(&source.join("locked.txt"), 0).unwrap();
        let _ = super::set_flags(&dest.join("locked.txt"), 0);

        copied.unwrap();
        assert!(copy_locked);
        assert_eq!(std::fs::read(dest.join("locked.txt")).unwrap(), b"locked");
        assert_no_marlin_copy_names(&dest);
    }

    #[test]
    fn a_source_changes_when_its_length_time_or_target_change_and_not_when_it_is_gone() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("a.txt");
        let destination = temp.path().join("copy.txt");
        std::fs::write(&source, b"a").unwrap();
        std::fs::write(&destination, b"a").unwrap();
        let copied = super::Copied {
            source: source.clone(),
            destination: destination.clone(),
            metadata: std::fs::symlink_metadata(&source).unwrap(),
        };
        assert!(!copied.source_changed());
        // Same length, new modified time: an editor saved over it.
        let modified = copied.metadata.modified().unwrap();
        std::fs::write(&source, b"b").unwrap();
        let file = std::fs::File::options().write(true).open(&source).unwrap();
        file.set_modified(modified + std::time::Duration::from_secs(1))
            .unwrap();
        assert!(copied.source_changed());
        // New length, same modified time.
        std::fs::write(&source, b"ab").unwrap();
        file.set_modified(modified).unwrap();
        assert!(copied.source_changed());
        drop(file);
        std::fs::remove_file(&source).unwrap();
        assert!(!copied.source_changed());

        #[cfg(unix)]
        {
            let link = temp.path().join("link");
            let link_copy = temp.path().join("link-copy");
            std::os::unix::fs::symlink("a.txt", &link).unwrap();
            std::os::unix::fs::symlink("a.txt", &link_copy).unwrap();
            let copied = super::Copied {
                source: link.clone(),
                destination: link_copy,
                metadata: std::fs::symlink_metadata(&link).unwrap(),
            };
            assert!(!copied.source_changed());
            std::fs::remove_file(&link).unwrap();
            std::os::unix::fs::symlink("b.txt", &link).unwrap();
            assert!(copied.source_changed());
        }
    }

    #[tokio::test]
    async fn a_merge_into_its_own_subfolder_moves_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("Project");
        let archive = project.join("archive");
        std::fs::create_dir_all(archive.join("Project")).unwrap();
        std::fs::write(project.join("a.txt"), b"a").unwrap();
        std::fs::write(archive.join("Project").join("b.txt"), b"b").unwrap();
        let (provider, location) = crate::locations::resolve_location(
            crate::locations::LocationInput::Raw(project.to_string_lossy().into_owned()),
        )
        .unwrap();

        let err = crate::commands::execute_local_conflict_action(
            &crate::commands::ConflictAction::Merge,
            None,
            &project,
            &archive,
            "Project",
            true,
            provider.as_ref(),
            &location,
        )
        .await
        .unwrap_err();

        assert!(err.contains("into itself"), "{err}");
        assert_eq!(std::fs::read(project.join("a.txt")).unwrap(), b"a");
        let mut merged: Vec<_> = std::fs::read_dir(archive.join("Project"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        merged.sort();
        assert_eq!(merged, ["b.txt"]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_copied_folder_keeps_its_extended_attributes() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("tagged");
        std::fs::create_dir(&source).unwrap();
        let xattr = |args: &[&std::ffi::OsStr]| {
            let output = std::process::Command::new("/usr/bin/xattr")
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout).unwrap()
        };
        xattr(&[
            "-w".as_ref(),
            "com.marlin.test".as_ref(),
            "kept".as_ref(),
            source.as_os_str(),
        ]);

        let dest = temp.path().join("copy");
        transfer(&source, &dest, Transfer::Copy).unwrap();

        assert_eq!(
            xattr(&["-p".as_ref(), "com.marlin.test".as_ref(), dest.as_os_str()]).trim(),
            "kept"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_merge_never_follows_a_link_to_a_folder() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let dest = temp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();
        std::os::unix::fs::symlink(&outside, dest.join("sub")).unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir_all(source.join("sub")).unwrap();
        std::fs::write(source.join("sub").join("a.txt"), b"a").unwrap();

        crate::commands::merge_local_directory(&source, &dest, Transfer::Copy).unwrap();

        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        assert!(std::fs::symlink_metadata(dest.join("sub"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read(dest.join("sub (2)").join("a.txt")).unwrap(),
            b"a"
        );
    }

    /// A folder on `/dev/shm` when that is a different filesystem from the normal temp folder.
    #[cfg(target_os = "linux")]
    fn folder_on_another_filesystem() -> Option<tempfile::TempDir> {
        let disk = tempfile::tempdir().ok()?;
        let other = tempfile::tempdir_in("/dev/shm").ok()?;
        use std::os::unix::fs::MetadataExt;
        let disk_dev = std::fs::metadata(disk.path()).ok()?.dev();
        let other_dev = std::fs::metadata(other.path()).ok()?.dev();
        (disk_dev != other_dev).then_some(other)
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_move_to_another_filesystem_copies_then_deletes_the_source() {
        let Some(shm) = folder_on_another_filesystem() else {
            eprintln!("skipped: no second filesystem");
            return;
        };
        let disk = tempfile::tempdir().unwrap();
        let album = shm.path().join("album");
        std::fs::create_dir_all(album.join("sub")).unwrap();
        let note_bytes = b"hello";
        let a_bytes = b"aaa";
        let b_bytes = b"bbb";
        std::fs::write(album.join("a.txt"), a_bytes).unwrap();
        let past = std::time::UNIX_EPOCH + std::time::Duration::from_secs(981158400);
        std::fs::File::open(album.join("a.txt"))
            .unwrap()
            .set_modified(past)
            .unwrap();
        let a_modified = std::fs::metadata(album.join("a.txt"))
            .unwrap()
            .modified()
            .unwrap();
        std::fs::write(album.join("sub").join("b.txt"), b_bytes).unwrap();
        std::os::unix::fs::symlink("a.txt", album.join("link")).unwrap();
        let note = shm.path().join("note.txt");
        std::fs::write(&note, note_bytes).unwrap();

        let disk_album = disk.path().join("album");
        let disk_note = disk.path().join("note.txt");
        transfer(&album, &disk_album, Transfer::Move).unwrap();
        transfer(&note, &disk_note, Transfer::Move).unwrap();

        assert!(std::fs::symlink_metadata(&album).is_err());
        assert!(std::fs::symlink_metadata(&note).is_err());
        assert_eq!(std::fs::read(disk_album.join("a.txt")).unwrap(), a_bytes);
        assert_eq!(
            std::fs::read(disk_album.join("sub").join("b.txt")).unwrap(),
            b_bytes
        );
        assert_eq!(std::fs::read(&disk_note).unwrap(), note_bytes);
        assert!(std::fs::symlink_metadata(disk_album.join("link"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_link(disk_album.join("link")).unwrap(),
            Path::new("a.txt")
        );
        assert_eq!(
            std::fs::metadata(disk_album.join("a.txt"))
                .unwrap()
                .modified()
                .unwrap(),
            a_modified
        );

        crate::commands::move_file(
            crate::locations::LocationInput::Raw(disk_album.to_string_lossy().into_owned()),
            crate::locations::LocationInput::Raw(album.to_string_lossy().into_owned()),
        )
        .await
        .unwrap();

        assert!(std::fs::symlink_metadata(&disk_album).is_err());
        assert_eq!(std::fs::read(album.join("a.txt")).unwrap(), a_bytes);
        assert_eq!(
            std::fs::read(album.join("sub").join("b.txt")).unwrap(),
            b_bytes
        );
        assert_eq!(
            std::fs::read_link(album.join("link")).unwrap(),
            Path::new("a.txt")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_failed_move_to_another_filesystem_leaves_the_source_whole() {
        let Some(shm) = folder_on_another_filesystem() else {
            eprintln!("skipped: no second filesystem");
            return;
        };
        let disk = tempfile::tempdir().unwrap();
        let source = shm.path().join("source");
        std::fs::create_dir_all(source.join("sub")).unwrap();
        std::fs::write(source.join("a.txt"), b"a").unwrap();
        std::fs::write(source.join("sub").join("b.txt"), b"b").unwrap();
        let _listener =
            std::os::unix::net::UnixListener::bind(source.join("sub").join("socket")).unwrap();

        let err = transfer(&source, &disk.path().join("source"), Transfer::Move).unwrap_err();
        assert!(err.contains("sub/socket"), "{err}");
        assert_eq!(std::fs::read(source.join("a.txt")).unwrap(), b"a");
        assert_eq!(
            std::fs::read(source.join("sub").join("b.txt")).unwrap(),
            b"b"
        );
        assert!(std::fs::symlink_metadata(source.join("sub").join("socket")).is_ok());
        assert_eq!(std::fs::read_dir(disk.path()).unwrap().count(), 0);
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

    /// A core with a spool and its uploader running, the same way a writable mount is built.
    #[cfg(target_os = "linux")]
    fn mount_with_spool(
        provider: Arc<dyn marlin_vfs::Provider>,
    ) -> (Arc<marlin_vfs::Vfs>, tempfile::TempDir) {
        let spool = tempfile::tempdir().unwrap();
        let vfs = Arc::new(
            marlin_vfs::Vfs::new(provider, marlin_vfs::POSIX, marlin_vfs::Options::default())
                .take_writes(marlin_vfs::Spool {
                    folder: spool.path().to_path_buf(),
                    limit: 1024 * 1024 * 1024,
                    stays_local: marlin_fuse::stays_local,
                })
                .unwrap(),
        );
        tokio::spawn(Arc::clone(&vfs).run_uploads());
        (vfs, spool)
    }

    #[cfg(target_os = "linux")]
    async fn wait_for_uploads(vfs: &marlin_vfs::Vfs) {
        let started = std::time::Instant::now();
        loop {
            if vfs.unsent().is_empty() {
                return;
            }
            if started.elapsed() >= std::time::Duration::from_secs(30) {
                panic!("{} still unsent", vfs.unsent().len());
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    #[cfg(target_os = "linux")]
    async fn stored_files(memory: &MemoryProvider) -> std::collections::BTreeMap<String, Vec<u8>> {
        let mut files = std::collections::BTreeMap::new();
        let mut pending = vec![(memory.root(), String::new())];
        while let Some((folder, prefix)) = pending.pop() {
            for item in memory.list(&folder).await.unwrap() {
                let path = if prefix.is_empty() {
                    item.name.clone()
                } else {
                    format!("{prefix}/{}", item.name)
                };
                match item.attributes.kind {
                    marlin_vfs::Kind::Folder => pending.push((item.id, path)),
                    marlin_vfs::Kind::File => {
                        files.insert(path, memory.contents(&item.id));
                    }
                }
            }
        }
        files
    }

    #[cfg(target_os = "linux")]
    fn mount_point() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let point = dir.path().join("mnt");
        std::fs::create_dir(&point).unwrap();
        (dir, point)
    }

    #[cfg(target_os = "linux")]
    #[ignore = "mounts for real: run src-tauri/test-in-docker.sh"]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_move_into_a_fuse_mount_leaves_only_final_names() {
        let memory = Arc::new(MemoryProvider::new());
        let (vfs, _spool) = mount_with_spool(memory.clone());
        let (_dir, point) = mount_point();
        let mut mount = marlin_fuse::mount(vfs.clone(), &point).await.unwrap();

        let disk = tempfile::tempdir().unwrap();
        let album = disk.path().join("album");
        std::fs::create_dir_all(album.join("sub")).unwrap();
        std::fs::write(album.join("a.txt"), b"aaa").unwrap();
        std::fs::write(album.join("sub").join("b.txt"), b"bbb").unwrap();
        let destination = point.join("album");

        let album_for_move = album.clone();
        tokio::task::spawn_blocking(move || {
            transfer(&album_for_move, &destination, Transfer::Move).unwrap();
        })
        .await
        .unwrap();

        assert!(std::fs::symlink_metadata(&album).is_err());
        wait_for_uploads(&vfs).await;
        assert_eq!(
            stored_files(&memory).await,
            std::collections::BTreeMap::from([
                ("album/a.txt".to_string(), b"aaa".to_vec()),
                ("album/sub/b.txt".to_string(), b"bbb".to_vec()),
            ])
        );

        mount.unmount().await.unwrap();
    }

    #[cfg(target_os = "linux")]
    #[ignore = "mounts for real: run src-tauri/test-in-docker.sh"]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_move_of_a_folder_that_holds_a_mount_fails_and_keeps_both() {
        let Some(shm) = folder_on_another_filesystem() else {
            eprintln!("skipped: no second filesystem");
            return;
        };
        let memory = Arc::new(MemoryProvider::new());
        memory.add_file(&memory.root(), "remote.txt", b"remote");
        let (vfs, _spool) = mount_with_spool(memory);
        let (dir, point) = mount_point();
        let mut mount = marlin_fuse::mount(vfs, &point).await.unwrap();
        std::fs::write(dir.path().join("local.txt"), b"local").unwrap();

        let source = dir.path().to_path_buf();
        let moved = shm.path().join("moved");
        let moved_for_task = moved.clone();
        let err = tokio::task::spawn_blocking(move || {
            transfer(&source, &moved_for_task, Transfer::Move).unwrap_err()
        })
        .await
        .unwrap();

        assert!(err.contains("another drive or volume"), "{err}");
        assert_eq!(std::fs::read(point.join("remote.txt")).unwrap(), b"remote");
        assert_eq!(
            std::fs::read(dir.path().join("local.txt")).unwrap(),
            b"local"
        );
        assert!(std::fs::symlink_metadata(&moved).is_err());

        mount.unmount().await.unwrap();
    }

    #[cfg(target_os = "linux")]
    #[ignore = "mounts for real: run src-tauri/test-in-docker.sh"]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_merge_move_of_a_folder_that_holds_a_mount_keeps_the_mount() {
        let Some(shm) = folder_on_another_filesystem() else {
            eprintln!("skipped: no second filesystem");
            return;
        };
        let memory = Arc::new(MemoryProvider::new());
        memory.add_file(&memory.root(), "remote.txt", b"remote");
        let (vfs, _spool) = mount_with_spool(memory);
        let (dir, point) = mount_point();
        let mut mount = marlin_fuse::mount(vfs, &point).await.unwrap();

        // Once with no `mnt` in the destination, and once with one, which the merge would recurse into.
        for destination_has_it in [false, true] {
            let dest = shm.path().join(format!("dest-{destination_has_it}"));
            std::fs::create_dir(&dest).unwrap();
            if destination_has_it {
                std::fs::create_dir(dest.join("mnt")).unwrap();
            }
            let source = dir.path().to_path_buf();
            let err = tokio::task::spawn_blocking(move || {
                crate::commands::merge_local_directory(&source, &dest, Transfer::Move).unwrap_err()
            })
            .await
            .unwrap();

            assert!(err.contains("another drive or volume"), "{err}");
            assert_eq!(std::fs::read(point.join("remote.txt")).unwrap(), b"remote");
        }

        mount.unmount().await.unwrap();
    }

    #[cfg(target_os = "linux")]
    #[ignore = "mounts for real: run src-tauri/test-in-docker.sh"]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_folder_with_a_link_moved_into_a_fuse_mount_keeps_the_source() {
        let memory = Arc::new(MemoryProvider::new());
        let (vfs, _spool) = mount_with_spool(memory);
        let (_dir, point) = mount_point();
        let mut mount = marlin_fuse::mount(vfs, &point).await.unwrap();

        let disk = tempfile::tempdir().unwrap();
        let linked = disk.path().join("album");
        std::fs::create_dir(&linked).unwrap();
        std::fs::write(linked.join("a.txt"), b"aaa").unwrap();
        std::os::unix::fs::symlink("a.txt", linked.join("link")).unwrap();
        let destination = point.join("album");

        let linked_for_move = linked.clone();
        let err = tokio::task::spawn_blocking(move || {
            transfer(&linked_for_move, &destination, Transfer::Move).unwrap_err()
        })
        .await
        .unwrap();
        assert!(err.contains("album/link"), "{err}");
        assert_eq!(std::fs::read(linked.join("a.txt")).unwrap(), b"aaa");
        assert_eq!(
            std::fs::read_link(linked.join("link")).unwrap(),
            Path::new("a.txt")
        );
        assert!(std::fs::symlink_metadata(point.join("album")).is_err());

        mount.unmount().await.unwrap();
    }

    #[cfg(target_os = "linux")]
    #[ignore = "mounts for real: run src-tauri/test-in-docker.sh"]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_copy_shows_its_final_name_only_when_whole() {
        let memory = Arc::new(MemoryProvider::new());
        let bytes: Vec<u8> = (0..4 * 1024 * 1024)
            .map(|index| (index % 251) as u8)
            .collect();
        memory.add_file(&memory.root(), "big.bin", &bytes);
        memory.set_delay(std::time::Duration::from_millis(200));
        let vfs = Arc::new(marlin_vfs::Vfs::new(
            memory as Arc<dyn Provider>,
            marlin_vfs::POSIX,
            marlin_vfs::Options::default(),
        ));
        let (_dir, point) = mount_point();
        let mut mount = marlin_fuse::mount(vfs, &point).await.unwrap();

        let disk = tempfile::tempdir().unwrap();
        let source = point.join("big.bin");
        let dest = disk.path().join("big.bin");
        let task = tokio::task::spawn_blocking(move || transfer(&source, &dest, Transfer::Copy));

        let mut saw_temp = false;
        loop {
            if let Ok(entries) = std::fs::read_dir(disk.path()) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if name.starts_with(".marlin-copy-") {
                        saw_temp = true;
                    }
                    if name == "big.bin" {
                        assert!(std::fs::read(entry.path()).unwrap() == bytes);
                    }
                }
            }
            if task.is_finished() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        task.await.unwrap().unwrap();
        assert!(saw_temp);
        assert_eq!(std::fs::read(disk.path().join("big.bin")).unwrap(), bytes);

        mount.unmount().await.unwrap();
    }

    #[cfg(target_os = "linux")]
    #[ignore = "mounts for real: run src-tauri/test-in-docker.sh"]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_move_keeps_a_file_added_to_the_source_during_the_copy() {
        let memory = Arc::new(MemoryProvider::new());
        let album = memory.add_folder(&memory.root(), "album");
        let bytes: Vec<u8> = (0..4 * 1024 * 1024)
            .map(|index| (index % 251) as u8)
            .collect();
        memory.add_file(&album, "big.bin", &bytes);
        memory.set_delay(std::time::Duration::from_millis(200));
        let (vfs, _spool) = mount_with_spool(memory);
        let (_dir, point) = mount_point();
        let mut mount = marlin_fuse::mount(vfs, &point).await.unwrap();

        let disk = tempfile::tempdir().unwrap();
        let source = point.join("album");
        let dest = disk.path().join("album");
        let task = tokio::task::spawn_blocking(move || transfer(&source, &dest, Transfer::Move));

        let mut wrote = false;
        loop {
            if !wrote {
                if let Ok(entries) = std::fs::read_dir(disk.path().join("album")) {
                    let saw_temp = entries.flatten().any(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with(".marlin-copy-")
                    });
                    if saw_temp {
                        let new_file = point.join("album").join("new.txt");
                        tokio::task::spawn_blocking(move || {
                            std::fs::write(new_file, b"new").unwrap()
                        })
                        .await
                        .unwrap();
                        wrote = true;
                    }
                }
            }
            if task.is_finished() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let err = task.await.unwrap().unwrap_err();
        assert!(wrote, "the copy finished before a temporary file was seen");
        assert!(err.contains("stayed"), "{err}");
        assert_eq!(
            std::fs::read(disk.path().join("album").join("big.bin")).unwrap(),
            bytes
        );
        assert_eq!(
            std::fs::read(point.join("album").join("new.txt")).unwrap(),
            b"new"
        );
        assert!(std::fs::symlink_metadata(point.join("album").join("big.bin")).is_err());

        mount.unmount().await.unwrap();
    }
}
