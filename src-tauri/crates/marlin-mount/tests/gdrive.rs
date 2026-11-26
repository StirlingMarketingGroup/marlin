//! The helper with its real providers against a fake Google (`marlin-test-google`: the Drive API and the token endpoint on one `wiremock` server) and the in-memory keychain. The tests that mount need FUSE. `crates/marlin-fuse/test-in-docker.sh` runs them on Linux, and `crates/marlin-fuse/test-on-mac.sh` runs them on a Mac.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use marlin_gdrive::{OAuthClient, KEYRING_SERVICE};
use marlin_mount::protocol::MountState;
use marlin_mount::protocol::{error_codes, MountStatus, ReadOnly};
use marlin_mount::{Config, LinkedProviders, OsAdapter, Server, SmbConfig};
use marlin_mount_client::Event;
use marlin_mount_client::{Client, ClientError, Start};
use marlin_test_google::{pattern, Drive};
use marlin_vfs::Options;
use serde_json::{json, Value};
use wiremock::Request;

struct Running {
    root: PathBuf,
    client: Client,
    /// Only the tests that mount read events.
    events: tokio::sync::mpsc::UnboundedReceiver<Event>,
    signal: tokio::sync::oneshot::Sender<()>,
    run: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

async fn running_with(
    api: String,
    oauth_client: impl Fn() -> Result<OAuthClient, String> + Send + Sync + 'static,
) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("mount.sock");
    let root = dir.path().join("Marlin");
    let server = Server::bind(
        Config {
            socket: socket.clone(),
            root: root.clone(),
            spool: dir.path().join("spool"),
            options: Options::default(),
        },
        Arc::new(OsAdapter),
        Arc::new(LinkedProviders::new(
            marlin_gdrive::Config {
                upload: format!("{api}/upload"),
                api,
                retry_delay: Duration::from_millis(1),
            },
            oauth_client,
            marlin_sftp::Config {
                servers: dir.path().join("sftp-servers.json"),
                known_hosts: dir.path().join("known_hosts"),
                retry_after: Duration::from_secs(60),
            },
            SmbConfig {
                binary: None,
                servers: dir.path().join("smb-servers.json"),
            },
        )),
    )
    .unwrap();
    let (signal, signaled) = tokio::sync::oneshot::channel();
    let run = tokio::spawn(server.run(async move {
        let _ = signaled.await;
    }));
    let (sender, events) = tokio::sync::mpsc::unbounded_channel();
    let client = Client::connect(
        &Start {
            binary: dir.path().join("not-started"),
            socket,
            root: None,
            log: dir.path().join("helper.log"),
        },
        move |event| {
            let _ = sender.send(event);
        },
    )
    .await
    .unwrap();
    Running {
        root,
        client,
        events,
        signal,
        run,
        _dir: dir,
    }
}

fn oauth(uri: &str) -> impl Fn() -> Result<OAuthClient, String> + Send + Sync + 'static {
    let uri = uri.to_string();
    move || {
        Ok(OAuthClient {
            client_id: "client-id".to_string(),
            client_secret: "client-secret".to_string(),
            token_uri: format!("{uri}/token"),
        })
    }
}

async fn stop(running: Running) {
    let Running { signal, run, .. } = running;
    let _ = signal.send(());
    run.await.unwrap();
}

fn rpc(error: ClientError) -> (i32, String) {
    match error {
        ClientError::Rpc(error) => (error.code, error.message),
        other => panic!("expected a helper error, got {other}"),
    }
}

async fn mount_gdrive(
    client: &Client,
    email: &str,
    name: &str,
) -> Result<MountStatus, ClientError> {
    client
        .call(
            "mount",
            json!({"provider": "gdrive", "account": email, "name": name}),
        )
        .await
}

#[tokio::test]
async fn an_account_whose_sign_in_is_gone_is_refused_and_leaves_no_folder() {
    marlin_test_keychain::install();
    let (server, google) = marlin_test_google::start().await;
    let uri = server.uri();
    let email = "gone@example.com";
    let running = running_with(uri.clone(), oauth(&uri)).await;

    let (code, message) = rpc(mount_gdrive(&running.client, email, "gone")
        .await
        .unwrap_err());
    assert_eq!(code, error_codes::SIGN_IN_NEEDED);
    assert_eq!(message, "the account needs a new sign-in");
    assert!(!running.root.join("gone").exists());

    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-1");
    google.lock().unwrap().rejected = true;
    let (code, message) = rpc(mount_gdrive(&running.client, email, "gone")
        .await
        .unwrap_err());
    assert_eq!(code, error_codes::SIGN_IN_NEEDED);
    assert_eq!(message, "the account needs a new sign-in");
    assert!(!running.root.join("gone").exists());

    stop(running).await;
}

