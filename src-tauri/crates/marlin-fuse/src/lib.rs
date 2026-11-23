//! Marlin's FUSE mount adapter: serves a marlin-vfs mount on Linux and macOS.

#![cfg(any(target_os = "linux", target_os = "macos"))]

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

use std::path::Path;
use std::time::SystemTime;

use marlin_vfs::{Access, Attributes, Kind, NodeId, Vfs, VfsError};

#[cfg(target_os = "linux")]
pub use linux::{mount, unmount_dead, Error, Mount};
#[cfg(target_os = "macos")]
pub use macos::{mount, unmount_dead, Error, Mount};

/// How long the kernel may reuse a name and its attributes without asking again.
pub(crate) const TTL: std::time::Duration = std::time::Duration::from_secs(1);

pub(crate) const BLOCK_SIZE: u32 = 4096;

/// The longest name apps expect, in bytes. A longer name is not listed and cannot be looked up.
pub(crate) const NAME_MAX: usize = 255;

/// The most entries one readdir reply copies out of a frozen listing with `Vfs::read_folder`.
pub(crate) const PAGE: usize = 256;

/// Names this OS writes for itself, which a mount keeps on this computer and never uploads (decision 0005, choice 14). Pass it as `Spool::stays_local`.
pub fn stays_local(name: &str) -> bool {
    cfg!(target_os = "macos") && marlin_vfs::finder_files(name)
}

/// How the mount shows every node: the mounting user owns it, and its mode follows whether the mount takes writes and whether the provider marks it read-only.
#[derive(Clone, Copy)]
pub(crate) struct Shown {
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    /// Set from `Vfs::writable` before the mount.
    pub(crate) writable: bool,
}

/// An existing name must already be in the folder. A new one is about to be made.
pub(crate) enum Name {
    Existing,
    New,
}

/// The name as the core takes it. A name over 255 bytes is `ENAMETOOLONG`. A name that is not UTF-8 is `ENOENT` for [`Name::Existing`], because the mount lists no such name, and `EINVAL` for [`Name::New`], because the remote cannot hold it.
pub(crate) fn utf8_name(name: &[u8], role: Name) -> Result<String, libc::c_int> {
    if name.len() > NAME_MAX {
        return Err(libc::ENAMETOOLONG);
    }
    match std::str::from_utf8(name) {
        Ok(name) => Ok(name.to_owned()),
        Err(_) => Err(match role {
            Name::Existing => libc::ENOENT,
            Name::New => libc::EINVAL,
        }),
    }
}

/// What the core opens a file for: reading for a read-only open, writing for any other.
pub(crate) fn open_access(flags: i32) -> Access {
    if flags & libc::O_ACCMODE == libc::O_RDONLY {
        Access::Read
    } else {
        Access::Write
    }
}

/// Sets a size and a modified time, then reads the attributes back.
///
/// The size goes first. `cp -p` writes, then sets the time, and a cut after the time would replace it. An owner or a mode change succeeds and changes nothing. No access time is kept.
pub(crate) async fn set_attributes(
    vfs: &Vfs,
    node: NodeId,
    size: Option<u64>,
    modified: Option<SystemTime>,
) -> Result<Attributes, VfsError> {
    if let Some(size) = size {
        vfs.truncate(node, size).await?;
    }
    if let Some(time) = modified {
        vfs.set_modified(node, time).await?;
    }
    vfs.attributes(node).await
}

impl Shown {
    /// The mode and link count of a node. A node has write permission when the mount takes writes and the provider has not marked it read-only: a folder `0o700` or `0o500`, a file `0o600` or `0o400`. A folder has 2 links. A file has 1.
    pub(crate) fn mode_and_links(self, attributes: &Attributes) -> (u16, u32) {
        let writable = self.writable && !attributes.read_only;
        match (attributes.kind, writable) {
            (Kind::Folder, true) => (0o700, 2),
            (Kind::Folder, false) => (0o500, 2),
            (Kind::File, true) => (0o600, 1),
            (Kind::File, false) => (0o400, 1),
        }
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
        VfsError::NotSupported => libc::ENOTSUP,
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

    #[test]
    fn utf8_name_rejects_a_long_name_before_it_checks_the_bytes() {
        let long = vec![0xff; 256];
        assert_eq!(utf8_name(&long, Name::Existing), Err(libc::ENAMETOOLONG));
        assert_eq!(utf8_name(&long, Name::New), Err(libc::ENAMETOOLONG));
        let bad = [0xff];
        assert_eq!(utf8_name(&bad, Name::Existing), Err(libc::ENOENT));
        assert_eq!(utf8_name(&bad, Name::New), Err(libc::EINVAL));
        let ok = "a".repeat(255);
        assert_eq!(utf8_name(ok.as_bytes(), Name::Existing).unwrap(), ok);
    }
}
