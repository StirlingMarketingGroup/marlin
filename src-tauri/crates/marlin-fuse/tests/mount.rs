//! Tests that mount through the kernel. On Linux, run them with `test-in-docker.sh`. On a Mac, run them with `test-on-mac.sh`.
//!
//! They must run one at a time (`--test-threads=1`). A test that starts a program (`fusermount3`, `umount`, `stat`, `ls`) forks, and until the child calls exec it holds a copy of every file this process has open. That includes a file another test has open on its own mount, so that test's unmount can fail with "Device or resource busy".

#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::collections::BTreeMap;
use std::ffi::{CString, OsString};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{FileExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use marlin_vfs::{
    Access, Attributes, Item, ItemId, Kind, MemoryProvider, Options, Provider, Space, Spool, Vfs,
    VfsError,
};

fn vfs<P: Provider + 'static>(provider: Arc<P>) -> Arc<Vfs> {
    let provider: Arc<dyn Provider> = provider;
    Arc::new(Vfs::new(
        provider,
        marlin_vfs::POSIX,
        Options {
            max_listings: 0,
            attributes_ttl: Duration::ZERO,
            space_ttl: Duration::ZERO,
            ..Options::default()
        },
    ))
}

/// A new empty folder to mount at. The folder goes away with the `TempDir`.
fn empty_dir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let point = dir.path().join("mnt");
    std::fs::create_dir(&point).unwrap();
    (dir, point)
}

/// The mount table entry for `path`.
///
/// On Linux this is the `/proc/self/mountinfo` line whose type is `fuse.marlin`. A line reads `<ids> <root> <mount point> <options> - <type> <source> <options>`.
/// On macOS this is the `mount` output line `<source> on <path> (<options>)` for that path.
fn mount_line(path: &Path) -> Option<String> {
    // The adapter mounts the canonical path. On macOS a temp folder's path often starts at `/var`, which is `/private/var`.
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    #[cfg(target_os = "linux")]
    {
        let table = std::fs::read_to_string("/proc/self/mountinfo").unwrap();
        table
            .lines()
            .find(|line| {
                let (before, after) = line.split_once(" - ").unwrap();
                before.split(' ').nth(4) == path.to_str() && after.starts_with("fuse.marlin ")
            })
            .map(str::to_string)
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("mount").output().unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        let marker = format!(" on {} (", path.display());
        text.lines()
            .find(|line| line.contains(&marker))
            .map(str::to_string)
    }
}

/// `block size`, `blocks`, `free blocks`, and the longest name, as the mounted filesystem reports them.
fn filesystem_line(path: &Path) -> String {
    #[cfg(target_os = "linux")]
    {
        let output = std::process::Command::new("stat")
            .args(["--file-system", "--format=%S %b %a %l"])
            .arg(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: statvfs is plain numbers. Zero is a valid value for the call to fill.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: `c` is the mount path, and `st` is a statvfs for the call to fill.
        let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
        assert_eq!(rc, 0, "{}", std::io::Error::last_os_error());
        format!(
            "{} {} {} {}",
            st.f_frsize, st.f_blocks, st.f_bavail, st.f_namemax
        )
    }
}

async fn names(path: &Path) -> std::io::Result<Vec<String>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(path)? {
            names.push(entry?.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        Ok(names)
    })
    .await
    .unwrap()
}

async fn read_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::read(path))
        .await
        .unwrap()
}

async fn read_at(path: &Path, offset: u64, size: usize) -> Vec<u8> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(path).unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();
        let mut buf = vec![0; size];
        file.read_exact(&mut buf).unwrap();
        buf
    })
    .await
    .unwrap()
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index % 251) as u8).collect()
}

/// A core with a spool and its uploader running. The settle time is whatever `options` holds.
fn spooled_vfs<P: Provider + 'static>(
    provider: Arc<P>,
    spool_limit: u64,
    options: Options,
) -> (Arc<Vfs>, tempfile::TempDir) {
    let provider: Arc<dyn Provider> = provider;
    let spool = tempfile::tempdir().unwrap();
    let vfs = Arc::new(
        Vfs::new(provider, marlin_vfs::POSIX, options)
            .take_writes(Spool {
                folder: spool.path().to_path_buf(),
                limit: spool_limit,
                stays_local: marlin_fuse::stays_local,
            })
            .unwrap(),
    );
    tokio::spawn(Arc::clone(&vfs).run_uploads());
    (vfs, spool)
}

