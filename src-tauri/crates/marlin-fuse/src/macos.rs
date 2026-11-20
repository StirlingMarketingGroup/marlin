//! Marlin's macOS mount adapter: serves a marlin-vfs mount through macFUSE's libfuse 3.

mod fs;
mod sys;

use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use marlin_vfs::Vfs;
use tokio::sync::oneshot;

/// Why a mount or an unmount failed. `Display` is the text the user sees. `NoMacFuse`, `MacOsTooOld`, and `Refused` say how to fix the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// macFUSE is not installed, or its library did not load.
    NoMacFuse,

    /// macOS is older than 15.4.
    MacOsTooOld,

    /// The mount ended before its volume appeared. macFUSE's mount program writes the reason to its own stderr, which this process cannot read. Turned-off extensions are the usual reason.
    Refused,

    /// The mount failed.
    Mount(String),

    /// The unmount failed, for example because a file is open. Holds what `umount` reported. The mount is still served.
    Unmount(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoMacFuse => f.write_str(
                "Marlin needs macFUSE 5 or later to mount. Install it from https://macfuse.github.io. Then open System Settings, General, Login Items & Extensions, File System Extensions, and turn on both \"macFUSE\" and \"macFUSE (local)\".",
            ),
            Self::MacOsTooOld => f.write_str("Marlin needs macOS 15.4 or later to mount."),
            Self::Refused => f.write_str(
                "macFUSE refused the mount. Open System Settings, General, Login Items & Extensions, File System Extensions, and check that both \"macFUSE\" and \"macFUSE (local)\" are turned on.",
            ),
            Self::Mount(text) => write!(f, "macOS could not mount: {text}"),
            Self::Unmount(text) => write!(f, "macOS could not unmount: {text}"),
        }
    }
}

impl std::error::Error for Error {}

const LIBFUSE: &str = "/usr/local/lib/libfuse3.4.dylib";

const MOUNT_WAIT: Duration = Duration::from_secs(30);

/// Where one session is in its life. The `Mount`, the session thread, and every request that has not been answered share it.
pub(super) struct Life {
    inner: Mutex<Inner>,
    /// Signaled when `inflight` reaches zero.
    idle: Condvar,
}

struct Inner {
    /// `fuse_session_loop` has returned, or never ran. The OS no longer serves this mount.
    left_loop: bool,

    /// Requests that have not been answered. The session thread destroys the session only at zero, because a reply needs the session.
    inflight: usize,

    /// `mount` gave up waiting. The session thread must not start the loop.
    abandoned: bool,
}

impl Life {
    pub(super) fn begin_request(&self) {
        self.inner.lock().unwrap().inflight += 1;
    }

    pub(super) fn end_request(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.inflight -= 1;
        if inner.inflight == 0 {
            self.idle.notify_all();
        }
    }

    /// Asks the session, not the path: another mount may be at the path by now.
    fn left_loop(&self) -> bool {
        self.inner.lock().unwrap().left_loop
    }
}

/// One live mount.
pub struct Mount {
    /// The path as the OS mount table spells it.
    mount_point: PathBuf,
    life: Arc<Life>,
}

