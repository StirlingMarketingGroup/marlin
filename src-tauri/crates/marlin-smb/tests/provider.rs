use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use marlin_smb_client::protocol::READ_MAX;
use marlin_smb_client::{add_server, SmbProvider, SmbSignIn};
use marlin_test_samba::{Server, DENIED_SHARE, PASSWORD};
use marlin_vfs::{ItemId, Kind, Provider, Revision, VfsError};

fn mounted(password: &str) -> (Server, tempfile::TempDir, SmbProvider) {
    marlin_test_keychain::install();
    let server = Server::start();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("smb-servers.json");
    add_server(
        &path,
        server.host(),
        server.user().into(),
        password.into(),
        None,
    )
    .unwrap();
    let sign_in = Arc::new(SmbSignIn::new(&server.host(), path));
    let provider = SmbProvider::new(sign_in, Some(env!("CARGO_BIN_EXE_marlin-smb").into()));
    (server, dir, provider)
}

#[tokio::test]
async fn the_server_lists_its_shares_and_a_folder() {
    let (server, _dir, provider) = mounted(PASSWORD);
    let notes = server.path("notes.txt");
    let mut file = File::create(&notes).unwrap();
    file.write_all(b"hello").unwrap();
    // A fractional file time: a listing and `item` both report the whole second.
    let modified = UNIX_EPOCH + Duration::from_millis(1_700_000_000_500);
    file.set_modified(modified).unwrap();
    drop(file);
    std::fs::create_dir(server.path("sub")).unwrap();
    std::fs::write(server.path(".hidden"), b"h").unwrap();

    let shares = provider.list(&ItemId("/".into())).await.unwrap();
    let mut ids: Vec<_> = shares.iter().map(|item| item.id.0.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["/data", "/denied", "/odd%41\"share"]);
    assert!(shares
        .iter()
        .all(|item| item.attributes.kind == Kind::Folder));
    let data = provider.item(&ItemId("/data".into())).await.unwrap();
    assert_eq!(data.attributes.kind, Kind::Folder);
    assert_eq!(data.attributes.modified, UNIX_EPOCH);
    // A share the user may not open is still a folder, as the listing shows it.
    let denied = provider.item(&ItemId("/denied".into())).await.unwrap();
    assert_eq!(denied.attributes.kind, Kind::Folder);
    let root = provider.item(&ItemId("/".into())).await.unwrap();
    assert_eq!(root.attributes.kind, Kind::Folder);

    let listed = provider.list(&ItemId("/data".into())).await.unwrap();
    let notes_item = listed.iter().find(|item| item.name == "notes.txt").unwrap();
    let sub = listed.iter().find(|item| item.name == "sub").unwrap();
    let hidden = listed.iter().find(|item| item.name == ".hidden").unwrap();
    assert_eq!(notes_item.attributes.kind, Kind::File);
    assert_eq!(notes_item.attributes.size, 5);
    assert_eq!(
        notes_item.attributes.modified,
        UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    );
    assert_eq!(sub.attributes.kind, Kind::Folder);
    assert_eq!(hidden.attributes.kind, Kind::File);

    let asked = provider.item(&notes_item.id).await.unwrap();
    assert_eq!(asked.attributes, notes_item.attributes);
    assert_eq!(asked.name, notes_item.name);
}

#[tokio::test]
async fn names_with_url_characters_reach_their_own_file() {
    let (server, _dir, provider) = mounted(PASSWORD);
    std::fs::write(server.path("a%20b.txt"), b"percent twenty").unwrap();
    std::fs::write(server.path("a b.txt"), b"space").unwrap();

    let listed = provider.list(&ItemId("/data".into())).await.unwrap();
    let item = listed.iter().find(|item| item.name == "a%20b.txt").unwrap();
    assert_eq!(item.id.0, "/data/a%20b.txt");
    assert_eq!(item.attributes.size, 14);
    let asked = provider.item(&item.id).await.unwrap();
    assert_eq!(asked.attributes, item.attributes);

    let got = provider.read(&item.id, 0, 100).await.unwrap();
    assert_eq!(got, b"percent twenty");

    let odd = provider
        .list(&ItemId("/odd%41\"share".into()))
        .await
        .unwrap();
    assert!(odd.iter().any(|item| item.name == "a%20b.txt"));
}

