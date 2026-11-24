use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use marlin_smb_client::protocol::READ_MAX;
use marlin_smb_client::{add_server, SmbProvider, SmbSignIn};
use marlin_test_samba::{Server, PASSWORD};
use marlin_vfs::{ItemId, Kind, Provider, VfsError};

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