/// Polls until every spooled file has been uploaded.
async fn wait_for_uploads(vfs: &Vfs) {
    let started = Instant::now();
    loop {
        if vfs.unsent().is_empty() {
            return;
        }
        if started.elapsed() >= Duration::from_secs(30) {
            panic!("{} still unsent", vfs.unsent().len());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The provider's tree. A file's value is its bytes. A folder's value is `None`.
async fn remote_files(provider: &MemoryProvider) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![(provider.root(), String::new())];
    while let Some((folder, prefix)) = pending.pop() {
        for item in provider.list(&folder).await.unwrap() {
            let path = if prefix.is_empty() {
                item.name.clone()
            } else {
                format!("{prefix}/{}", item.name)
            };
            if files.contains_key(&path) {
                panic!("{path} occurs twice");
            }
            match item.attributes.kind {
                Kind::Folder => {
                    files.insert(path.clone(), None);
                    pending.push((item.id, path));
                }
                Kind::File => {
                    files.insert(path, Some(provider.contents(&item.id)));
                }
            }
        }
    }
    files
}

/// Runs `sh -c script`. A failure shows the script's stderr.
async fn sh(script: &str) {
    let script = script.to_owned();
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn shell(path: &Path) -> String {
    format!("'{}'", path.display())
}

fn os_code<T>(result: std::io::Result<T>) -> Option<i32> {
    result.err().and_then(|error| error.raw_os_error())
}

async fn cat(path: &Path) -> Vec<u8> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let output = std::process::Command::new("cat")
            .arg(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    })
    .await
    .unwrap()
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn mounts_lists_reads_and_unmounts() {
    let provider = Arc::new(MemoryProvider::new());
    let root = provider.root();
    provider.add_file(&root, "alpha.txt", b"small");
    let folder = provider.add_folder(&root, "folder");
    provider.add_file(&folder, "beta.txt", b"inside");
    let big = pattern(300_000);
    provider.add_file(&root, "big.bin", &big);

    let (dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs(Arc::clone(&provider)), &point)
        .await
        .unwrap();
    // Without `allow_other`, the kernel lets no other user into the mount.
    assert!(!mount_line(&point).unwrap().contains("allow_other"));

    assert_eq!(
        names(&point).await.unwrap(),
        ["alpha.txt", "big.bin", "folder"]
    );
    assert_eq!(names(&point.join("folder")).await.unwrap(), ["beta.txt"]);
    assert_eq!(read_file(&point.join("alpha.txt")).await.unwrap(), b"small");
    assert_eq!(read_file(&point.join("big.bin")).await.unwrap(), big);
    assert_eq!(
        read_at(&point.join("big.bin"), 200_000, 1000).await,
        big[200_000..201_000]
    );

    let at = point.clone();
    let (file, folder) = tokio::task::spawn_blocking(move || {
        (
            std::fs::metadata(at.join("alpha.txt")).unwrap(),
            std::fs::metadata(at.join("folder")).unwrap(),
        )
    })
    .await
    .unwrap();
    assert_eq!(file.len(), 5);
    assert_eq!(file.mode() & 0o777, 0o400);
    // The temporary folder belongs to the user who runs the test, and so must every file in the mount.
    assert_eq!(file.uid(), std::fs::metadata(dir.path()).unwrap().uid());
    assert_eq!(folder.mode() & 0o777, 0o500);

    // The memory provider reports 1,000,000 bytes in total and 250,000 free. The fields are block size, blocks, free blocks, and the longest name.
    let at = point.clone();
    let space = tokio::task::spawn_blocking(move || filesystem_line(&at))
        .await
        .unwrap();
    assert_eq!(space, "4096 244 61 255");

    let new = point.join("new.txt");
    let created = tokio::task::spawn_blocking(move || std::fs::File::create(new))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(created.raw_os_error(), Some(libc::EROFS));

    let missing = read_file(&point.join("missing.txt")).await.unwrap_err();
    assert_eq!(missing.raw_os_error(), Some(libc::ENOENT));

    mount.unmount().await.unwrap();
    assert!(mount_line(&point).is_none());
    assert!(names(&point).await.unwrap().is_empty());
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn provider_errors_become_errnos() {
    let provider = Arc::new(MemoryProvider::new());
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs(Arc::clone(&provider)), &point)
        .await
        .unwrap();

    for (error, errno) in [
        (VfsError::NotFound, libc::ENOENT),
        (VfsError::AccessDenied, libc::EACCES),
        (VfsError::ReadOnly, libc::EROFS),
        (VfsError::Conflict, libc::EEXIST),
        (VfsError::NotAFolder, libc::ENOTDIR),
        (VfsError::IsAFolder, libc::EISDIR),
        (VfsError::NotEmpty, libc::ENOTEMPTY),
        (VfsError::NoSpace, libc::ENOSPC),
        (VfsError::SignInNeeded, libc::EACCES),
        (VfsError::TimedOut, libc::ETIMEDOUT),
        (VfsError::Offline, libc::ENETUNREACH),
        (VfsError::NotSupported, libc::ENOTSUP),
        (VfsError::StaleHandle, libc::ESTALE),
        (VfsError::Other("boom".into()), libc::EIO),
    ] {
        provider.fail_with(Some(error));
        let failed = names(&point).await.unwrap_err();
        assert_eq!(failed.raw_os_error(), Some(errno), "{failed}");
        provider.fail_with(None);
        names(&point).await.unwrap();
    }

    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn slow_calls_run_side_by_side() {
    let provider = Arc::new(MemoryProvider::new());
    let root = provider.root();
    for name in ["one", "two", "three", "four"] {
        provider.add_file(&root, name, name.as_bytes());
    }
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs(Arc::clone(&provider)), &point)
        .await
        .unwrap();

    provider.set_delay(Duration::from_millis(200));
    let mut tasks = Vec::new();
    for name in ["one", "two", "three", "four"] {
        let path = point.join(name);
        tasks.push(tokio::task::spawn_blocking(move || {
            std::fs::read(&path).map(|bytes| (name, bytes))
        }));
    }
    for task in tasks {
        let (name, bytes) = task.await.unwrap().unwrap();
        assert_eq!(bytes, name.as_bytes());
    }
    assert!(provider.peak_calls() >= 2);

    provider.set_delay(Duration::ZERO);
    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn busy_unmount_fails_and_the_mount_stays() {
    let provider = Arc::new(MemoryProvider::new());
    provider.add_file(&provider.root(), "note.txt", b"keep");
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs(provider), &point).await.unwrap();

    let note = point.join("note.txt");
    let file = tokio::task::spawn_blocking(move || std::fs::File::open(note).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        mount.unmount().await,
        Err(marlin_fuse::Error::Unmount(_))
    ));
    assert!(mount_line(&point).is_some());
    assert_eq!(names(&point).await.unwrap(), ["note.txt"]);

    let bytes = tokio::task::spawn_blocking(move || {
        let mut file = file;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).unwrap();
        buf
    })
    .await
    .unwrap();
    assert_eq!(bytes, b"keep");

    mount.unmount().await.unwrap();
    mount.unmount().await.unwrap();
    assert!(mount_line(&point).is_none());
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn unmount_does_not_wait_for_the_provider() {
    let provider = Arc::new(MemoryProvider::new());
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs(Arc::clone(&provider)), &point)
        .await
        .unwrap();

    // macOS sends a new mount about ten requests of its own in its first moments. One that meets the delay below holds the unmount until it is answered, so wait until half a second passes with no provider call.
    if cfg!(target_os = "macos") {
        let mut quiet = false;
        for _ in 0..20 {
            let calls = provider.calls();
            tokio::time::sleep(Duration::from_millis(500)).await;
            if provider.in_flight() == 0 && provider.calls() == calls {
                quiet = true;
                break;
            }
        }
        assert!(quiet, "macOS never stopped sending the new mount requests");
    }

    // An unmount that called into the mount would wait the full delay.
    provider.set_delay(Duration::from_secs(20));
    tokio::time::timeout(Duration::from_secs(5), mount.unmount())
        .await
        .expect("unmount waited for the provider")
        .unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn two_mounts_serve_their_own_files() {
    let first = Arc::new(MemoryProvider::new());
    let second = Arc::new(MemoryProvider::new());
    first.add_file(&first.root(), "from-a.txt", b"a");
    second.add_file(&second.root(), "from-b.txt", b"b");

    let (_dir_a, point_a) = empty_dir();
    let dir_b = tempfile::tempdir().unwrap();
    let point_b = dir_b.path().join("my drive");
    std::fs::create_dir(&point_b).unwrap();
    let mut mount_a = marlin_fuse::mount(vfs(first), &point_a).await.unwrap();
    let mut mount_b = marlin_fuse::mount(vfs(second), &point_b).await.unwrap();

    assert_eq!(names(&point_a).await.unwrap(), ["from-a.txt"]);
    assert_eq!(names(&point_b).await.unwrap(), ["from-b.txt"]);

    mount_a.unmount().await.unwrap();
    mount_b.unmount().await.unwrap();
    assert!(mount_line(&point_b).is_none());
    // Linux writes a space in the mount table as `\040`. A wrong read of that would leave the mount.
    #[cfg(target_os = "linux")]
    {
        let table = std::fs::read_to_string("/proc/self/mountinfo").unwrap();
        assert!(!table.contains("my\\040drive"));
    }
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_big_folder_lists_every_name_once() {
    let provider = Arc::new(MemoryProvider::new());
    let folder = provider.add_folder(&provider.root(), "many");
    for number in 0..1000 {
        provider.add_file(&folder, &format!("file-{number:04}.txt"), b"x");
    }

    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs(provider), &point).await.unwrap();
    let expected: Vec<_> = (0..1000)
        .map(|number| format!("file-{number:04}.txt"))
        .collect();
    assert_eq!(names(&point.join("many")).await.unwrap(), expected);

    // `read_dir` leaves out `.` and `..`, so `ls -a` checks that the folder lists each of them once.
    // macOS `ls` colors a name when `CLICOLOR_FORCE` is set, so the line is no longer exactly `.`.
    let many = point.join("many");
    let listed = tokio::task::spawn_blocking(move || {
        std::process::Command::new("ls")
            .arg("-a")
            .arg(many)
            .env_remove("CLICOLOR_FORCE")
            .env("CLICOLOR", "0")
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert_eq!(listed.lines().count(), 1002);
    assert_eq!(listed.lines().filter(|name| *name == ".").count(), 1);
    assert_eq!(listed.lines().filter(|name| *name == "..").count(), 1);

    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn opening_every_listed_file_asks_the_provider_for_no_file() {
    let provider = Arc::new(MemoryProvider::new());
    let folder = provider.add_folder(&provider.root(), "many");
    for number in 0..100 {
        provider.add_file(&folder, &format!("file-{number:03}.txt"), b"x");
    }

    // The core's default limits, not the ones in `vfs()`: this test is about what the core reuses.
    let core: Arc<dyn Provider> = provider.clone();
    let core = Arc::new(Vfs::new(core, marlin_vfs::POSIX, Options::default()));
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(core, &point).await.unwrap();

    let many = point.join("many");
    let listed = names(&many).await.unwrap();
    assert_eq!(listed.len(), 100);
    let calls = provider.calls();
    tokio::task::spawn_blocking(move || {
        for name in listed {
            let file = std::fs::File::open(many.join(name)).unwrap();
            assert_eq!(file.metadata().unwrap().size(), 1);
        }
    })
    .await
    .unwrap();
    // The stored attributes answer each open. On macOS each open also asks for the free space, and the stored answer covers that. The OS may ask about a folder again in that time, so a few calls are allowed.
    let asked = provider.calls() - calls;
    assert!(asked < 10, "provider calls: {asked}");

    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn unmount_succeeds_after_an_unmount_from_outside() {
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs(Arc::new(MemoryProvider::new())), &point)
        .await
        .unwrap();

    let at = point.clone();
    let status = tokio::task::spawn_blocking(move || unmount_from_outside(&at, false))
        .await
        .unwrap();
    assert!(status.success());

    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_name_over_255_bytes_is_left_out() {
    let provider = Arc::new(MemoryProvider::new());
    let long = "n".repeat(256);
    provider.add_file(&provider.root(), &long, b"x");
    provider.add_file(&provider.root(), &"n".repeat(255), b"x");
    provider.add_file(&provider.root(), "short.txt", b"x");

    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs(provider), &point).await.unwrap();
    assert_eq!(
        names(&point).await.unwrap(),
        ["n".repeat(255), "short.txt".into()]
    );
    let failed = read_file(&point.join(&long)).await.unwrap_err();
    assert_eq!(failed.raw_os_error(), Some(libc::ENAMETOOLONG));
    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn dropping_a_mount_detaches_it() {
    let provider = Arc::new(MemoryProvider::new());
    provider.add_file(&provider.root(), "note.txt", b"x");
    let (_dir, point) = empty_dir();
    let mount = marlin_fuse::mount(vfs(provider), &point).await.unwrap();
    assert!(mount_line(&point).is_some());
    drop(mount);

    let started = Instant::now();
    while mount_line(&point).is_some() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "still mounted 5 seconds after the drop"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_live_mount_is_not_a_dead_mount() {
    let provider = Arc::new(MemoryProvider::new());
    provider.add_file(&provider.root(), "note.txt", b"note");
    let (dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs(provider), &point).await.unwrap();

    assert!(!marlin_fuse::is_dead_mount(&point).await);
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&point, &link).unwrap();
    assert!(!marlin_fuse::is_dead_mount(&link).await);
    assert!(mount_line(&point).is_some());
    assert_eq!(read_file(&point.join("note.txt")).await.unwrap(), b"note");

    mount.unmount().await.unwrap();
}

/// One file whose bytes and modified time a test can replace. `reads` counts `read` calls.
struct CountingFile {
    inner: Mutex<(Vec<u8>, SystemTime)>,
    reads: AtomicUsize,
}

impl CountingFile {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            inner: Mutex::new((bytes, UNIX_EPOCH + Duration::from_secs(1_700_000_000))),
            reads: AtomicUsize::new(0),
        }
    }

    #[cfg(target_os = "linux")]
    fn set(&self, bytes: Vec<u8>, modified: SystemTime) {
        *self.inner.lock().unwrap() = (bytes, modified);
    }

    fn file_item(&self) -> Item {
        let inner = self.inner.lock().unwrap();
        Item {
            id: ItemId("file".to_string()),
            name: "file.bin".to_string(),
            attributes: Attributes {
                kind: Kind::File,
                size: inner.0.len() as u64,
                modified: inner.1,
                revision: None,
                read_only: false,
            },
        }
    }
}

#[async_trait::async_trait]
impl Provider for CountingFile {
    fn root(&self) -> ItemId {
        ItemId("root".to_string())
    }

    async fn item(&self, id: &ItemId) -> Result<Item, VfsError> {
        if id.0 == "root" {
            return Ok(Item {
                id: id.clone(),
                name: String::new(),
                attributes: Attributes {
                    kind: Kind::Folder,
                    size: 0,
                    modified: UNIX_EPOCH,
                    revision: None,
                    read_only: false,
                },
            });
        }
        if id.0 == "file" {
            return Ok(self.file_item());
        }
        Err(VfsError::NotFound)
    }

    async fn list(&self, folder: &ItemId) -> Result<Vec<Item>, VfsError> {
        if folder.0 != "root" {
            return Err(VfsError::NotAFolder);
        }
        Ok(vec![self.file_item()])
    }

    async fn read(&self, id: &ItemId, offset: u64, length: u32) -> Result<Vec<u8>, VfsError> {
        if id.0 != "file" {
            return Err(VfsError::NotFound);
        }
        self.reads.fetch_add(1, Ordering::SeqCst);
        let inner = self.inner.lock().unwrap();
        let size = inner.0.len() as u64;
        let start = offset.min(size) as usize;
        let end = offset.saturating_add(u64::from(length)).min(size) as usize;
        Ok(inner.0[start..end].to_vec())
    }

    async fn space(&self) -> Result<Space, VfsError> {
        Ok(Space {
            total: 1_000_000,
            free: 250_000,
        })
    }
}

// Linux only: this tests keep_cache, a flag the macOS adapter does not set.
#[cfg(target_os = "linux")]
#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_second_open_reads_nothing_again_and_a_changed_file_is_read_again() {
    let original = pattern(10_000);
    let file = Arc::new(CountingFile::new(original.clone()));
    let (_dir, point) = empty_dir();
    // The core's default limits, not the ones in `vfs()`: a reopened file passes the core's stored answers and the kernel's.
    let provider: Arc<dyn Provider> = file.clone();
    let core = Arc::new(Vfs::new(provider, marlin_vfs::POSIX, Options::default()));
    let mut mount = marlin_fuse::mount(core, &point).await.unwrap();

    let path = point.join("file.bin");
    assert_eq!(read_file(&path).await.unwrap(), original);
    let reads = file.reads.load(Ordering::SeqCst);
    assert!(reads > 0);
    assert_eq!(read_file(&path).await.unwrap(), original);
    assert_eq!(file.reads.load(Ordering::SeqCst), reads);

    let changed: Vec<u8> = original.iter().map(|byte| byte.wrapping_add(1)).collect();
    file.set(
        changed.clone(),
        UNIX_EPOCH + Duration::from_secs(1_700_000_100),
    );
    // The core reuses a listing and a node's attributes for five seconds, and the adapter tells the kernel it may cache names and attributes for one more.
    tokio::time::sleep(Duration::from_millis(6500)).await;
    assert_eq!(read_file(&path).await.unwrap(), changed);

    mount.unmount().await.unwrap();
}

/// Unmounts as a user would from a terminal. A plain unmount is refused while a file is open. A forced one is not.
fn unmount_from_outside(path: &Path, forced: bool) -> std::process::ExitStatus {
    let (program, plain, force): (&str, &[&str], &[&str]) = if cfg!(target_os = "linux") {
        ("fusermount3", &["-u"], &["-u", "-z"])
    } else {
        ("umount", &[], &["-f"])
    };
    std::process::Command::new(program)
        .args(if forced { force } else { plain })
        .arg(path)
        .status()
        .unwrap()
}

/// A file whose `read` waits until the test releases it.
struct HeldRead {
    bytes: Vec<u8>,
    entered: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl HeldRead {
    fn new(
        bytes: Vec<u8>,
    ) -> (
        Self,
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        (
            Self {
                bytes,
                entered: tokio::sync::Mutex::new(Some(entered_tx)),
                release: tokio::sync::Mutex::new(Some(release_rx)),
            },
            release_tx,
            entered_rx,
        )
    }
}

#[async_trait::async_trait]
impl Provider for HeldRead {
    fn root(&self) -> ItemId {
        ItemId("root".to_string())
    }

    async fn item(&self, id: &ItemId) -> Result<Item, VfsError> {
        if id.0 == "root" {
            return Ok(Item {
                id: id.clone(),
                name: String::new(),
                attributes: Attributes {
                    kind: Kind::Folder,
                    size: 0,
                    modified: std::time::UNIX_EPOCH,
                    revision: None,
                    read_only: false,
                },
            });
        }
        if id.0 == "file" {
            return Ok(Item {
                id: id.clone(),
                name: "note.txt".to_string(),
                attributes: Attributes {
                    kind: Kind::File,
                    size: self.bytes.len() as u64,
                    modified: std::time::UNIX_EPOCH,
                    revision: None,
                    read_only: false,
                },
            });
        }
        Err(VfsError::NotFound)
    }

    async fn list(&self, folder: &ItemId) -> Result<Vec<Item>, VfsError> {
        if folder.0 != "root" {
            return Err(VfsError::NotAFolder);
        }
        Ok(vec![self.item(&ItemId("file".to_string())).await?])
    }

    async fn read(&self, id: &ItemId, _offset: u64, _length: u32) -> Result<Vec<u8>, VfsError> {
        if id.0 != "file" {
            return Err(VfsError::NotFound);
        }
        let waiting = self.release.lock().await.take();
        if let Some(waiting) = waiting {
            if let Some(entered) = self.entered.lock().await.take() {
                let _ = entered.send(());
            }
            let _ = waiting.await;
        }
        Ok(self.bytes.clone())
    }

    async fn space(&self) -> Result<Space, VfsError> {
        Ok(Space {
            total: 1_000_000,
            free: 250_000,
        })
    }
}

fn read_settled(read: std::io::Result<Vec<u8>>) {
    match read {
        Ok(bytes) => assert_eq!(bytes, b"kept"),
        Err(error) => assert!(error.raw_os_error().is_some(), "{error}"),
    }
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn an_outside_unmount_during_a_read_does_not_crash() {
    let (gate, release, entered) = HeldRead::new(b"kept".to_vec());
    let gate = Arc::new(gate);
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs(Arc::clone(&gate)), &point)
        .await
        .unwrap();

    let path = point.join("note.txt");
    let reading = tokio::task::spawn_blocking(move || std::fs::read(path));
    entered.await.unwrap();
    let at = point.clone();
    // Forced: the waiting read holds the file open. On Linux the forced form is the lazy unmount, which detaches at once. macOS holds even a forced unmount until the read is answered, so the read is released while the unmount runs.
    let unmounting = tokio::task::spawn_blocking(move || unmount_from_outside(&at, true));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = release.send(());
    let read = tokio::time::timeout(Duration::from_secs(10), reading)
        .await
        .expect("the read never returned")
        .unwrap();
    read_settled(read);
    assert!(unmounting.await.unwrap().success());
    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn dropping_a_mount_during_a_read_does_not_crash() {
    let (gate, release, entered) = HeldRead::new(b"kept".to_vec());
    let gate = Arc::new(gate);
    let (_dir, point) = empty_dir();
    let mount = marlin_fuse::mount(vfs(Arc::clone(&gate)), &point)
        .await
        .unwrap();

    let path = point.join("note.txt");
    let reading = tokio::task::spawn_blocking(move || std::fs::read(path));
    entered.await.unwrap();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = release.send(());
    });
    drop(mount);
    let read = tokio::time::timeout(Duration::from_secs(10), reading)
        .await
        .expect("the read never returned")
        .unwrap();
    read_settled(read);

    let started = Instant::now();
    while mount_line(&point).is_some() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "still mounted 10 seconds after the drop"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_first_mount_leaves_a_second_mount_at_the_same_path() {
    let (_dir, point) = empty_dir();
    let first = marlin_fuse::mount(vfs(Arc::new(MemoryProvider::new())), &point)
        .await
        .unwrap();
    let at = point.clone();
    let status = tokio::task::spawn_blocking(move || unmount_from_outside(&at, false))
        .await
        .unwrap();
    assert!(status.success());
    let started = Instant::now();
    while mount_line(&point).is_some() {
        assert!(started.elapsed() < Duration::from_secs(10));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let provider = Arc::new(MemoryProvider::new());
    provider.add_file(&provider.root(), "note.txt", b"second");
    let mut second = marlin_fuse::mount(vfs(provider), &point).await.unwrap();
    drop(first);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(mount_line(&point).is_some());
    assert_eq!(read_file(&point.join("note.txt")).await.unwrap(), b"second");
    second.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_composed_accent_is_listed_and_read() {
    let provider = Arc::new(MemoryProvider::new());
    let name = "\u{e9}.txt";
    provider.add_file(&provider.root(), name, b"accent");
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs(provider), &point).await.unwrap();
    assert!(names(&point).await.unwrap().contains(&name.to_string()));
    assert_eq!(read_file(&point.join(name)).await.unwrap(), b"accent");
    // Opening by the decomposed name is #542. This records what a mount does until then.
    #[cfg(target_os = "macos")]
    {
        let error = read_file(&point.join("e\u{301}.txt")).await.unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ENOENT));
    }
    mount.unmount().await.unwrap();
}

// Linux lets a mount sit on a file, so only macOS refuses this one.
#[cfg(target_os = "macos")]
#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_mount_on_a_file_fails() {
    let dir = tempfile::tempdir().unwrap();
    let point = dir.path().join("file");
    std::fs::write(&point, b"not a folder").unwrap();
    let started = Instant::now();
    let failed = marlin_fuse::mount(vfs(Arc::new(MemoryProvider::new())), &point).await;
    assert!(matches!(failed, Err(marlin_fuse::Error::Refused)));
    // macOS reports this by ending the session, not through an error code. The adapter must notice that, not wait out its 30 seconds.
    assert!(started.elapsed() < Duration::from_secs(15));
    assert!(mount_line(&point).is_none());
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_provider_that_cannot_write_mounts_read_only() {
    let (vfs, _spool) = spooled_vfs(
        Arc::new(CountingFile::new(b"abc".to_vec())),
        1024 * 1024 * 1024,
        Options::default(),
    );
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs, &point).await.unwrap();

    let line = mount_line(&point).unwrap();
    #[cfg(target_os = "linux")]
    assert!(line.split(' ').nth(5).unwrap().starts_with("ro"), "{line}");
    #[cfg(target_os = "macos")]
    assert!(line.contains("read-only"), "{line}");
    let at = point.clone();
    let (file_mode, root_mode) = tokio::task::spawn_blocking(move || {
        (
            std::fs::metadata(at.join("file.bin")).unwrap().mode() & 0o777,
            std::fs::metadata(&at).unwrap().mode() & 0o777,
        )
    })
    .await
    .unwrap();
    assert_eq!(file_mode, 0o400);
    assert_eq!(root_mode, 0o500);

    let at = point.clone();
    let errors = tokio::task::spawn_blocking(move || {
        let file = at.join("file.bin");
        let created = os_code(std::fs::File::create(at.join("new.txt")));
        let opened = os_code(OpenOptions::new().write(true).open(&file));
        let made = os_code(std::fs::create_dir(at.join("dir")));
        let removed = os_code(std::fs::remove_file(&file));
        let renamed = os_code(std::fs::rename(&file, at.join("other.bin")));
        let mode = os_code(std::fs::set_permissions(
            &file,
            std::fs::Permissions::from_mode(0o600),
        ));
        let stamped = os_code(
            std::fs::File::open(&file).and_then(|opened| opened.set_modified(SystemTime::now())),
        );
        [created, opened, made, removed, renamed, mode, stamped]
    })
    .await
    .unwrap();
    for error in errors {
        assert_eq!(error, Some(libc::EROFS));
    }

    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn copies_reach_the_provider() {
    let provider = Arc::new(MemoryProvider::new());
    let (vfs, _spool) = spooled_vfs(
        Arc::clone(&provider),
        1024 * 1024 * 1024,
        Options::default(),
    );
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs.clone(), &point).await.unwrap();

    let src = tempfile::tempdir().unwrap();
    let top = pattern(300_000);
    let five = pattern(5 * 1024 * 1024);
    let one = pattern(1024 * 1024);
    std::fs::write(src.path().join("top.bin"), &top).unwrap();
    std::fs::write(src.path().join("five.bin"), &five).unwrap();
    let tree = src.path().join("tree");
    std::fs::create_dir_all(tree.join("sub/deeper")).unwrap();
    std::fs::create_dir(tree.join("empty")).unwrap();
    std::fs::write(tree.join("a.txt"), b"a\n").unwrap();
    std::fs::write(tree.join("sub/b.bin"), &one).unwrap();
    std::fs::write(tree.join("sub/deeper/c.txt"), b"c\n").unwrap();

    sh(&format!(
        "cp -- {src}/top.bin {src}/five.bin {mnt}/ && cp -r -- {src}/tree {mnt}/ && rsync -r -- {src}/tree/ {mnt}/synced/ && cp -- {mnt}/top.bin {mnt}/top-copy.bin",
        src = shell(src.path()),
        mnt = shell(&point),
    ))
    .await;

    let mut expect = BTreeMap::new();
    expect.insert("top.bin".to_string(), Some(top.clone()));
    // Both ends on the mount. On Linux the kernel copies with reads and writes, because the adapter has no `copy_file_range`.
    expect.insert("top-copy.bin".to_string(), Some(top));
    expect.insert("five.bin".to_string(), Some(five));
    expect.insert("tree".to_string(), None);
    expect.insert("tree/a.txt".to_string(), Some(b"a\n".to_vec()));
    expect.insert("tree/sub".to_string(), None);
    expect.insert("tree/sub/b.bin".to_string(), Some(one.clone()));
    expect.insert("tree/sub/deeper".to_string(), None);
    expect.insert("tree/sub/deeper/c.txt".to_string(), Some(b"c\n".to_vec()));
    expect.insert("tree/empty".to_string(), None);
    expect.insert("synced".to_string(), None);
    expect.insert("synced/a.txt".to_string(), Some(b"a\n".to_vec()));
    expect.insert("synced/sub".to_string(), None);
    expect.insert("synced/sub/b.bin".to_string(), Some(one));
    expect.insert("synced/sub/deeper".to_string(), None);
    expect.insert("synced/sub/deeper/c.txt".to_string(), Some(b"c\n".to_vec()));
    expect.insert("synced/empty".to_string(), None);
    let files_copied = expect.values().filter(|bytes| bytes.is_some()).count();
    for (rel, bytes) in &expect {
        if let Some(bytes) = bytes {
            assert_eq!(read_file(&point.join(rel)).await.unwrap(), *bytes, "{rel}");
        }
    }

    wait_for_uploads(&vfs).await;
    assert_eq!(remote_files(&provider).await, expect);
    eprintln!(
        "uploads {} for {} files copied",
        provider.uploads(),
        files_copied
    );

    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn editor_saves_keep_one_file() {
    let provider = Arc::new(MemoryProvider::new());
    let id = provider.add_file(&provider.root(), "notes.txt", b"first line\nold text\n");
    let (vfs, _spool) = spooled_vfs(
        Arc::clone(&provider),
        1024 * 1024 * 1024,
        Options::default(),
    );
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs.clone(), &point).await.unwrap();
    let notes = point.join("notes.txt");

    let before = provider.uploads();
    let in_place = if cfg!(target_os = "macos") {
        "-i ''"
    } else {
        "-i"
    };
    sh(&format!("sed {in_place} 's/old/new/' {}", shell(&notes))).await;
    // macOS writes a `._name` file beside each new file, and those stay on this computer.
    let user_names = |names: Vec<String>| -> Vec<String> {
        names
            .into_iter()
            .filter(|name| !marlin_fuse::stays_local(name))
            .collect()
    };
    assert_eq!(user_names(names(&point).await.unwrap()), ["notes.txt"]);
    wait_for_uploads(&vfs).await;
    let items = provider.list(&provider.root()).await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "notes.txt");
    assert_eq!(items[0].id, id);
    assert_eq!(provider.contents(&id), b"first line\nnew text\n");
    eprintln!("sed save added {} uploads", provider.uploads() - before);

    let before = provider.uploads();
    let vim = if cfg!(target_os = "macos") {
        "/usr/bin/vim"
    } else {
        "vim.tiny"
    };
    sh(&format!(
        "{vim} -N -u NONE -i NONE -es -c '%s/new/vim/' -c 'wq' {} < /dev/null",
        shell(&notes)
    ))
    .await;
    assert_eq!(user_names(names(&point).await.unwrap()), ["notes.txt"]);
    wait_for_uploads(&vfs).await;
    let items = provider.list(&provider.root()).await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "notes.txt");
    assert_eq!(provider.contents(&items[0].id), b"first line\nvim text\n");
    eprintln!(
        "vim save added {} uploads, kept the item id: {}",
        provider.uploads() - before,
        items[0].id == id
    );

    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn dd_into_the_middle_changes_only_those_bytes() {
    let provider = Arc::new(MemoryProvider::new());
    let mut expect = pattern(3 * 1024 * 1024);
    let id = provider.add_file(&provider.root(), "data.bin", &expect);
    let (vfs, _spool) = spooled_vfs(
        Arc::clone(&provider),
        1024 * 1024 * 1024,
        Options::default(),
    );
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs.clone(), &point).await.unwrap();

    let patch = point.parent().unwrap().join("patch.bin");
    // The mount point's parent is the test's temp folder, outside the mount.
    std::fs::write(&patch, vec![0xAA; 1000]).unwrap();
    sh(&format!(
        "dd if={} of={} bs=1000 seek=1500 count=1 conv=notrunc status=none",
        shell(&patch),
        shell(&point.join("data.bin"))
    ))
    .await;
    expect[1_500_000..1_501_000].fill(0xAA);
    assert_eq!(read_file(&point.join("data.bin")).await.unwrap(), expect);
    wait_for_uploads(&vfs).await;
    assert_eq!(provider.contents(&id), expect);

    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_file_the_account_may_not_change_shows_read_only() {
    let provider = Arc::new(MemoryProvider::new());
    let mine_id = provider.add_file(&provider.root(), "mine.txt", b"mine");
    let theirs_id = provider.add_file(&provider.root(), "theirs.txt", b"theirs");
    provider.set_read_only(&theirs_id, true);
    let gone_id = provider.add_file(&provider.root(), "gone.txt", b"gone");
    provider.set_read_only(&gone_id, true);
    let (vfs, _spool) = spooled_vfs(
        Arc::clone(&provider),
        1024 * 1024 * 1024,
        Options::default(),
    );
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs.clone(), &point).await.unwrap();

    let mine = point.join("mine.txt");
    let at = point.clone();
    // The new file stays open, so its upload waits and the rename meets a file that is only in the spool.
    let (modes, asked, open, rename, kept, moved, removed, writing) =
        tokio::task::spawn_blocking(move || {
            let mode = |name: &str| std::fs::metadata(at.join(name)).unwrap().mode() & 0o777;
            let modes = [mode("mine.txt"), mode("theirs.txt")];
            // What an app that asks before it saves hears, as Cocoa's `isWritableFileAtPath` asks.
            let path = CString::new(at.join("theirs.txt").into_os_string().into_vec()).unwrap();
            // SAFETY: `path` is a valid C string that lives through the call.
            let asked = (unsafe { libc::access(path.as_ptr(), libc::W_OK) } == -1)
                .then(|| std::io::Error::last_os_error().raw_os_error())
                .flatten();
            let open = os_code(OpenOptions::new().write(true).open(at.join("theirs.txt")));
            let writing = std::fs::File::create(at.join("tmp.txt")).unwrap();
            writing.write_at(b"new", 0).unwrap();
            let rename = os_code(std::fs::rename(at.join("tmp.txt"), at.join("theirs.txt")));
            let kept = std::fs::read(at.join("theirs.txt")).unwrap();
            // The mark is about the file's bytes. Its name still changes, as a link file's does.
            let moved = os_code(std::fs::rename(at.join("theirs.txt"), at.join("kept.txt")));
            let removed = os_code(std::fs::remove_file(at.join("gone.txt")));
            (modes, asked, open, rename, kept, moved, removed, writing)
        })
        .await
        .unwrap();
    assert_eq!(modes, [0o600, 0o400]);
    // macOS does not check the mode on this mount, and the adapter leaves `access` unanswered (see `Fs` in `macos/fs.rs`). An app that asks first finds out when its open fails.
    let refused_by_access = cfg!(target_os = "linux").then_some(libc::EACCES);
    assert_eq!(asked, refused_by_access);
    assert_eq!(open, Some(libc::EACCES));
    assert_eq!(rename, Some(libc::EACCES));
    assert_eq!(kept, b"theirs");
    assert_eq!(moved, None);
    assert_eq!(removed, None);
    // macOS writes a `._name` file beside each new file, and those stay on this computer.
    let listed: Vec<String> = names(&point)
        .await
        .unwrap()
        .into_iter()
        .filter(|name| !marlin_fuse::stays_local(name))
        .collect();
    assert_eq!(listed, ["kept.txt", "mine.txt", "tmp.txt"]);
    // Nothing was spooled for the two refusals.
    let unsent: Vec<String> = vfs
        .unsent()
        .into_iter()
        .map(|file| file.name)
        .filter(|name| !marlin_fuse::stays_local(name))
        .collect();
    assert_eq!(unsent, ["tmp.txt"]);
    tokio::task::spawn_blocking(move || drop(writing))
        .await
        .unwrap();

    sh(&format!("printf changed > {}", shell(&mine))).await;
    wait_for_uploads(&vfs).await;
    assert_eq!(provider.contents(&mine_id), b"changed");
    let files = remote_files(&provider).await;
    assert_eq!(files.get("kept.txt"), Some(&Some(b"theirs".to_vec())));
    assert_eq!(files.get("tmp.txt"), Some(&Some(b"new".to_vec())));
    assert!(!files.contains_key("gone.txt"));

    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_new_file_the_remote_cannot_hold_fails_like_a_new_folder() {
    let provider = Arc::new(MemoryProvider::new());
    let shares_id = provider.add_folder(&provider.root(), "shares");
    provider.refuse_new_items(&shares_id, true);
    let (vfs, _spool) = spooled_vfs(
        Arc::clone(&provider),
        1024 * 1024 * 1024,
        Options::default(),
    );
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs.clone(), &point).await.unwrap();

    let at = point.clone();
    let (made, folder, rename, writing) = tokio::task::spawn_blocking(move || {
        let made = os_code(std::fs::File::create(at.join("shares/a.txt")));
        let folder = os_code(std::fs::create_dir(at.join("shares/b")));
        let writing = std::fs::File::create(at.join("c.txt")).unwrap();
        writing.write_at(b"kept", 0).unwrap();
        let rename = os_code(std::fs::rename(at.join("c.txt"), at.join("shares/c.txt")));
        (made, folder, rename, writing)
    })
    .await
    .unwrap();
    assert_eq!(made, Some(libc::ENOTSUP));
    assert_eq!(folder, Some(libc::ENOTSUP));
    assert_eq!(rename, Some(libc::ENOTSUP));
    tokio::task::spawn_blocking(move || drop(writing))
        .await
        .unwrap();
    let listed: Vec<String> = names(&point.join("shares"))
        .await
        .unwrap()
        .into_iter()
        .filter(|name| !marlin_fuse::stays_local(name))
        .collect();
    assert!(listed.is_empty());
    wait_for_uploads(&vfs).await;
    let files = remote_files(&provider).await;
    assert_eq!(files.get("c.txt"), Some(&Some(b"kept".to_vec())));
    assert!(files.keys().all(|key| !key.starts_with("shares/")));

    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_folder_the_account_may_not_add_to_shows_read_only() {
    let provider = Arc::new(MemoryProvider::new());
    provider.add_folder(&provider.root(), "mine");
    let theirs_id = provider.add_folder(&provider.root(), "theirs");
    provider.set_read_only(&theirs_id, true);
    provider.add_file(&theirs_id, "shared.txt", b"old");
    let (vfs, _spool) = spooled_vfs(
        Arc::clone(&provider),
        1024 * 1024 * 1024,
        Options::default(),
    );
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs.clone(), &point).await.unwrap();

    let at = point.clone();
    let (modes, made, folder, rename, editing, writing) = tokio::task::spawn_blocking(move || {
        let mode = |name: &str| std::fs::metadata(at.join(name)).unwrap().mode() & 0o777;
        let modes = [mode("mine"), mode("theirs")];
        let made = os_code(std::fs::File::create(at.join("theirs/a.txt")));
        let folder = os_code(std::fs::create_dir(at.join("theirs/b")));
        let writing = std::fs::File::create(at.join("c.txt")).unwrap();
        writing.write_at(b"kept", 0).unwrap();
        let rename = os_code(std::fs::rename(at.join("c.txt"), at.join("theirs/c.txt")));
        let editing = OpenOptions::new()
            .write(true)
            .open(at.join("theirs/shared.txt"))
            .unwrap();
        editing.write_at(b"new", 0).unwrap();
        (modes, made, folder, rename, editing, writing)
    })
    .await
    .unwrap();
    assert_eq!(modes, [0o700, 0o500]);
    // On Linux the kernel refuses for a normal user from mode 0500 (`default_permissions`) and the core refuses for root. macOS checks no modes on this mount, so the core and the provider refuse. Every refusal is EACCES on both systems.
    assert_eq!(made, Some(libc::EACCES));
    assert_eq!(folder, Some(libc::EACCES));
    assert_eq!(rename, Some(libc::EACCES));
    tokio::task::spawn_blocking(move || {
        drop(editing);
        drop(writing);
    })
    .await
    .unwrap();
    let listed: Vec<String> = names(&point.join("theirs"))
        .await
        .unwrap()
        .into_iter()
        .filter(|name| !marlin_fuse::stays_local(name))
        .collect();
    assert_eq!(listed, ["shared.txt"]);
    wait_for_uploads(&vfs).await;
    let files = remote_files(&provider).await;
    assert_eq!(files.get("c.txt"), Some(&Some(b"kept".to_vec())));
    assert_eq!(files.get("theirs/shared.txt"), Some(&Some(b"new".to_vec())));
    assert_eq!(
        files
            .keys()
            .filter(|key| key.starts_with("theirs/"))
            .cloned()
            .collect::<Vec<_>>(),
        ["theirs/shared.txt"]
    );

    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_write_is_read_back_at_once_also_by_another_process() {
    let provider = Arc::new(MemoryProvider::new());
    let fresh = vec![1u8; 100_000];
    let page_bytes = pattern(100_000);
    let fresh_id = provider.add_file(&provider.root(), "fresh.bin", &fresh);
    let page_id = provider.add_file(&provider.root(), "page.txt", &page_bytes);
    let (vfs, _spool) = spooled_vfs(
        Arc::clone(&provider),
        1024 * 1024 * 1024,
        Options::default(),
    );
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs.clone(), &point).await.unwrap();

    sh(&format!("printf x > {}", shell(&point.join("fresh.bin")))).await;
    assert_eq!(provider.largest_read(), 0);
    assert_eq!(cat(&point.join("fresh.bin")).await, b"x");

    let page = point.join("page.txt");
    assert_eq!(read_file(&page).await.unwrap(), page_bytes);
    let mut expect = page_bytes;
    expect[50_000..50_007].copy_from_slice(b"CHANGED");
    let writing = tokio::task::spawn_blocking({
        let page = page.clone();
        move || {
            let file = OpenOptions::new().write(true).open(page).unwrap();
            file.write_at(b"CHANGED", 50_000).unwrap();
            file
        }
    })
    .await
    .unwrap();
    assert_eq!(read_file(&page).await.unwrap(), expect);
    assert_eq!(cat(&page).await, expect);
    tokio::task::spawn_blocking(move || drop(writing))
        .await
        .unwrap();
    assert_eq!(read_file(&page).await.unwrap(), expect);
    assert_eq!(cat(&page).await, expect);

    sh(&format!("printf %s replaced > {}", shell(&page))).await;
    let expect = b"replaced".to_vec();
    assert_eq!(read_file(&page).await.unwrap(), expect);
    assert_eq!(cat(&page).await, expect);
    sh(&format!("printf %s \" and more\" >> {}", shell(&page))).await;
    let expect = b"replaced and more".to_vec();
    assert_eq!(read_file(&page).await.unwrap(), expect);
    assert_eq!(cat(&page).await, expect);
    sh(&format!("truncate -s 4 {}", shell(&page))).await;
    let expect = b"repl".to_vec();
    assert_eq!(read_file(&page).await.unwrap(), expect);
    assert_eq!(cat(&page).await, expect);
    sh(&format!("truncate -s 10 {}", shell(&page))).await;
    let expect = b"repl\0\0\0\0\0\0".to_vec();
    assert_eq!(read_file(&page).await.unwrap(), expect);
    assert_eq!(cat(&page).await, expect);

    wait_for_uploads(&vfs).await;
    assert_eq!(provider.contents(&fresh_id), b"x");
    assert_eq!(provider.contents(&page_id), expect);
    assert_eq!(read_file(&page).await.unwrap(), expect);

    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn fsync_puts_open_bytes_in_the_spool() {
    let provider = Arc::new(MemoryProvider::new());
    let id = provider.add_file(&provider.root(), "notes.txt", b"remote\n");
    let (vfs, _spool) = spooled_vfs(
        Arc::clone(&provider),
        1024 * 1024 * 1024,
        Options::default(),
    );
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs.clone(), &point).await.unwrap();

    let notes = point.join("notes.txt");
    let file = tokio::task::spawn_blocking(move || {
        let file = OpenOptions::new().write(true).open(notes).unwrap();
        file.write_all_at(b"LOCAL", 0).unwrap();
        file.sync_all().unwrap();
        file
    })
    .await
    .unwrap();
    // Decision 0005 choice 5 says `fsync` returns when the bytes are in the spool on this computer.
    assert_eq!(vfs.unsent().len(), 1);
    let node = vfs.lookup(Vfs::ROOT, "notes.txt").await.unwrap().node;
    let handle = vfs.open(node, Access::Read).await.unwrap();
    assert_eq!(vfs.read(handle, 0, 100).await.unwrap(), b"LOCALe\n");
    vfs.release(handle);
    vfs.forget(node, 1);
    tokio::task::spawn_blocking(move || drop(file))
        .await
        .unwrap();
    wait_for_uploads(&vfs).await;
    assert_eq!(provider.contents(&id), b"LOCALe\n");

    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn names_change_through_the_mount() {
    let provider = Arc::new(MemoryProvider::new());
    let root = provider.root();
    let a_id = provider.add_file(&root, "a.txt", b"aaa");
    provider.add_file(&root, "b.txt", b"bbb");
    let dir = provider.add_folder(&root, "dir");
    provider.add_file(&dir, "inner.txt", b"inner");
    provider.add_folder(&root, "other");
    let (vfs, _spool) = spooled_vfs(
        Arc::clone(&provider),
        1024 * 1024 * 1024,
        Options::default(),
    );
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs.clone(), &point).await.unwrap();

    let at = point.clone();
    tokio::task::spawn_blocking(move || std::fs::create_dir(at.join("made")).unwrap())
        .await
        .unwrap();
    let mut expect = BTreeMap::from([
        ("a.txt".to_string(), Some(b"aaa".to_vec())),
        ("b.txt".to_string(), Some(b"bbb".to_vec())),
        ("dir".to_string(), None),
        ("dir/inner.txt".to_string(), Some(b"inner".to_vec())),
        ("other".to_string(), None),
        ("made".to_string(), None),
    ]);
    assert_eq!(remote_files(&provider).await, expect);

    let at = point.clone();
    tokio::task::spawn_blocking(move || {
        std::fs::rename(at.join("a.txt"), at.join("made/a2.txt")).unwrap();
    })
    .await
    .unwrap();
    expect.remove("a.txt");
    expect.insert("made/a2.txt".to_string(), Some(b"aaa".to_vec()));
    assert_eq!(remote_files(&provider).await, expect);
    let made_id = provider
        .list(&provider.root())
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.name == "made")
        .unwrap()
        .id;
    let moved = provider.list(&made_id).await.unwrap();
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].id, a_id);
    assert_eq!(provider.contents(&a_id), b"aaa");

    // The OS answers these itself: a rename onto a taken name, and a rename flag it does not pass on.
    #[cfg(target_os = "linux")]
    let (excl, swap, swap_errno) = (libc::RENAME_NOREPLACE, libc::RENAME_EXCHANGE, libc::EINVAL);
    #[cfg(target_os = "macos")]
    let (excl, swap, swap_errno) = (libc::RENAME_EXCL, libc::RENAME_SWAP, libc::ENOTSUP);
    let at = point.clone();
    let errno = tokio::task::spawn_blocking(move || {
        rename_flag(&at.join("b.txt"), &at.join("made/a2.txt"), excl)
    })
    .await
    .unwrap();
    assert_eq!(errno, libc::EEXIST);
    let at = point.clone();
    let errno = tokio::task::spawn_blocking(move || {
        rename_flag(&at.join("b.txt"), &at.join("made/a2.txt"), swap)
    })
    .await
    .unwrap();
    assert_eq!(errno, swap_errno);

    let at = point.clone();
    let errors = tokio::task::spawn_blocking(move || {
        [
            os_code(std::fs::rename(at.join("dir"), at.join("dir/inside"))),
            os_code(std::fs::remove_dir(at.join("dir"))),
            os_code(std::fs::remove_file(at.join("dir"))),
        ]
    })
    .await
    .unwrap();
    // Darwin's `unlink` of a folder is `EPERM` (`man 2 unlink`). Linux's is `EISDIR`.
    #[cfg(target_os = "linux")]
    let unlink_dir = libc::EISDIR;
    #[cfg(target_os = "macos")]
    let unlink_dir = libc::EPERM;
    assert_eq!(
        errors,
        [Some(libc::EINVAL), Some(libc::ENOTEMPTY), Some(unlink_dir)]
    );

    let at = point.clone();
    tokio::task::spawn_blocking(move || {
        std::fs::remove_file(at.join("dir/inner.txt")).unwrap();
        std::fs::remove_dir(at.join("dir")).unwrap();
        std::fs::remove_dir(at.join("other")).unwrap();
    })
    .await
    .unwrap();
    expect.remove("dir/inner.txt");
    expect.remove("dir");
    expect.remove("other");
    assert_eq!(remote_files(&provider).await, expect);

    let at = point.clone();
    tokio::task::spawn_blocking(move || {
        std::fs::rename(at.join("b.txt"), at.join("made/a2.txt")).unwrap();
    })
    .await
    .unwrap();
    expect.remove("b.txt");
    expect.insert("made/a2.txt".to_string(), Some(b"bbb".to_vec()));
    assert_eq!(remote_files(&provider).await, expect);

    let at = point.clone();
    let errors = tokio::task::spawn_blocking(move || {
        let mut bytes = b"bad".to_vec();
        bytes.push(0xff);
        let name = OsString::from_vec(bytes);
        let created = os_code(std::fs::File::create(at.join(&name)));
        let made = os_code(std::fs::create_dir(at.join(&name)));
        let long = os_code(std::fs::File::create(at.join("a".repeat(256))));
        let linked = os_code(std::fs::hard_link(
            at.join("made/a2.txt"),
            at.join("made/link.txt"),
        ));
        let symlinked = os_code(std::os::unix::fs::symlink("target", at.join("made/sym")));
        [created, made, long, linked, symlinked]
    })
    .await
    .unwrap();
    // Linux leaves `link` and `symlink` at `fuser`'s `EPERM`. macOS answers `ENOTSUP`, as `link(2)` documents there.
    #[cfg(target_os = "linux")]
    let unsupported = libc::EPERM;
    #[cfg(target_os = "macos")]
    let unsupported = libc::ENOTSUP;
    assert_eq!(
        errors,
        [
            Some(libc::EINVAL),
            Some(libc::EINVAL),
            Some(libc::ENAMETOOLONG),
            Some(unsupported),
            Some(unsupported),
        ]
    );

    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_new_file_shows_in_the_next_listing() {
    let provider = Arc::new(MemoryProvider::new());
    provider.add_file(&provider.root(), "a.txt", b"a");
    let (vfs, _spool) = spooled_vfs(
        Arc::clone(&provider),
        1024 * 1024 * 1024,
        Options::default(),
    );
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs.clone(), &point).await.unwrap();

    names(&point).await.unwrap();
    let at = point.clone();
    tokio::task::spawn_blocking(move || std::fs::write(at.join("b.txt"), b"b").unwrap())
        .await
        .unwrap();
    assert!(names(&point)
        .await
        .unwrap()
        .iter()
        .any(|name| name == "b.txt"));
    let at = point.clone();
    tokio::task::spawn_blocking(move || std::fs::remove_file(at.join("a.txt")).unwrap())
        .await
        .unwrap();
    assert!(!names(&point)
        .await
        .unwrap()
        .iter()
        .any(|name| name == "a.txt"));

    let at = point.clone();
    let second = tokio::task::spawn_blocking(move || {
        let path = CString::new(at.as_os_str().as_bytes()).unwrap();
        // SAFETY: `path` is nul-terminated and lives until `closedir`.
        let dir = unsafe { libc::opendir(path.as_ptr()) };
        assert!(!dir.is_null(), "{}", std::io::Error::last_os_error());
        let read_names = |dir: *mut libc::DIR| {
            let mut found = Vec::new();
            loop {
                // SAFETY: `dir` is the stream `opendir` returned, and this thread has not closed it.
                let entry = unsafe { libc::readdir(dir) };
                if entry.is_null() {
                    break;
                }
                // SAFETY: `entry` is the dirent `readdir` just returned for this stream.
                let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
                found.push(name.to_string_lossy().into_owned());
            }
            found
        };
        let first = read_names(dir);
        assert!(first.iter().any(|name| name == "b.txt"), "{first:?}");
        assert!(!first.iter().any(|name| name == "c.txt"), "{first:?}");
        std::fs::write(at.join("c.txt"), b"c").unwrap();
        // SAFETY: `dir` is still the stream `opendir` returned.
        unsafe { libc::rewinddir(dir) };
        let second = read_names(dir);
        // SAFETY: `dir` is the stream this call opened.
        let rc = unsafe { libc::closedir(dir) };
        assert_eq!(rc, 0, "{}", std::io::Error::last_os_error());
        second
    })
    .await
    .unwrap();
    assert!(second.iter().any(|name| name == "c.txt"));

    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_refresh_shows_a_remote_change_at_once() {
    let provider = Arc::new(MemoryProvider::new());
    let a = provider.add_file(&provider.root(), "a.txt", b"a");
    // The default options keep the listing cache. `vfs()` turns it off.
    let vfs = Arc::new(Vfs::new(
        Arc::clone(&provider) as Arc<dyn Provider>,
        marlin_vfs::POSIX,
        Options::default(),
    ));
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs.clone(), &point).await.unwrap();

    let file = point.join("a.txt");
    let size = || {
        let file = file.clone();
        tokio::task::spawn_blocking(move || std::fs::metadata(file).unwrap().len())
    };
    assert_eq!(names(&point).await.unwrap(), ["a.txt"]);
    assert_eq!(size().await.unwrap(), 1);
    // Bytes the kernel may keep across opens, which the new size must drop.
    assert_eq!(read_file(&file).await.unwrap(), b"a");

    provider.add_file(&provider.root(), "b.txt", b"b");
    provider.write(&a, b"abc");
    assert_eq!(names(&point).await.unwrap(), ["a.txt"]);

    let started = Instant::now();
    vfs.refresh();
    // The kernel keeps no listing.
    assert_eq!(names(&point).await.unwrap(), ["a.txt", "b.txt"]);
    // The kernel keeps a name's attributes for `TTL`, 1 second.
    while size().await.unwrap() != 3 {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the new size never showed"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let shown = started.elapsed();
    assert!(
        shown < Duration::from_millis(1500),
        "the new size showed after {shown:?}"
    );
    assert_eq!(read_file(&file).await.unwrap(), b"abc");

    mount.unmount().await.unwrap();
}