#[tokio::test]
async fn a_read_crosses_the_16_mib_piece() {
    let (server, _dir, provider) = mounted(PASSWORD);
    let size = 20 * 1024 * 1024;
    let bytes: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    std::fs::write(server.path("big.bin"), &bytes).unwrap();
    let id = ItemId("/data/big.bin".into());

    let got = provider.read(&id, 3, READ_MAX + 1000).await.unwrap();
    let start = 3;
    let end = start + READ_MAX as usize + 1000;
    // Not `assert_eq!` on the bytes: a failure would print 16 MiB twice.
    assert_eq!(got.len(), end - start);
    assert_eq!(
        got.iter()
            .zip(&bytes[start..end])
            .position(|(got, want)| got != want),
        None,
        "the first wrong byte"
    );

    let len = bytes.len() as u64;
    let short = provider.read(&id, len - 10, 50).await.unwrap();
    assert_eq!(short, bytes[bytes.len() - 10..]);
    let empty = provider.read(&id, len + 10, 50).await.unwrap();
    assert!(empty.is_empty());
}

#[tokio::test]
async fn errors_match_the_server() {
    let (server, _dir, provider) = mounted(PASSWORD);
    std::fs::write(server.path("notes.txt"), b"hello").unwrap();
    std::fs::create_dir(server.path("sub")).unwrap();

    assert_eq!(
        provider.list(&ItemId("/denied".into())).await.unwrap_err(),
        VfsError::AccessDenied
    );
    assert_eq!(
        provider
            .item(&ItemId("/data/none".into()))
            .await
            .unwrap_err(),
        VfsError::NotFound
    );
    assert_eq!(
        provider
            .list(&ItemId("/data/notes.txt".into()))
            .await
            .unwrap_err(),
        VfsError::NotAFolder
    );
    assert_eq!(
        provider
            .read(&ItemId("/data/sub".into()), 0, 8)
            .await
            .unwrap_err(),
        VfsError::IsAFolder
    );
}

#[tokio::test]
async fn a_changed_password_works_after_a_refusal() {
    let (server, _dir, provider) = mounted("wrong-password");
    assert_eq!(
        provider.list(&ItemId("/".into())).await.unwrap_err(),
        VfsError::SignInNeeded
    );
    marlin_test_keychain::set("marlin-smb", &server.host(), PASSWORD);
    let listed = provider.list(&ItemId("/".into())).await.unwrap();
    assert!(listed.iter().any(|item| item.id.0 == "/data"));
    assert!(listed.iter().any(|item| item.id.0 == "/denied"));
}

#[tokio::test]
async fn space_reports_a_size() {
    let (server, _dir, provider) = mounted(PASSWORD);
    let space = provider.space().await.unwrap();
    assert!(space.free <= space.total);

    // The share is a local folder, so its size is the size of the disk that holds it, not the 1 PiB a mount answers when no share reports one.
    let path = std::ffi::CString::new(server.path("").as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: statvfs is plain integers, so all zeros is a valid value.
    let mut local: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are to locals that outlive the call.
    assert_eq!(unsafe { libc::statvfs(path.as_ptr(), &mut local) }, 0);
    let local_total = local.f_blocks as u64 * local.f_frsize as u64;
    let local_free = local.f_bavail as u64 * local.f_frsize as u64;
    assert!(
        space.total.abs_diff(local_total) <= local_total / 100,
        "the mount reports {} bytes, the disk {local_total}",
        space.total
    );
    // Other programs write to the disk meanwhile, so free space gets the same 1% of the total.
    assert!(
        space.free.abs_diff(local_free) <= local_total / 100,
        "the mount reports {} bytes free, the disk {local_free}",
        space.free
    );
}

#[tokio::test]
async fn a_stopped_server_is_offline() {
    let (server, _dir, provider) = mounted(PASSWORD);
    provider.list(&ItemId("/".into())).await.unwrap();
    drop(server);
    assert_eq!(
        provider.list(&ItemId("/data".into())).await.unwrap_err(),
        VfsError::Offline
    );
}

fn local_file(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("upload");
    std::fs::write(&path, bytes).unwrap();
    (dir, path)
}

fn whole_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn assert_matches_disk(server: &Server, name: &str, item: &marlin_vfs::Item, bytes: &[u8]) {
    let path = server.path(name);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let on_disk = std::fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(
        whole_seconds(item.attributes.modified),
        whole_seconds(on_disk)
    );
}

fn assert_no_marlin_names(dir: &Path) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        assert!(
            !name.to_string_lossy().starts_with(".marlin-"),
            "{}",
            name.to_string_lossy()
        );
        if entry.file_type().unwrap().is_dir() {
            assert_no_marlin_names(&entry.path());
        }
    }
}

