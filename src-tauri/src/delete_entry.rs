//! Deletes a selected entry. On Windows an error does not name the child that failed,
//! because the `remove_dir_all` crate does not return it. Decision 0007 says why the code
//! is shaped this way.

use std::io;
use std::path::Path;
use std::sync::LazyLock;

/// On APFS, ext4 and NTFS, 4 threads delete 2 to 3 times faster than one, and on APFS 16 threads are slower than one (#48).
const THREADS: usize = 4;

/// One pool for every delete, so two deletes at once still run `THREADS` threads.
static POOL: LazyLock<Option<rayon::ThreadPool>> = LazyLock::new(|| {
    rayon::ThreadPoolBuilder::new()
        .num_threads(THREADS)
        .thread_name(|i| format!("delete-{i}"))
        .build()
        .inspect_err(|err| log::warn!("Failed to start the delete threads: {err}"))
        .ok()
});

/// Deletes the entry at `path`: a folder with everything in it, on `THREADS` threads, or a
/// link or a file alone. A link is never followed. On macOS and Linux, a folder on another
/// drive or volume than its parent is left in place.
pub fn delete_entry(path: &Path) -> io::Result<()> {
    // `folder/..`, a drive root, and a bare relative name do not name an entry in a known
    // folder. `link/` names the link.
    let (Some(parent), Some(name)) = (
        path.parent()
            .filter(|parent| !parent.as_os_str().is_empty()),
        path.file_name(),
    ) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Cannot delete this path",
        ));
    };

    let delete = || {
        #[cfg(not(target_os = "windows"))]
        {
            unix::delete(parent, name)
        }
        #[cfg(target_os = "windows")]
        {
            windows::delete(parent, name)
        }
    };
    // Without the pool, rayon's global pool runs the delete.
    match POOL.as_ref() {
        Some(pool) => pool.install(delete),
        None => delete(),
    }
}