#[cfg(target_os = "macos")]
fn rename_flag(from: &Path, to: &Path, flags: libc::c_uint) -> i32 {
    let from = CString::new(from.as_os_str().as_bytes()).unwrap();
    let to = CString::new(to.as_os_str().as_bytes()).unwrap();
    // SAFETY: both paths are nul-terminated and live until the call returns.
    let rc = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), flags) };
    assert_eq!(rc, -1);
    std::io::Error::last_os_error().raw_os_error().unwrap()
}

#[cfg(target_os = "linux")]
fn rename_flag(from: &Path, to: &Path, flags: libc::c_uint) -> i32 {
    let from = CString::new(from.as_os_str().as_bytes()).unwrap();
    let to = CString::new(to.as_os_str().as_bytes()).unwrap();
    // SAFETY: both paths are nul-terminated and live until the call returns.
    let rc = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            flags,
        )
    };
    assert_eq!(rc, -1);
    std::io::Error::last_os_error().raw_os_error().unwrap()
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn preserved_times_reach_the_provider() {
    // 2001-02-03 04:05:06 UTC.
    let modified = UNIX_EPOCH + Duration::from_secs(981_173_106);
    let provider = Arc::new(MemoryProvider::new());
    let (vfs, _spool) = spooled_vfs(
        Arc::clone(&provider),
        1024 * 1024 * 1024,
        Options::default(),
    );
    let (dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs.clone(), &point).await.unwrap();
    let src = dir.path().join("src.txt");
    std::fs::write(&src, b"kept\n").unwrap();
    std::fs::File::open(&src)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    sh(&format!(
        "cp -p -- {} {} && rsync -t -- {} {}",
        shell(&src),
        shell(&point.join("cp.txt")),
        shell(&src),
        shell(&point.join("rsync.txt"))
    ))
    .await;
    let cp = point.join("cp.txt");
    let rsync = point.join("rsync.txt");
    let (cp_time, rsync_time) = tokio::task::spawn_blocking(move || {
        (
            std::fs::metadata(cp).unwrap().modified().unwrap(),
            std::fs::metadata(rsync).unwrap().modified().unwrap(),
        )
    })
    .await
    .unwrap();
    assert_eq!(cp_time, modified);
    assert_eq!(rsync_time, modified);

    wait_for_uploads(&vfs).await;
    let mut items = provider.list(&provider.root()).await.unwrap();
    items.sort_by(|left, right| left.name.cmp(&right.name));
    let names: Vec<&str> = items.iter().map(|item| item.name.as_str()).collect();
    assert_eq!(names, ["cp.txt", "rsync.txt"]);
    for item in &items {
        assert_eq!(item.attributes.modified, modified, "{}", item.name);
    }

    mount.unmount().await.unwrap();
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_copy_bigger_than_the_spool_waits_for_uploads() {
    let provider = Arc::new(MemoryProvider::new());
    let (vfs, _spool) = spooled_vfs(
        Arc::clone(&provider),
        32 * 1024 * 1024,
        Options {
            call_timeout: Duration::from_secs(10),
            ..Options::default()
        },
    );
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs.clone(), &point).await.unwrap();

    let src = tempfile::tempdir().unwrap();
    let big = src.path().join("big");
    std::fs::create_dir(&big).unwrap();
    let mut bodies = Vec::new();
    for n in 0..4u8 {
        let bytes = vec![n; 16 * 1024 * 1024];
        std::fs::write(big.join(format!("f{n}.bin")), &bytes).unwrap();
        bodies.push(bytes);
    }
    let started = Instant::now();
    sh(&format!("cp -r -- {} {}", shell(&big), shell(&point))).await;
    let took = started.elapsed();
    eprintln!("cp of four 16 MiB files took {took:?}");
    // Half the files fit. The rest wait for the first ones to upload, after their settle time.
    assert!(took >= Options::default().settle, "{took:?}");
    wait_for_uploads(&vfs).await;
    let remote = remote_files(&provider).await;
    assert_eq!(remote.get("big"), Some(&None));
    for (n, bytes) in bodies.iter().enumerate() {
        let got = remote
            .get(&format!("big/f{n}.bin"))
            .unwrap()
            .as_ref()
            .unwrap();
        assert!(got == bytes, "big/f{n}.bin");
    }
    assert_eq!(remote.len(), 5);

    let target = point.join("huge.bin");
    let error =
        tokio::task::spawn_blocking(move || std::fs::write(target, vec![9u8; 40 * 1024 * 1024]))
            .await
            .unwrap()
            .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::ENOSPC), "{error}");

    mount.unmount().await.unwrap();
}

