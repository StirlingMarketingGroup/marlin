//! Tests that mount through the kernel. On Linux, run them with `test-in-docker.sh`. On a Mac, run them with `test-on-mac.sh`.
//!
//! They must run one at a time (`--test-threads=1`). A test that starts a program (`fusermount3`, `umount`, `stat`, `ls`) forks, and until the child calls exec it holds a copy of every file this process has open. That includes a file another test has open on its own mount, so that test's unmount can fail with "Device or resource busy".

#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::io::{Read, Seek, SeekFrom};
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::sync::Mutex;
use std::time::{Duration, Instant};
#[cfg(target_os = "linux")]
use std::time::{SystemTime, UNIX_EPOCH};

use marlin_vfs::{
    Attributes, Item, ItemId, Kind, MemoryProvider, Options, Provider, Space, Vfs, VfsError,
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
        (VfsError::NoSpace, libc::ENOSPC),
        (VfsError::SignInNeeded, libc::EACCES),
        (VfsError::TimedOut, libc::ETIMEDOUT),
        (VfsError::Offline, libc::ENETUNREACH),
        (VfsError::NotSupported, libc::EOPNOTSUPP),
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
#[cfg(target_os = "linux")]
struct CountingFile {
    inner: Mutex<(Vec<u8>, SystemTime)>,
    reads: AtomicUsize,
}

#[cfg(target_os = "linux")]
impl CountingFile {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            inner: Mutex::new((bytes, UNIX_EPOCH + Duration::from_secs(1_700_000_000))),
            reads: AtomicUsize::new(0),
        }
    }

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
            },
        }
    }
}

#[cfg(target_os = "linux")]
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

// Linux drop detaches whatever FUSE mount is at the path, so it would remove the second mount.
#[cfg(target_os = "macos")]
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