// Not the `remove_dir_all` crate: on Unix it canonicalizes the path, which follows a
// selected link to its target, and it runs on one thread on macOS.
#[cfg(not(target_os = "windows"))]
mod unix {
    use rayon::prelude::*;
    use rustix::fs::{
        fstat, open, openat, statat, unlinkat, AtFlags, Dev, Dir, FileType, Mode, OFlags,
    };
    use rustix::io::{fcntl_dupfd_cloexec, Errno};
    use std::ffi::{CStr, CString, OsStr};
    use std::io;
    use std::os::fd::OwnedFd;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    const FOLDER: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);

    /// What could not be deleted, relative to the selected entry (empty for the entry
    /// itself), and why.
    struct Failure {
        path: PathBuf,
        cause: Cause,
    }

    enum Cause {
        Os(Errno),
        /// A folder on another drive or volume than its parent, left in place.
        OtherVolume,
    }

    impl From<Errno> for Failure {
        fn from(errno: Errno) -> Self {
            Failure {
                path: PathBuf::new(),
                cause: Cause::Os(errno),
            }
        }
    }

    impl Failure {
        /// Puts `name` in front of the path, as the failure leaves that folder.
        fn in_child(mut self, name: &CStr) -> Self {
            let name = Path::new(OsStr::from_bytes(name.to_bytes()));
            self.path = if self.path.as_os_str().is_empty() {
                name.to_path_buf()
            } else {
                name.join(&self.path)
            };
            self
        }
    }

    impl From<Failure> for io::Error {
        fn from(failure: Failure) -> Self {
            let (kind, cause) = match failure.cause {
                Cause::Os(errno) => {
                    let error = io::Error::from(errno);
                    (error.kind(), error.to_string())
                }
                Cause::OtherVolume => (
                    io::ErrorKind::CrossesDevices,
                    "it is on another drive or volume, so it was left in place".to_string(),
                ),
            };
            if failure.path.as_os_str().is_empty() {
                io::Error::new(kind, cause)
            } else {
                io::Error::new(kind, format!("{}: {cause}", failure.path.display()))
            }
        }
    }

    /// Holds one open folder per level in progress on each thread; `raise_open_file_limit`
    /// in `lib.rs` makes room for that.
    pub fn delete(parent: &Path, name: &OsStr) -> io::Result<()> {
        let name = CString::new(name.as_bytes())?;
        let parent = open(
            parent,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let device = fstat(&parent)?.st_dev;
        Ok(delete_child(&parent, device, &name)?)
    }

    /// Deletes `name` in `parent` without following a link: a link or a file is unlinked,
    /// and a folder on the same drive as `parent` is emptied, then removed.
    fn delete_child(parent: &OwnedFd, device: Dev, name: &CStr) -> Result<(), Failure> {
        match openat(parent, name, FOLDER, Mode::empty()) {
            Ok(folder) => {
                // A drive mounted here is not deleted with the folder, as `rm -rf` would.
                if fstat(&folder)?.st_dev != device {
                    return Err(Failure {
                        path: PathBuf::new(),
                        cause: Cause::OtherVolume,
                    });
                }
                delete_contents(&folder, device)?;
                // Closed first: a server that shares files the Windows way can refuse to
                // remove a folder that is still open.
                drop(folder);
                Ok(unlinkat(parent, name, AtFlags::REMOVEDIR)?)
            }
            // The entry is a link or a file, or a folder became one after the listing.
            Err(Errno::LOOP | Errno::NOTDIR) => Ok(unlinkat(parent, name, AtFlags::empty())?),
            Err(errno) => Err(errno.into()),
        }
    }

    /// A child that is already gone counts as deleted: another program removed it.
    fn delete_contents(folder: &OwnedFd, device: Dev) -> Result<(), Failure> {
        let mut folders = Vec::new();
        let mut others = Vec::new();
        // On a copy of the handle, as std does: reopening the folder would need search permission.
        for entry in Dir::new(fcntl_dupfd_cloexec(folder, 0)?)? {
            let entry = entry?;
            let name = entry.file_name();
            if name == c"." || name == c".." {
                continue;
            }
            let is_folder = match entry.file_type() {
                FileType::Directory => true,
                // Some file systems never fill in the type.
                FileType::Unknown => match statat(folder, name, AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(stat) => FileType::from_raw_mode(stat.st_mode).is_dir(),
                    Err(Errno::NOENT) => continue,
                    Err(errno) => return Err(Failure::from(errno).in_child(name)),
                },
                _ => false,
            };
            if is_folder {
                folders.push(name.to_owned());
            } else {
                others.push(name.to_owned());
            }
        }

        others.par_iter().try_for_each(|name| {
            match unlinkat(folder, name.as_c_str(), AtFlags::empty()) {
                Ok(()) | Err(Errno::NOENT) => Ok(()),
                Err(errno) => Err(Failure::from(errno).in_child(name)),
            }
        })?;
        folders
            .par_iter()
            .try_for_each(|name| match delete_child(folder, device, name) {
                Err(Failure {
                    cause: Cause::Os(Errno::NOENT),
                    ..
                }) => Ok(()),
                result => result.map_err(|failure| failure.in_child(name)),
            })
    }

    #[cfg(test)]
    mod tests {
        use super::{Cause, Failure};
        use std::io;
        use std::path::PathBuf;

        #[test]
        fn a_folder_on_another_volume_says_it_was_left_in_place() {
            let error = io::Error::from(Failure {
                path: PathBuf::from("a/mnt"),
                cause: Cause::OtherVolume,
            });

            assert_eq!(error.kind(), io::ErrorKind::CrossesDevices);
            assert_eq!(
                error.to_string(),
                "a/mnt: it is on another drive or volume, so it was left in place"
            );
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::ffi::OsStr;
    use std::fs;
    use std::io;
    use std::os::windows::fs::FileTypeExt;
    use std::path::Path;

    pub fn delete(parent: &Path, name: &OsStr) -> io::Result<()> {
        // Joined again, so `link\` names the link, not the folder it points to.
        let path = &parent.join(name);
        let metadata = fs::symlink_metadata(path)?;
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            // The crate removes the folder itself with `RemoveDirectoryW`, which refuses a
            // read-only folder. Windows marks a folder read-only to give it a custom icon, so
            // the flag goes back when the delete fails.
            let read_only = metadata.permissions().readonly();
            if read_only {
                set_read_only(path, false)?;
            }
            // The crate opens each folder without following a link, works relative to open
            // handles, and clears the read-only flag of what is inside.
            let deleted = remove_dir_all::remove_dir_all(path);
            if deleted.is_err() && read_only {
                let _ = set_read_only(path, true);
            }
            deleted
        } else if file_type.is_symlink_dir() {
            // A link to a folder, or a junction, is removed as a folder, even when its target is gone.
            fs::remove_dir(path)
        } else {
            fs::remove_file(path)
        }
    }

    fn set_read_only(path: &Path, read_only: bool) -> io::Result<()> {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_readonly(read_only);
        fs::set_permissions(path, permissions)
    }
}

#[cfg(test)]
mod tests {
    use super::delete_entry;
    use std::fs;
    use std::io;
    use std::path::Path;

    #[test]
    fn deletes_a_tree_of_files_empty_folders_a_hidden_file_and_a_name_with_a_space() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::create_dir(root.join("empty")).unwrap();
        fs::write(root.join("a/one.txt"), b"one").unwrap();
        fs::write(root.join("a/two.txt"), b"two").unwrap();
        fs::write(root.join("a/b/three.txt"), b"three").unwrap();
        fs::write(root.join("a/b/file name.txt"), b"space").unwrap();
        fs::write(root.join(".hidden"), b"hidden").unwrap();

        delete_entry(&root).unwrap();

        assert!(fs::symlink_metadata(&root).is_err());
    }

    #[test]
    fn deletes_a_chain_of_64_nested_folders_each_with_a_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir(&root).unwrap();
        let mut dir = root.clone();
        for i in 0..64 {
            dir.push(format!("level-{i}"));
            fs::create_dir(&dir).unwrap();
            fs::write(dir.join("file.txt"), b"x").unwrap();
        }

        delete_entry(&root).unwrap();

        assert!(fs::symlink_metadata(&root).is_err());
    }

    #[test]
    fn deletes_a_folder_of_2000_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir(&root).unwrap();
        for i in 0..2_000 {
            fs::write(root.join(format!("file-{i}.txt")), b"x").unwrap();
        }

        delete_entry(&root).unwrap();

        assert!(fs::symlink_metadata(&root).is_err());
    }

    #[test]
    fn deletes_a_file() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file.txt");
        fs::write(&file, b"x").unwrap();

        delete_entry(&file).unwrap();

        assert!(fs::symlink_metadata(&file).is_err());
    }

    #[test]
    fn refuses_a_path_that_ends_in_dot_dot_and_leaves_its_parent() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("child")).unwrap();
        fs::write(root.join("keep.txt"), b"keep").unwrap();

        let err = delete_entry(&root.join("child").join("..")).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(fs::read(root.join("keep.txt")).unwrap(), b"keep");
        assert!(root.join("child").is_dir());
    }

    #[test]
    fn refuses_a_bare_name() {
        let err = delete_entry(Path::new("file.txt")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_missing_path_returns_not_found() {
        let temp = tempfile::tempdir().unwrap();
        let err = delete_entry(&temp.path().join("missing")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[cfg(unix)]
    #[test]
    fn deletes_a_link_inside_the_tree_and_leaves_the_outside_folder() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("tree");
        fs::create_dir(&root).unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), b"keep").unwrap();
        symlink("../outside", root.join("link-to-outside")).unwrap();

        delete_entry(&root).unwrap();

        assert!(fs::symlink_metadata(&root).is_err());
        assert_eq!(fs::read(outside.join("keep.txt")).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn deletes_a_link_to_a_folder_and_leaves_the_folder() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep.txt"), b"keep").unwrap();
        let link = temp.path().join("link");
        symlink(&target, &link).unwrap();

        delete_entry(&link).unwrap();

        assert!(fs::symlink_metadata(&link).is_err());
        assert_eq!(fs::read(target.join("keep.txt")).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn deletes_a_link_given_with_a_trailing_slash_and_leaves_its_target() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep.txt"), b"keep").unwrap();
        let link = temp.path().join("link");
        symlink(&target, &link).unwrap();
        let mut with_slash = link.clone().into_os_string();
        with_slash.push("/");

        delete_entry(Path::new(&with_slash)).unwrap();

        assert!(fs::symlink_metadata(&link).is_err());
        assert_eq!(fs::read(target.join("keep.txt")).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn deletes_a_broken_link() {
        let temp = tempfile::tempdir().unwrap();
        let link = temp.path().join("broken");
        std::os::unix::fs::symlink("gone", &link).unwrap();

        delete_entry(&link).unwrap();

        assert!(fs::symlink_metadata(&link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn deletes_a_selected_fifo() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let temp = tempfile::tempdir().unwrap();
        let fifo = temp.path().join("pipe");
        let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);

        delete_entry(&fifo).unwrap();

        assert!(fs::symlink_metadata(&fifo).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn deletes_a_fifo_inside_the_tree() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir(&root).unwrap();
        let fifo = root.join("pipe");
        let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);

        delete_entry(&root).unwrap();

        assert!(fs::symlink_metadata(&root).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_without_write_permission_returns_permission_denied() {
        use std::os::unix::fs::PermissionsExt;

        if rustix::process::geteuid().is_root() {
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let locked = root.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("file.txt"), b"x").unwrap();
        let mut perms = fs::metadata(&locked).unwrap().permissions();
        perms.set_mode(0o500);
        fs::set_permissions(&locked, perms).unwrap();

        let result = delete_entry(&root);

        if let Ok(metadata) = fs::metadata(&locked) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o700);
            fs::set_permissions(&locked, perms).unwrap();
        }

        let err = result.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
        // The error names what failed, from inside the selected folder.
        assert!(err.to_string().starts_with("locked/file.txt: "), "{err}");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn deletes_a_read_only_folder() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("file.txt"), b"x").unwrap();
        let mut perms = fs::metadata(&root).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&root, perms).unwrap();

        delete_entry(&root).unwrap();

        assert!(fs::symlink_metadata(&root).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn a_read_only_folder_that_cannot_be_deleted_stays_read_only() {
        use std::os::windows::fs::OpenOptionsExt;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir(&root).unwrap();
        let locked = root.join("locked.txt");
        fs::write(&locked, b"x").unwrap();
        // Held open with no delete sharing, as an editor does.
        let _open = fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1)
            .open(&locked)
            .unwrap();
        let mut perms = fs::metadata(&root).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&root, perms).unwrap();

        assert!(delete_entry(&root).is_err());

        assert!(fs::metadata(&root).unwrap().permissions().readonly());
        let mut perms = fs::metadata(&root).unwrap().permissions();
        perms.set_readonly(false);
        fs::set_permissions(&root, perms).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn deletes_a_selected_read_only_file() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("locked.txt");
        fs::write(&file, b"x").unwrap();
        let mut perms = fs::metadata(&file).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&file, perms).unwrap();

        delete_entry(&file).unwrap();

        assert!(fs::symlink_metadata(&file).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn deletes_a_read_only_file_inside_the_tree() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir(&root).unwrap();
        let file = root.join("locked.txt");
        fs::write(&file, b"x").unwrap();
        let mut perms = fs::metadata(&file).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&file, perms).unwrap();

        delete_entry(&root).unwrap();

        assert!(fs::symlink_metadata(&root).is_err());
    }

    #[cfg(target_os = "windows")]
    fn make_junction(junction: &Path, target: &Path) {
        let status = std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                junction.to_str().unwrap(),
                target.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn deletes_a_selected_junction_and_leaves_its_target() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep.txt"), b"keep").unwrap();
        let junction = temp.path().join("junction");
        make_junction(&junction, &target);

        delete_entry(&junction).unwrap();

        assert!(fs::symlink_metadata(&junction).is_err());
        assert_eq!(fs::read(target.join("keep.txt")).unwrap(), b"keep");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn deletes_a_selected_junction_whose_target_is_gone() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        let junction = temp.path().join("junction");
        make_junction(&junction, &target);
        fs::remove_dir(&target).unwrap();

        delete_entry(&junction).unwrap();

        assert!(fs::symlink_metadata(&junction).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn deletes_a_junction_inside_the_tree_and_leaves_the_outside_folder() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir(&root).unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), b"keep").unwrap();
        make_junction(&root.join("junction"), &outside);

        delete_entry(&root).unwrap();

        assert!(fs::symlink_metadata(&root).is_err());
        assert_eq!(fs::read(outside.join("keep.txt")).unwrap(), b"keep");
    }
}