// This is what a Finder drag does. The first run asks to let the terminal control Finder.
#[cfg(target_os = "macos")]
#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn finder_copies_reach_the_provider() {
    let provider = Arc::new(MemoryProvider::new());
    let (vfs, _spool) = spooled_vfs(
        Arc::clone(&provider),
        1024 * 1024 * 1024,
        Options::default(),
    );
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs.clone(), &point).await.unwrap();

    // Finder refuses a copy bigger than the free space before it writes a byte, and says "An item with the same name already exists". `MemoryProvider` reports 250,000 bytes free.
    let src = tempfile::tempdir().unwrap();
    let copied_bin = pattern(100_000);
    let b_bin = pattern(50_000);
    std::fs::write(src.path().join("copied.bin"), &copied_bin).unwrap();
    std::fs::create_dir_all(src.path().join("copied/sub")).unwrap();
    std::fs::write(src.path().join("copied/a.txt"), b"a\n").unwrap();
    std::fs::write(src.path().join("copied/sub/b.bin"), &b_bin).unwrap();

    sh(&format!(
        "osascript -e 'tell application \"Finder\" to duplicate {{POSIX file \"{src}/copied.bin\", POSIX file \"{src}/copied\"}} to (POSIX file \"{mnt}\" as alias)'",
        src = src.path().display(),
        mnt = point.display(),
    ))
    .await;

    wait_for_uploads(&vfs).await;
    let mut expect = BTreeMap::new();
    expect.insert("copied.bin".to_string(), Some(copied_bin));
    expect.insert("copied".to_string(), None);
    expect.insert("copied/a.txt".to_string(), Some(b"a\n".to_vec()));
    expect.insert("copied/sub".to_string(), None);
    expect.insert("copied/sub/b.bin".to_string(), Some(b_bin));
    assert_eq!(remote_files(&provider).await, expect);

    mount.unmount().await.unwrap();
}

