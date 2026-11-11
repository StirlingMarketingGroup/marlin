use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

use fuser::{
    BackgroundSession, Config, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags,
    Generation, INodeNo, LockOwner, MountOption, OpenFlags, ReplyAttr, ReplyData, ReplyDirectory,
    ReplyEntry, ReplyOpen, Request, Session,
};
use tokio::runtime::{Handle, Runtime};
use tokio::signal::unix::{signal, SignalKind};

const USAGE: &str = "usage: mount-hello <serve|self-test> <mountpoint>";

const TTL: Duration = Duration::from_secs(1);
const HELLO: &str = "Hello from Marlin\n";
const SLOW: &str = "slow\n";
const SLOW_READ: Duration = Duration::from_secs(3);
const HELLO_INO: INodeNo = INodeNo(2);
const SLOW_INO: INodeNo = INodeNo(3);

/// Every callback moves its reply into a tokio task and returns at once.
/// The fuser dispatch thread never waits on the "backend".
struct HelloFs {
    rt: Handle,
    uid: u32,
    gid: u32,
    /// True while a read of `slow.txt` is waiting.
    slow_pending: Arc<AtomicBool>,
    /// Callbacks that arrived while `slow_pending` was true.
    callbacks_while_slow: Arc<AtomicU64>,
}