/// Serves `vfs` at `mount_point`, an empty folder that exists. Returns once the mount is in the OS mount table.
pub async fn mount(vfs: Arc<Vfs>, mount_point: &Path) -> Result<Mount, Error> {
    // Taken before the mount exists: afterwards it would be a call into the new filesystem. The mount table holds this spelling, for example `/private/var` for `/var`.
    let mount_point = mount_point
        .canonicalize()
        .map_err(|error| Error::Mount(error.to_string()))?;

    let mut version = [0u8; 32];
    let mut len = version.len();
    // SAFETY: `version` is `len` bytes, and the name is a constant.
    let failed = unsafe {
        libc::sysctlbyname(
            c"kern.osproductversion".as_ptr(),
            version.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } != 0;
    if failed {
        return Err(Error::Mount(std::io::Error::last_os_error().to_string()));
    }
    let version = String::from_utf8_lossy(&version[..len]);
    preflight(version.trim_end_matches('\0'))?;
    let sys = sys::load(Path::new(LIBFUSE)).map_err(|_| Error::NoMacFuse)?;

    // The volume is named after the folder. libfuse splits options at a comma and reads a backslash as an escape, so each gets a backslash in front.
    let volname = mount_point
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace(',', "\\,");
    let writable = vfs.writable();
    // `backend=fskit` is required. Without it macFUSE loads its kernel extension, and macOS asks the user to change security settings in Recovery Mode. `ro` is only for a core that cannot write. A writable mount names neither `ro` nor `rw`.
    let ro = if writable { "" } else { "ro," };
    let options = CString::new(format!("{ro}backend=fskit,fsname=marlin,volname={volname}"))
        .map_err(|error| Error::Mount(error.to_string()))?;
    let path = CString::new(mount_point.as_os_str().as_bytes())
        .map_err(|error| Error::Mount(error.to_string()))?;

    let life = Arc::new(Life {
        inner: Mutex::new(Inner {
            left_loop: false,
            inflight: 0,
            abandoned: false,
        }),
        idle: Condvar::new(),
    });
    let fs = Box::new(fs::Fs {
        vfs,
        runtime: tokio::runtime::Handle::current(),
        shown: crate::Shown {
            // SAFETY: getuid cannot fail and touches no memory.
            uid: unsafe { libc::getuid() },
            // SAFETY: getgid cannot fail and touches no memory.
            gid: unsafe { libc::getgid() },
            writable,
        },
        sys,
        life: Arc::clone(&life),
    });
    let (tx, rx) = oneshot::channel();
    // Not joined. The thread ends by itself after the loop, and a join would wait for every provider call.
    std::thread::Builder::new()
        .name("marlin-fuse".into())
        .spawn(move || serve(fs, &options, &path, tx))
        .map_err(|error| Error::Mount(error.to_string()))?;

    let appeared = tokio::time::timeout(MOUNT_WAIT, async {
        rx.await
            .map_err(|_| Error::Mount("the session thread stopped".into()))??;
        // macFUSE starts the real mount when the loop starts, and the volume appears a second or two later. A mount that macOS or macFUSE refuses ends the loop instead.
        loop {
            if life.left_loop() {
                return Err(Error::Refused);
            }
            if is_mounted(&mount_point).map_err(|error| Error::Mount(error.to_string()))? {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    match appeared {
        Ok(Ok(())) => Ok(Mount { mount_point, life }),
        Ok(Err(error)) => Err(error),
        Err(_) => {
            // A thread still inside `fuse_session_mount` sees this when that call returns, skips the loop, and destroys the session. A thread already in the loop stays there until the OS ends the session: libfuse has no call that wakes it.
            life.inner.lock().unwrap().abandoned = true;
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                tokio::process::Command::new("umount")
                    .arg("-f")
                    .arg(&mount_point)
                    .output(),
            )
            .await;
            Err(Error::Mount("the volume did not appear".into()))
        }
    }
}

/// The session thread: creates the session, runs its loop, and is the only code that destroys it.
fn serve(fs: Box<fs::Fs>, options: &CStr, path: &CStr, tx: oneshot::Sender<Result<(), Error>>) {
    let sys = fs.sys;
    let life = Arc::clone(&fs.life);
    let mut argv = [
        c"marlin".as_ptr().cast_mut(),
        c"-o".as_ptr().cast_mut(),
        options.as_ptr().cast_mut(),
        std::ptr::null_mut(),
    ];
    let mut args = sys::FuseArgs {
        argc: 3,
        argv: argv.as_mut_ptr(),
        allocated: 0,
    };
    // Extensions stay off, so `struct stat` and `struct statvfs` are the system's own.
    let mut version = sys::LibfuseVersion {
        major: 3,
        minor: 18,
        hotfix: 3,
        flags: 0,
    };
    let fs = Box::into_raw(fs);
    // SAFETY: `args` and `version` live until the session is destroyed below. `OPS` is a static. `fs` is the userdata, freed below only after every request is answered.
    let session = unsafe {
        (sys.session_new)(
            &mut args,
            &fs::OPS,
            std::mem::size_of_val(&fs::OPS),
            &mut version,
            fs.cast(),
        )
    };
    let started = if session.is_null() {
        Err(Error::Mount("macFUSE could not start a session".into()))
    // SAFETY: `session` came from session_new, and `path` is the mount folder.
    } else if unsafe { (sys.session_mount)(session, path.as_ptr()) } != 0 {
        Err(Error::Refused)
    } else {
        Ok(())
    };
    let mounted = started.is_ok();
    let _ = tx.send(started);
    if mounted && !life.inner.lock().unwrap().abandoned {
        // SAFETY: the session is mounted. The call returns when the OS ends the session, which any unmount does.
        unsafe { (sys.session_loop)(session) };
    }
    {
        let mut inner = life.inner.lock().unwrap();
        inner.left_loop = true;
        while inner.inflight > 0 {
            inner = life.idle.wait(inner).unwrap();
        }
    }
    // SAFETY: no request is in flight and no new one can start, so nothing else uses the session or `fs`. A session that was never created or mounted skips its call.
    unsafe {
        if mounted {
            (sys.session_unmount)(session);
        }
        if !session.is_null() {
            (sys.session_destroy)(session);
        }
        (sys.opt_free_args)(&mut args);
        drop(Box::from_raw(fs));
    }
}

impl Mount {
    /// Unmounts and returns once the OS no longer shows the mount. It does not wait for provider calls that are still running. After an error the mount is still served.
    pub async fn unmount(&mut self) -> Result<(), Error> {
        // Nothing to do when the session has ended: this is a second call, or something outside Marlin removed the mount (`umount`, or eject in Finder).
        if self.life.left_loop() {
            return Ok(());
        }
        // A plain unmount fails while a file is open, and the mount stays served.
        let output = tokio::process::Command::new("umount")
            .arg(&self.mount_point)
            .output()
            .await
            .map_err(|error| Error::Unmount(error.to_string()))?;
        // A failed command still counts when the mount is gone: something outside Marlin removed it a moment before.
        if !output.status.success() && is_mounted(&self.mount_point).unwrap_or(true) {
            let text = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(Error::Unmount(text));
        }
        // The loop returns a moment after the mount goes. Waiting for it keeps a later drop from unmounting whatever is at this path by then.
        let started = Instant::now();
        while !self.life.left_loop() && started.elapsed() < Duration::from_secs(5) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        if self.life.left_loop() {
            return;
        }
        // A forced unmount, as dropping a `fuser` session detaches the mount on Linux. The session thread then leaves the loop and cleans up by itself.
        let _ = std::process::Command::new("umount")
            .arg("-f")
            .arg(&self.mount_point)
            .output();
    }
}

/// Unmounts the dead mount at `mount_point`. The folder itself stays.
pub async fn unmount_dead(mount_point: &Path) -> Result<(), Error> {
    // Forced, unlike `Mount::unmount`. Nothing serves a dead mount, so there is no open file to protect, and a plain unmount fails while any process has its working folder inside.
    let output = tokio::process::Command::new("umount")
        .arg("-f")
        .arg(mount_point)
        .output()
        .await
        .map_err(|error| Error::Unmount(error.to_string()))?;
    if !output.status.success() {
        let text = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(Error::Unmount(text));
    }
    Ok(())
}

/// Whether the OS mount table lists a mount at `mount_point`. `getfsstat` with `MNT_NOWAIT` reads the table without entering any filesystem, so it cannot wait on this adapter's own callbacks.
fn is_mounted(mount_point: &Path) -> std::io::Result<bool> {
    // SAFETY: a null buffer asks for the count and writes nothing.
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if count < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // Room for mounts that appear between the two calls.
    let mut table = Vec::new();
    // SAFETY: statfs is plain numbers and byte arrays. Zero is a valid value.
    table.resize_with(count as usize + 8, || unsafe {
        std::mem::zeroed::<libc::statfs>()
    });
    let bytes = std::mem::size_of_val(table.as_slice()) as libc::c_int;
    // SAFETY: `table` is `bytes` long. The call fills at most that much.
    let found = unsafe { libc::getfsstat(table.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
    if found < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(table[..found as usize].iter().any(|entry| {
        // SAFETY: the kernel ends `f_mntonname` with a NUL inside the array.
        let at = unsafe { CStr::from_ptr(entry.f_mntonname.as_ptr()) };
        Path::new(std::ffi::OsStr::from_bytes(at.to_bytes())) == mount_point
    }))
}

/// Checks that this macOS can mount. FSKit, which the adapter mounts through, came with macOS 15.4. The version is a parameter so a test can name an old one.
fn preflight(version: &str) -> Result<(), Error> {
    let mut parts = version.split('.').map(|part| part.parse::<u32>().ok());
    let major = parts.next().flatten();
    let minor = parts.next().flatten().unwrap_or(0);
    if !major.is_some_and(|major| (major, minor) >= (15, 4)) {
        return Err(Error::MacOsTooOld);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_macos_is_refused_and_a_missing_library_does_not_load() {
        assert_eq!(preflight("15.3"), Err(Error::MacOsTooOld));
        assert_eq!(preflight("14.6.1"), Err(Error::MacOsTooOld));
        assert_eq!(preflight("15"), Err(Error::MacOsTooOld));
        assert!(preflight("15.4").is_ok());
        assert!(preflight("15.4.1").is_ok());
        assert!(preflight("27.2").is_ok());
        // No unit test loads the real library, so nothing has filled the slot this call reads first.
        assert!(sys::load(Path::new("/marlin-no-such-library")).is_err());
        assert_eq!(
            Error::NoMacFuse.to_string(),
            "Marlin needs macFUSE 5 or later to mount. Install it from https://macfuse.github.io. Then open System Settings, General, Login Items & Extensions, File System Extensions, and turn on both \"macFUSE\" and \"macFUSE (local)\"."
        );
        assert_eq!(
            Error::MacOsTooOld.to_string(),
            "Marlin needs macOS 15.4 or later to mount."
        );
        assert_eq!(
            Error::Refused.to_string(),
            "macFUSE refused the mount. Open System Settings, General, Login Items & Extensions, File System Extensions, and check that both \"macFUSE\" and \"macFUSE (local)\" are turned on."
        );
    }
}