#[cfg(target_os = "macos")]
const COCOA_SAFE_SAVE: &str = r#"
// A document that saves the text in `TEXT`, through NSDocument's own safe save, as TextEdit does.
ObjC.import('AppKit');
var TEXT = '';
ObjC.registerSubclass({
  name: 'MarlinTestDocument',
  superclass: 'NSDocument',
  methods: {
    'dataOfType:error:': {
      types: ['id', ['id', 'void *']],
      implementation: function (type, error) { return $(TEXT).dataUsingEncoding($.NSUTF8StringEncoding); },
    },
  },
});
function run(argv) {
  TEXT = argv[1];
  const doc = $.MarlinTestDocument.alloc.init;
  const err = Ref();
  if (!doc.writeSafelyToURLOfTypeForSaveOperationError($.NSURL.fileURLWithPath(argv[0]), 'public.plain-text', $.NSSaveOperation, err)) throw new Error('save: ' + err[0].localizedDescription.js);
}
"#;

// The folder is made and removed on the remote on every save (#628). The saved file keeps its id because it is still only in the spool when it is renamed (decision 0005, choice 9).
#[cfg(target_os = "macos")]
#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_cocoa_safe_save_keeps_the_item() {
    let provider = Arc::new(MemoryProvider::new());
    let id = provider.add_file(&provider.root(), "notes.txt", b"old\n");
    let (vfs, _spool) = spooled_vfs(
        Arc::clone(&provider),
        1024 * 1024 * 1024,
        Options::default(),
    );
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs.clone(), &point).await.unwrap();

    let script = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(script.path(), COCOA_SAFE_SAVE).unwrap();
    sh(&format!(
        "osascript -l JavaScript {} {} saved",
        shell(script.path()),
        shell(&point.join("notes.txt")),
    ))
    .await;

    wait_for_uploads(&vfs).await;
    let items = provider.list(&provider.root()).await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "notes.txt");
    assert_eq!(items[0].id, id);
    assert_eq!(provider.contents(&id), b"saved");

    mount.unmount().await.unwrap();
}