#[tokio::test]
async fn a_mount_is_refused_while_google_is_out_of_reach() {
    marlin_test_keychain::install();
    let (server, _google) = marlin_test_google::start().await;
    let uri = server.uri();
    // Nothing listens on port 1.
    let nowhere = "http://127.0.0.1:1".to_string();

    let email = "no-token-endpoint@example.com";
    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-1");
    let running = running_with(uri.clone(), oauth(&nowhere)).await;
    let (code, message) = rpc(mount_gdrive(&running.client, email, "drive")
        .await
        .unwrap_err());
    assert_eq!(code, error_codes::UNREACHABLE);
    assert_eq!(message, "the account's storage could not be reached");
    assert!(!running.root.join("drive").exists());
    stop(running).await;

    // A token alone is not enough: the Drive API has to answer too.
    let email = "no-drive-api@example.com";
    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-1");
    let running = running_with(nowhere, oauth(&uri)).await;
    let (code, message) = rpc(mount_gdrive(&running.client, email, "drive")
        .await
        .unwrap_err());
    assert_eq!(code, error_codes::UNREACHABLE);
    assert_eq!(message, "the account's storage could not be reached");
    assert!(!running.root.join("drive").exists());
    stop(running).await;
}

#[tokio::test]
async fn a_drive_api_that_answers_503_or_403_refuses_the_mount() {
    marlin_test_keychain::install();
    let (server, google) = marlin_test_google::start().await;
    let uri = server.uri();
    let email = "drive-api-down@example.com";
    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-1");
    google.lock().unwrap().drive = Drive::Down;
    let running = running_with(uri.clone(), oauth(&uri)).await;
    let (code, message) = rpc(mount_gdrive(&running.client, email, "drive")
        .await
        .unwrap_err());
    assert_eq!(code, error_codes::PROVIDER);
    assert_eq!(message, "Google Drive answered 503 Service Unavailable");
    assert!(!running.root.join("drive").exists());

    google.lock().unwrap().drive = Drive::Forbidden;
    let (code, message) = rpc(mount_gdrive(&running.client, email, "drive")
        .await
        .unwrap_err());
    assert_eq!(code, error_codes::PROVIDER);
    assert_eq!(message, "the account's storage does not let Marlin read it");
    assert!(!running.root.join("drive").exists());
    stop(running).await;
}

#[tokio::test]
async fn a_helper_with_no_oauth_client_refuses_the_mount_and_keeps_the_loaders_text_out() {
    marlin_test_keychain::install();
    let (server, _google) = marlin_test_google::start().await;
    let email = "noclient@example.com";
    let running = running_with(server.uri(), || {
        Err("Failed to read /secret/place/google-credentials.json".into())
    })
    .await;
    let (code, message) = rpc(mount_gdrive(&running.client, email, "drive")
        .await
        .unwrap_err());
    assert_eq!(code, error_codes::PROVIDER);
    assert_eq!(message, "this build of Marlin has no Google OAuth client");
    assert!(!running.root.join("drive").exists());
    stop(running).await;
}

async fn names(path: &std::path::Path) -> Vec<String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(path).unwrap() {
            names.push(entry.unwrap().file_name().to_string_lossy().into_owned());
        }
        names.sort();
        names
    })
    .await
    .unwrap()
}

async fn read_file(path: &std::path::Path) -> Vec<u8> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::read(path).unwrap())
        .await
        .unwrap()
}

async fn read_at(path: &std::path::Path, offset: u64, size: usize) -> Vec<u8> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(path).unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();
        let mut buf = vec![0; size];
        file.read_exact(&mut buf).unwrap();
        buf
    })
    .await
    .unwrap()
}

/// Makes one provider call that no cache answers, and ignores the result. It reads a file that nothing has read yet. A free-space call would not do: macOS asks for the space when it mounts and answers later calls from that.
async fn read_big_file(point: &std::path::Path) {
    let big = point.join("My Drive").join("big.bin");
    tokio::task::spawn_blocking(move || {
        let _ = std::fs::read(big);
    })
    .await
    .unwrap();
}

