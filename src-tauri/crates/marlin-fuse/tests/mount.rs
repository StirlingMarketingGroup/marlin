//! Tests that mount through the kernel. Run them with `test-in-docker.sh`.
//!
//! They must run one at a time (`--test-threads=1`). A test that starts a program (`fusermount3`, `stat`, `ls`) forks, and until the child calls exec it holds a copy of every file this process has open. That includes a file another test has open on its own mount, so that test's unmount can fail with "Device or resource busy".

#![cfg(target_os = "linux")]

use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use marlin_vfs::{MemoryProvider, Options, Provider, Vfs, VfsError};

fn vfs(provider: Arc<MemoryProvider>) -> Arc<Vfs> {
    Arc::new(Vfs::new(
        provider,
        marlin_vfs::POSIX,
        Options {
            max_listings: 0,
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

/// The `/proc/self/mountinfo` line that lists `path` as a mount of type `fuse.marlin`. A line reads `<ids> <root> <mount point> <options> - <type> <source> <options>`.
fn mount_line(path: &Path) -> Option<String> {
    let table = std::fs::read_to_string("/proc/self/mountinfo").unwrap();
    table
        .lines()
        .find(|line| {
            let (before, after) = line.split_once(" - ").unwrap();
            before.split(' ').nth(4) == path.to_str() && after.starts_with("fuse.marlin ")
        })
        .map(str::to_string)
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

#[ignore = "needs /dev/fuse and fusermount3: run crates/marlin-fuse/test-in-docker.sh"]
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
    let space = tokio::task::spawn_blocking(move || {
        std::process::Command::new("stat")
            .args(["--file-system", "--format=%S %b %a %l"])
            .arg(at)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&space.stdout).trim(),
        "4096 244 61 255"
    );

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

#[ignore = "needs /dev/fuse and fusermount3: run crates/marlin-fuse/test-in-docker.sh"]
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

#[ignore = "needs /dev/fuse and fusermount3: run crates/marlin-fuse/test-in-docker.sh"]
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

#[ignore = "needs /dev/fuse and fusermount3: run crates/marlin-fuse/test-in-docker.sh"]
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

#[ignore = "needs /dev/fuse and fusermount3: run crates/marlin-fuse/test-in-docker.sh"]
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

#[ignore = "needs /dev/fuse and fusermount3: run crates/marlin-fuse/test-in-docker.sh"]
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
    // The mount table writes the space as `\040`. If `unmount` read that wrong, it would find no mount and do nothing.
    let table = std::fs::read_to_string("/proc/self/mountinfo").unwrap();
    assert!(!table.contains("my\\040drive"));
}

#[ignore = "needs /dev/fuse and fusermount3: run crates/marlin-fuse/test-in-docker.sh"]
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
    let many = point.join("many");
    let listed = tokio::task::spawn_blocking(move || {
        std::process::Command::new("ls")
            .arg("-a")
            .arg(many)
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

#[ignore = "needs /dev/fuse and fusermount3: run crates/marlin-fuse/test-in-docker.sh"]
#[tokio::test(flavor = "multi_thread")]
async fn unmount_succeeds_after_an_unmount_from_outside() {
    let (_dir, point) = empty_dir();
    let mut mount = marlin_fuse::mount(vfs(Arc::new(MemoryProvider::new())), &point)
        .await
        .unwrap();

    let at = point.clone();
    let status = tokio::task::spawn_blocking(move || {
        std::process::Command::new("fusermount3")
            .arg("-u")
            .arg(at)
            .status()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(status.success());

    mount.unmount().await.unwrap();
}

#[ignore = "needs /dev/fuse and fusermount3: run crates/marlin-fuse/test-in-docker.sh"]
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

#[ignore = "needs /dev/fuse and fusermount3: run crates/marlin-fuse/test-in-docker.sh"]
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