impl HelloFs {
    fn count_callback(&self) {
        if self.slow_pending.load(Ordering::SeqCst) {
            self.callbacks_while_slow.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn file_attr(ino: INodeNo, uid: u32, gid: u32) -> Option<FileAttr> {
    let (kind, perm, size, nlink) = match ino {
        INodeNo::ROOT => (FileType::Directory, 0o555, 0, 2),
        HELLO_INO => (FileType::RegularFile, 0o444, HELLO.len() as u64, 1),
        SLOW_INO => (FileType::RegularFile, 0o444, SLOW.len() as u64, 1),
        _ => return None,
    };
    Some(FileAttr {
        ino,
        size,
        blocks: size.div_ceil(512),
        atime: UNIX_EPOCH,
        mtime: UNIX_EPOCH,
        ctime: UNIX_EPOCH,
        crtime: UNIX_EPOCH,
        kind,
        perm,
        nlink,
        uid,
        gid,
        rdev: 0,
        blksize: 512,
        flags: 0,
    })
}

fn slice_at(data: &str, offset: u64, size: u32) -> &[u8] {
    let start = (offset as usize).min(data.len());
    let end = start.saturating_add(size as usize).min(data.len());
    &data.as_bytes()[start..end]
}

impl Filesystem for HelloFs {
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        self.count_callback();
        let ino = match name.to_str() {
            Some("hello.txt") if parent == INodeNo::ROOT => HELLO_INO,
            Some("slow.txt") if parent == INodeNo::ROOT => SLOW_INO,
            _ => INodeNo(0),
        };
        let attr = file_attr(ino, self.uid, self.gid);
        self.rt.spawn(async move {
            match attr {
                Some(attr) => reply.entry(&TTL, &attr, Generation(0)),
                None => reply.error(Errno::ENOENT),
            }
        });
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        self.count_callback();
        let attr = file_attr(ino, self.uid, self.gid);
        self.rt.spawn(async move {
            match attr {
                Some(attr) => reply.attr(&TTL, &attr),
                None => reply.error(Errno::ENOENT),
            }
        });
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        self.count_callback();
        self.rt.spawn(async move {
            if ino != INodeNo::ROOT {
                reply.error(Errno::ENOTDIR);
                return;
            }
            let entries = [
                (INodeNo::ROOT, FileType::Directory, "."),
                (INodeNo::ROOT, FileType::Directory, ".."),
                (HELLO_INO, FileType::RegularFile, "hello.txt"),
                (SLOW_INO, FileType::RegularFile, "slow.txt"),
            ];
            for (index, (ino, kind, name)) in entries.into_iter().enumerate().skip(offset as usize)
            {
                // The offset given back to the kernel is where the next call resumes.
                if reply.add(ino, (index + 1) as u64, kind, name) {
                    break;
                }
            }
            reply.ok();
        });
    }

    fn open(&self, _req: &Request, _ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        self.count_callback();
        self.rt.spawn(async move {
            reply.opened(FileHandle(0), FopenFlags::empty());
        });
    }

    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        if ino != SLOW_INO {
            self.count_callback();
            self.rt.spawn(async move {
                match ino {
                    HELLO_INO => reply.data(slice_at(HELLO, offset, size)),
                    _ => reply.error(Errno::EISDIR),
                }
            });
            return;
        }
        let slow_pending = Arc::clone(&self.slow_pending);
        self.rt.spawn(async move {
            slow_pending.store(true, Ordering::SeqCst);
            tokio::time::sleep(SLOW_READ).await;
            reply.data(slice_at(SLOW, offset, size));
            slow_pending.store(false, Ordering::SeqCst);
        });
    }
}

fn mount(
    rt: &Runtime,
    mountpoint: &Path,
) -> io::Result<(BackgroundSession, Arc<AtomicBool>, Arc<AtomicU64>)> {
    let slow_pending = Arc::new(AtomicBool::new(false));
    let callbacks_while_slow = Arc::new(AtomicU64::new(0));
    let hello_fs = HelloFs {
        rt: rt.handle().clone(),
        // SAFETY: getuid and getgid cannot fail and touch no memory.
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
        slow_pending: Arc::clone(&slow_pending),
        callbacks_while_slow: Arc::clone(&callbacks_while_slow),
    };

    let mut config = Config::default();
    config.mount_options = vec![
        MountOption::RO,
        MountOption::FSName("marlin-hello".to_string()),
    ];
    let session = Session::new(hello_fs, mountpoint, &config)?.spawn()?;
    Ok((session, slow_pending, callbacks_while_slow))
}

fn serve(rt: &Runtime, mountpoint: &Path) -> Result<(), String> {
    let (session, _, _) = mount(rt, mountpoint).map_err(|err| format!("mount: {err}"))?;
    println!("mounted {}", mountpoint.display());
    io::stdout()
        .flush()
        .map_err(|err| format!("stdout: {err}"))?;

    let signaled = rt.block_on(async {
        let mut sigint = signal(SignalKind::interrupt())?;
        let mut sigterm = signal(SignalKind::terminate())?;
        tokio::select! {
            _ = sigint.recv() => {}
            _ = sigterm.recv() => {}
        }
        io::Result::Ok(())
    });
    // join() alone waits for the session and does not unmount.
    session
        .umount_and_join()
        .map_err(|err| format!("unmount: {err}"))?;
    signaled.map_err(|err| format!("signal: {err}"))
}

fn is_mount(path: &Path) -> io::Result<bool> {
    Ok(fs::metadata(path)?.dev() != fs::metadata(path.join(".."))?.dev())
}

fn dir_names(dir: &Path) -> io::Result<Vec<OsString>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(dir)? {
        names.push(entry?.file_name());
    }
    names.sort();
    Ok(names)
}