async fn changed(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    state: MountState,
) -> MountStatus {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match events.recv().await {
                Some(Event::Changed(status)) if status.state == state => return status,
                Some(Event::Changed(_) | Event::Unsent(_)) => {}
                Some(Event::Closed) | None => panic!("the helper closed"),
            }
        }
    })
    .await
    .expect("no mount event")
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_google_account_mounts_and_reads_through_fuse() {
    marlin_test_keychain::install();
    let (server, google) = marlin_test_google::start().await;
    let uri = server.uri();
    let email = "fuse@example.com";
    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-1");
    let running = running_with(uri.clone(), oauth(&uri)).await;

    // The fake Google takes no change but a new file's upload, so this test keeps the mount read-only through the "spool folder cannot be made" rule, and checks that rule through a real mount.
    let spool = running.root.parent().unwrap().join("spool");
    std::fs::write(&spool, b"not a folder").unwrap();
    let status = mount_gdrive(&running.client, email, "drive").await.unwrap();
    assert_eq!(status.state, MountState::Mounted);
    assert!(matches!(status.read_only, Some(ReadOnly::Spool { .. })));
    let point = PathBuf::from(&status.mount_point);
    assert_eq!(
        names(&point).await,
        ["My Drive".to_string(), "Shared drives".to_string()]
    );
    let my_drive = point.join("My Drive");
    assert_eq!(
        names(&my_drive).await,
        [
            "Notes.document.html".to_string(),
            "a%2Fb.txt".to_string(),
            "big.bin".to_string(),
            "same (2).txt".to_string(),
            "same.txt".to_string(),
        ]
    );
    let notes_path = my_drive.join("Notes.document.html");
    let notes = read_file(&notes_path).await;
    assert!(String::from_utf8(notes.clone())
        .unwrap()
        .contains("https://drive.google.com/open?id=doc"));
    let notes_len =
        tokio::task::spawn_blocking(move || std::fs::metadata(notes_path).unwrap().len())
            .await
            .unwrap();
    assert_eq!(notes_len, notes.len() as u64);
    let big = pattern(300_000);
    let big_path = my_drive.join("big.bin");
    assert_eq!(read_file(&big_path).await, big);
    assert_eq!(
        read_at(&big_path, 200_000, 1000).await,
        big[200_000..201_000]
    );

    let media = |requests: &[Request]| {
        requests
            .iter()
            .filter(|request| {
                request
                    .url
                    .query_pairs()
                    .any(|(key, value)| key == "alt" && value == "media")
            })
            .count()
    };
    let before = media(&server.received_requests().await.unwrap());
    assert_eq!(read_file(&big_path).await, big);
    assert_eq!(media(&server.received_requests().await.unwrap()), before);

    let new_file = my_drive.join("new.txt");
    let created = tokio::task::spawn_blocking(move || std::fs::write(new_file, b"x"))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(created.raw_os_error(), Some(libc::EROFS));
    let removed = tokio::task::spawn_blocking({
        let path = big_path.clone();
        move || std::fs::remove_file(path)
    })
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(removed.raw_os_error(), Some(libc::EROFS));
    let renamed = tokio::task::spawn_blocking({
        let from = big_path.clone();
        let to = my_drive.join("other.bin");
        move || std::fs::rename(from, to)
    })
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(renamed.raw_os_error(), Some(libc::EROFS));

    running
        .client
        .call::<Value>("unmount", json!({"provider": "gdrive", "account": email}))
        .await
        .unwrap();
    // The saved sign-in still holds a token, and the mount asks Google all the same.
    google.lock().unwrap().drive = Drive::Down;
    let (code, _) = rpc(mount_gdrive(&running.client, email, "drive")
        .await
        .unwrap_err());
    assert_eq!(code, error_codes::PROVIDER);
    assert!(!point.exists());
    google.lock().unwrap().drive = Drive::Up;
    let again = mount_gdrive(&running.client, email, "drive").await.unwrap();
    assert_eq!(again.state, MountState::Mounted);
    running
        .client
        .call::<Value>("unmount", json!({"provider": "gdrive", "account": email}))
        .await
        .unwrap();
    assert!(!point.exists());

    let requests = server.received_requests().await.unwrap();
    let posts: Vec<_> = requests
        .iter()
        .filter(|request| request.method.as_str() == "POST")
        .collect();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].url.path(), "/token");
    assert!(requests
        .iter()
        .all(|request| request.method.as_str() == "POST" || request.method.as_str() == "GET"));

    stop(running).await;
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_sign_in_shows_auth_required_and_a_new_sign_in_recovers() {
    marlin_test_keychain::install();
    let (server, google) = marlin_test_google::start().await;
    let uri = server.uri();
    let email = "recover@example.com";
    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-1");
    let mut running = running_with(uri.clone(), oauth(&uri)).await;

    let status = mount_gdrive(&running.client, email, "drive").await.unwrap();
    assert_eq!(status.state, MountState::Mounted);
    assert_eq!(status.read_only, None);
    let point = PathBuf::from(status.mount_point);

    {
        let mut google = google.lock().unwrap();
        google.rejected = true;
        google.access_token = "access-2".to_string();
    }
    read_big_file(&point).await;
    let failed = changed(&mut running.events, MountState::AuthRequired).await;
    assert_eq!(failed.account, email);

    {
        let mut google = google.lock().unwrap();
        google.rejected = false;
        // Google now knows the new sign-in.
        google.refresh_token = "refresh-2".to_string();
    }
    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-2");
    read_big_file(&point).await;
    let recovered = changed(&mut running.events, MountState::Mounted).await;
    assert_eq!(recovered.account, email);

    running
        .client
        .call::<Value>("unmount", json!({"provider": "gdrive", "account": email}))
        .await
        .unwrap();
    stop(running).await;
}