/// Whether the helper reads a file's access from the test server, which is not on port 445. libsmbclient before 4.23 reads access only on port 445, so with it the helper refuses a replace before anything is made.
fn server_reads_access() -> bool {
    // SAFETY: libsmbclient returns a static string, such as `4.17.12-Debian`.
    let version = unsafe { std::ffi::CStr::from_ptr(pavao_sys::smbc_version()) }.to_string_lossy();
    let mut numbers = version
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0));
    (numbers.next().unwrap_or(0), numbers.next().unwrap_or(0)) >= (4, 23)
}

#[tokio::test]
async fn samba_makes_moves_and_removes_folders_and_files() {
    let (server, _dir, provider) = mounted(PASSWORD);
    let modified = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let folder = provider
        .create_folder(&ItemId("/data".into()), "box")
        .await
        .unwrap();
    assert_eq!(folder.id, ItemId("/data/box".into()));
    assert!(server.path("box").is_dir());
    assert_eq!(
        provider
            .create_folder(&ItemId("/data".into()), "box")
            .await
            .unwrap_err(),
        VfsError::Conflict
    );

    let bytes = b"hello";
    let (_local, path) = local_file(bytes);
    let file = provider
        .create_file(&folder.id, "note.txt", &path, modified, &|_| {})
        .await
        .unwrap();
    assert_eq!(file.id, ItemId("/data/box/note.txt".into()));
    assert_matches_disk(&server, "box/note.txt", &file, bytes);
    assert_eq!(
        provider
            .create_file(&folder.id, "note.txt", &path, modified, &|_| {})
            .await
            .unwrap_err(),
        VfsError::Conflict
    );

    let renamed = provider
        .rename(&file.id, &folder.id, &folder.id, "other.txt", None)
        .await
        .unwrap();
    assert_eq!(renamed.id, ItemId("/data/box/other.txt".into()));
    assert!(!server.path("box/note.txt").exists());

    let other = provider
        .create_folder(&ItemId("/data".into()), "other")
        .await
        .unwrap();
    let moved = provider
        .rename(&renamed.id, &folder.id, &other.id, "other.txt", None)
        .await
        .unwrap();
    assert_eq!(moved.id, ItemId("/data/other/other.txt".into()));

    let (_kept_dir, kept_path) = local_file(b"OLD");
    let kept = provider
        .create_file(&other.id, "kept.txt", &kept_path, modified, &|_| {})
        .await
        .unwrap();
    assert_eq!(
        provider
            .rename(&moved.id, &other.id, &other.id, "kept.txt", None)
            .await
            .unwrap_err(),
        VfsError::Conflict
    );
    assert_eq!(
        std::fs::read(server.path("other/kept.txt")).unwrap(),
        b"OLD"
    );
    let replaced = provider
        .rename(&moved.id, &other.id, &other.id, "kept.txt", Some(&kept.id))
        .await
        .unwrap();
    assert_eq!(replaced.id, kept.id);
    assert!(!server.path("other/other.txt").exists());
    assert_eq!(std::fs::read(server.path("other/kept.txt")).unwrap(), bytes);

    assert_eq!(
        provider
            .rename(
                &folder.id,
                &ItemId("/data".into()),
                &folder.id,
                "inner",
                None
            )
            .await
            .unwrap_err(),
        VfsError::NotSupported
    );
    assert_eq!(
        provider
            .rename(
                &replaced.id,
                &other.id,
                &ItemId(format!("/{DENIED_SHARE}")),
                "x.txt",
                None,
            )
            .await
            .unwrap_err(),
        VfsError::NotSupported
    );
    assert_eq!(
        provider
            .rename(&replaced.id, &other.id, &ItemId("/".into()), "x.txt", None)
            .await
            .unwrap_err(),
        VfsError::NotSupported
    );
    assert_eq!(
        provider
            .create_folder(&ItemId("/data".into()), "a?b")
            .await
            .unwrap_err(),
        VfsError::NotSupported
    );
    assert_eq!(
        provider
            .create_file(&ItemId("/data".into()), "a:b", &path, modified, &|_| {})
            .await
            .unwrap_err(),
        VfsError::NotSupported
    );

    provider.remove(&replaced.id, &other.id).await.unwrap();
    assert!(!server.path("other/kept.txt").exists());
    let (_stay_dir, stay_path) = local_file(b"stay");
    let stay = provider
        .create_file(&folder.id, "stay.txt", &stay_path, modified, &|_| {})
        .await
        .unwrap();
    assert_eq!(
        provider
            .remove(&folder.id, &ItemId("/data".into()))
            .await
            .unwrap_err(),
        VfsError::NotEmpty
    );
    provider.remove(&stay.id, &folder.id).await.unwrap();
    provider
        .remove(&folder.id, &ItemId("/data".into()))
        .await
        .unwrap();
    assert!(!server.path("box").exists());
    assert_no_marlin_names(&server.path(""));
}

