//! Marlin's FUSE mount adapter: serves a marlin-vfs mount on Linux and macOS.

#![cfg(any(target_os = "linux", target_os = "macos"))]

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

use std::path::Path;

use marlin_vfs::{Kind, VfsError};

#[cfg(target_os = "linux")]
pub use linux::{mount, unmount_dead, Error, Mount};
#[cfg(target_os = "macos")]
pub use macos::{mount, unmount_dead, Error, Mount};

/// How long the kernel may reuse a name and its attributes without asking again.
pub(crate) const TTL: std::time::Duration = std::time::Duration::from_secs(1);

pub(crate) const BLOCK_SIZE: u32 = 4096;

/// The longest name apps expect, in bytes. A longer name is not listed and cannot be looked up.
pub(crate) const NAME_MAX: usize = 255;

/// One page of a folder listing. A bigger folder is read in pages, so one readdir never clones the whole listing.
pub(crate) const PAGE: usize = 256;

/// The mode and link count a mount shows. A folder is mode `0o500` with 2 links. A file is mode `0o400` with 1 link.
pub(crate) fn mode_and_links(kind: Kind) -> (u16, u32) {
    match kind {
        Kind::Folder => (0o500, 2),
        Kind::File => (0o400, 1),
    }
}

// Never map a provider error to ENOSYS. The kernel takes it to mean this filesystem has no such call and stops sending that call for the life of the mount. Only the callbacks this adapter leaves out answer it.
pub(crate) fn errno(error: &VfsError) -> libc::c_int {
    match error {
        VfsError::NotFound => libc::ENOENT,
        VfsError::AccessDenied => libc::EACCES,
        VfsError::ReadOnly => libc::EROFS,
        VfsError::Conflict => libc::EEXIST,
        VfsError::NotAFolder => libc::ENOTDIR,
        VfsError::IsAFolder => libc::EISDIR,
        VfsError::NotEmpty => libc::ENOTEMPTY,
        VfsError::NoSpace => libc::ENOSPC,
        VfsError::SignInNeeded => libc::EACCES,
        VfsError::TimedOut => libc::ETIMEDOUT,
        VfsError::Offline => libc::ENETUNREACH,
        VfsError::NotSupported => libc::EOPNOTSUPP,
        VfsError::StaleHandle => libc::ESTALE,
        VfsError::Other(_) => libc::EIO,
    }
}

/// Whether a dead mount is at `mount_point`: one whose server was killed. A live mount, a plain folder, a symbolic link, and a path that is not there are not dead mounts.
///
/// A dead mount fails the open, or the read of the first entry. Linux reports `ENOTCONN`. macOS reports `ENXIO`.
pub async fn is_dead_mount(mount_point: &Path) -> bool {
    let path = mount_point.to_path_buf();
    // The check blocks, and a test serves a live mount from this same process.
    tokio::task::spawn_blocking(move || {
        // `read_dir` follows a symbolic link, so a link is decided here, before that call.
        if std::fs::symlink_metadata(&path).is_ok_and(|found| found.is_symlink()) {
            return false;
        }
        let dead = if cfg!(target_os = "linux") {
            libc::ENOTCONN
        } else {
            libc::ENXIO
        };
        std::fs::read_dir(&path)
            .and_then(|mut entries| entries.next().transpose())
            .is_err_and(|error| error.raw_os_error() == Some(dead))
    })
    .await
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_plain_folder_a_missing_path_and_a_symlink_are_not_dead_mounts() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("plain");
        std::fs::create_dir(&folder).unwrap();
        assert!(!is_dead_mount(&folder).await);
        assert!(!is_dead_mount(&dir.path().join("missing")).await);
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&folder, &link).unwrap();
        assert!(!is_dead_mount(&link).await);
    }
}