/// Runs the checks against the live mount through ordinary OS calls.
/// Stops at the first failure and returns `<step>: <reason>`.
fn check_mounted(
    mountpoint: &Path,
    slow_pending: &AtomicBool,
    callbacks_while_slow: &AtomicU64,
) -> Result<(), String> {
    if !is_mount(mountpoint).map_err(|err| format!("mount: {err}"))? {
        return Err("mount: st_dev matches the parent".to_string());
    }
    println!("PASS mount");

    let names = dir_names(mountpoint).map_err(|err| format!("list: {err}"))?;
    if names != ["hello.txt", "slow.txt"] {
        return Err(format!("list: {names:?}"));
    }
    println!("PASS list");

    let hello_path = mountpoint.join("hello.txt");
    let hello = fs::read_to_string(&hello_path).map_err(|err| format!("read: {err}"))?;
    if hello != HELLO {
        return Err(format!("read: {hello:?}"));
    }
    println!("PASS read");

    let slow_path = mountpoint.join("slow.txt");
    let slow_started = Instant::now();
    let slow_reader = thread::spawn(move || fs::read_to_string(slow_path));
    while !slow_pending.load(Ordering::SeqCst) {
        if slow_started.elapsed() > Duration::from_secs(2) {
            return Err("concurrent: the slow read never reached the filesystem".to_string());
        }
        thread::sleep(Duration::from_millis(5));
    }
    let fast_started = Instant::now();
    dir_names(mountpoint).map_err(|err| format!("concurrent: list: {err}"))?;
    fs::read_to_string(&hello_path).map_err(|err| format!("concurrent: read: {err}"))?;
    let fast = fast_started.elapsed();
    let callbacks = callbacks_while_slow.load(Ordering::SeqCst);
    if !slow_pending.load(Ordering::SeqCst) {
        return Err("concurrent: the slow read finished before the fast calls".to_string());
    }
    if callbacks == 0 {
        return Err(
            "concurrent: no callback reached the filesystem during the slow read".to_string(),
        );
    }
    if fast > Duration::from_secs(1) {
        return Err(format!("concurrent: fast calls took {fast:?}"));
    }
    let slow = slow_reader
        .join()
        .map_err(|_| "concurrent: the slow reader panicked".to_string())?
        .map_err(|err| format!("concurrent: slow read: {err}"))?;
    let slow_took = slow_started.elapsed();
    if slow != SLOW || slow_took < Duration::from_millis(2500) {
        return Err(format!(
            "concurrent: slow read gave {slow:?} in {slow_took:?}"
        ));
    }
    println!(
        "PASS concurrent: fast calls took {fast:?} with {callbacks} callbacks during the slow read ({slow_took:?})"
    );

    match fs::File::create(mountpoint.join("new.txt")) {
        Ok(_) => return Err("read-only: create succeeded".to_string()),
        Err(err) if err.raw_os_error() == Some(libc::EROFS) => println!("PASS read-only: EROFS"),
        Err(err) if err.raw_os_error() == Some(libc::EACCES) => println!("PASS read-only: EACCES"),
        Err(err) => return Err(format!("read-only: {err}")),
    }
    Ok(())
}

fn self_test(rt: &Runtime, mountpoint: &Path) -> Result<(), String> {
    fs::create_dir_all(mountpoint).map_err(|err| format!("mount: {err}"))?;
    let (session, slow_pending, callbacks_while_slow) =
        mount(rt, mountpoint).map_err(|err| format!("mount: {err}"))?;

    let checked = check_mounted(mountpoint, &slow_pending, &callbacks_while_slow);
    // Unmount even when a check failed, so a failed run leaves no mount behind.
    let unmounted = session.umount_and_join();
    checked?;
    unmounted.map_err(|err| format!("unmount: {err}"))?;

    if is_mount(mountpoint).map_err(|err| format!("unmount: {err}"))? {
        return Err("unmount: the mountpoint is still a mount".to_string());
    }
    let left = dir_names(mountpoint).map_err(|err| format!("unmount: {err}"))?;
    if !left.is_empty() {
        return Err(format!("unmount: the directory still holds {left:?}"));
    }
    println!("PASS unmount");
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let run: fn(&Runtime, &Path) -> Result<(), String> =
        match (args.first().map(String::as_str), args.get(1)) {
            (Some("serve"), Some(_)) => serve,
            (Some("self-test"), Some(_)) => self_test,
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        };

    let rt = match Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            println!("FAIL runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    match run(&rt, Path::new(&args[1])) {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            println!("FAIL {reason}");
            ExitCode::FAILURE
        }
    }
}