#[tokio::test]
async fn samba_replaces_bytes_only_at_the_base_revision() {
    let (server, _dir, provider) = mounted(PASSWORD);
    let modified = UNIX_EPOCH + Duration::from_secs(1_700_000_100);
    let original = vec![1u8; 2 * 1024 * 1024];
    let (_dir, path) = local_file(&original);
    let item = provider
        .create_file(
            &ItemId("/data".into()),
            "note.bin",
            &path,
            modified,
            &|_| {},
        )
        .await
        .unwrap();
    let base = item.attributes.revision.clone().unwrap();
    let replacement = vec![2u8; original.len()];
    let (_dir, next) = local_file(&replacement);
    let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
    assert_eq!(
        provider
            .replace_bytes(&item.id, &next, &Revision("0:1".into()), modified, &{
                let heard = Arc::clone(&heard);
                move |bytes| heard.lock().unwrap().push(bytes)
            },)
            .await
            .unwrap_err(),
        VfsError::Conflict
    );
    assert!(heard.lock().unwrap().is_empty());
    assert_eq!(std::fs::read(server.path("note.bin")).unwrap(), original);

    let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
    // The same size, so only the later time tells the two revisions apart.
    let later = modified + Duration::from_secs(3600);
    let replaced = provider
        .replace_bytes(&item.id, &next, &base, later, &{
            let heard = Arc::clone(&heard);
            move |bytes| heard.lock().unwrap().push(bytes)
        })
        .await;
    if server_reads_access() {
        let replaced = replaced.unwrap();
        assert_eq!(std::fs::read(server.path("note.bin")).unwrap(), replacement);
        let heard = heard.lock().unwrap();
        assert!(heard.windows(2).all(|pair| pair[0] <= pair[1]), "{heard:?}");
        assert_eq!(heard.last().copied(), Some(replacement.len() as u64));
        assert_eq!(
            replaced.attributes.revision,
            Some(Revision(format!("{}:1700003700", replacement.len())))
        );
    } else {
        // libsmbclient before 4.23 reads access only on port 445, and this server is not on 445, so the replace is refused.
        assert_eq!(replaced.unwrap_err(), VfsError::AccessDenied);
        assert_eq!(std::fs::read(server.path("note.bin")).unwrap(), original);
    }
    assert_no_marlin_names(&server.path(""));
}

#[tokio::test]
async fn a_stopped_upload_leaves_no_temporary_file() {
    let (server, _dir, provider) = mounted(PASSWORD);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sparse");
    let file = File::create(&path).unwrap();
    file.set_len(64 * 1024 * 1024).unwrap();
    drop(file);

    let provider = Arc::new(provider);
    let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let upload = tokio::spawn({
        let provider = Arc::clone(&provider);
        let seen = Arc::clone(&seen);
        let path = path.clone();
        async move {
            let _ = provider
                .create_file(
                    &ItemId("/data".into()),
                    "big.bin",
                    &path,
                    UNIX_EPOCH,
                    &move |_| seen.store(true, std::sync::atomic::Ordering::Relaxed),
                )
                .await;
        }
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    while !seen.load(std::sync::atomic::Ordering::Relaxed) {
        assert!(Instant::now() < deadline, "the upload sent no progress");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    upload.abort();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let left = std::fs::read_dir(server.path("")).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".marlin-")
        });
        if !left {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "a temporary file stayed in the share"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!server.path("big.bin").exists());
}
