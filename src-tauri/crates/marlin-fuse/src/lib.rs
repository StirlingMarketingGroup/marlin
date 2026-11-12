//! Marlin's Linux mount adapter: serves a marlin-vfs mount through FUSE.

#![cfg(target_os = "linux")]

mod fs;

use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use marlin_vfs::Vfs;

/// Why a mount or an unmount failed. `Display` is the text the user sees. `NoFuseDevice` and `NoFusermount` say how to fix the machine. The other two carry what `fuser` or `fusermount3` reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `/dev/fuse` does not exist.
    NoFuseDevice,

    /// `fusermount3` is not installed.
    NoFusermount,

    /// The mount failed. Holds what `fuser`, `fusermount3`, or the mount task reported.
    Mount(String),

    /// The unmount failed, for example because a file is open. Holds what `fusermount3` reported. The mount is still served.
    Unmount(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoFuseDevice => f.write_str(
                "This machine has no /dev/fuse. Load the FUSE kernel module with `sudo modprobe fuse`. In a container, pass the host's /dev/fuse through.",
            ),
            Self::NoFusermount => f.write_str(
                "Marlin needs fusermount3 to mount. Install the fuse3 package: `sudo apt install fuse3` on Debian and Ubuntu, `sudo dnf install fuse3` on Fedora, `sudo pacman -S fuse3` on Arch.",
            ),
            Self::Mount(text) => write!(f, "Linux could not mount: {text}"),
            Self::Unmount(text) => write!(f, "Linux could not unmount: {text}"),
        }
    }
}

impl std::error::Error for Error {}

/// One live mount.
pub struct Mount {
    /// The path as the kernel's mount table spells it.
    mount_point: PathBuf,

    /// Held so that a drop detaches the mount. `fuser` 0.18.0 cannot tell that a mount is already gone, so a drop always asks it to detach whatever FUSE mount is at the path. Drop a `Mount` before the path is used again.
    _session: fuser::BackgroundSession,
}

/// Serves `vfs` at `mount_point`, an empty folder that exists. Returns once the mount is in the OS mount table.
pub async fn mount(vfs: Arc<Vfs>, mount_point: &Path) -> Result<Mount, Error> {
    let filesystem = fs::Fs {
        vfs,
        runtime: tokio::runtime::Handle::current(),
        // SAFETY: getuid cannot fail and touches no memory.
        uid: unsafe { libc::getuid() },
        // SAFETY: getgid cannot fail and touches no memory.
        gid: unsafe { libc::getgid() },
    };
    let mut config = fuser::Config::default();
    // No `allow_other`: only the mounting user may open the mount. `default_permissions` has the kernel enforce the modes, so the adapter needs no access callback.
    config.mount_options = vec![
        fuser::MountOption::RO,
        fuser::MountOption::DefaultPermissions,
        fuser::MountOption::FSName("marlin".into()),
        fuser::MountOption::Subtype("marlin".into()),
    ];
    // Taken before the mount exists: afterwards it would be a call into the new filesystem.
    let mount_point = mount_point
        .canonicalize()
        .map_err(|error| Error::Mount(error.to_string()))?;
    let at = mount_point.clone();
    // These calls block: they run `fusermount3` and wait for the kernel's first message. The mount is in the mount table when `Session::new` returns.
    let session = tokio::task::spawn_blocking(move || {
        preflight(Path::new("/dev/fuse"), "fusermount3")?;
        fuser::Session::new(filesystem, &at, &config)
            .and_then(|session| session.spawn())
            .map_err(|error| Error::Mount(error.to_string().trim().to_string()))
    })
    .await
    .map_err(|error| Error::Mount(error.to_string()))??;
    Ok(Mount {
        mount_point,
        _session: session,
    })
}

impl Mount {
    /// Unmounts and returns once the OS no longer shows the mount. It does not wait for the session thread, which ends only when every copy of the mount is gone. After an error the mount is still served.
    pub async fn unmount(&mut self) -> Result<(), Error> {
        // Nothing to do when the mount is already gone: this is a second call, or something outside Marlin removed it (`fusermount3 -u`, or eject in a file manager).
        if !is_mounted(&self.mount_point).map_err(|error| Error::Unmount(error.to_string()))? {
            return Ok(());
        }
        // A plain unmount fails while a file is open, and the mount stays served.
        // BackgroundSession::umount_and_join is a lazy unmount (MNT_DETACH, or `fusermount3 -u -q -z`), which would hide a busy mount.
        let output = tokio::process::Command::new("fusermount3")
            .arg("-u")
            .arg("--")
            .arg(&self.mount_point)
            .output()
            .await
            .map_err(|error| Error::Unmount(error.to_string()))?;
        if !output.status.success() {
            let text = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(Error::Unmount(text));
        }
        Ok(())
    }
}

/// Whether this process's mount table lists a mount at `mount_point`.
fn is_mounted(mount_point: &Path) -> std::io::Result<bool> {
    let table = std::fs::read_to_string("/proc/self/mountinfo")?;
    Ok(table.lines().any(|line| {
        // The mount point is the fifth field of a line. The kernel writes a space, tab, newline, and backslash in it as `\040`, `\011`, `\012`, and `\134`.
        let at = line
            .split(' ')
            .nth(4)
            .unwrap_or_default()
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\");
        Path::new(&at) == mount_point
    }))
}

/// Checks what a mount needs. The two inputs are parameters so the tests can name a device and a program that do not exist.
fn preflight(device: &Path, fusermount: &str) -> Result<(), Error> {
    if !device.exists() {
        return Err(Error::NoFuseDevice);
    }
    match Command::new(fusermount).arg("--version").output() {
        Err(error) if error.kind() == ErrorKind::NotFound => Err(Error::NoFusermount),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_reports_a_missing_device_and_a_missing_fusermount() {
        assert_eq!(
            preflight(Path::new("/marlin-no-such-device"), "fusermount3"),
            Err(Error::NoFuseDevice)
        );
        assert_eq!(
            preflight(Path::new("/dev/null"), "marlin-no-such-program"),
            Err(Error::NoFusermount)
        );
        assert!(Error::NoFusermount.to_string().contains("fuse3"));
        assert!(Error::NoFuseDevice.to_string().contains("modprobe fuse"));
    }
}
