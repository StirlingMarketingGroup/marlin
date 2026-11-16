//! The helper with its real providers against a fake Google: the Drive API and the token endpoint on one `wiremock` server, and the in-memory keychain. The tests that mount need FUSE. `crates/marlin-fuse/test-in-docker.sh` runs them on Linux, and `crates/marlin-fuse/test-on-mac.sh` runs them on a Mac.

#![cfg(unix)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use marlin_gdrive::{OAuthClient, KEYRING_SERVICE};
use marlin_mount::protocol::MountState;
use marlin_mount::protocol::{error_codes, MountStatus};
use marlin_mount::{Config, LinkedProviders, OsAdapter, Server};
use marlin_mount_client::Event;
use marlin_mount_client::{Client, ClientError, Start};
use serde_json::{json, Value};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const MODIFIED: &str = "2020-01-02T03:04:05.678Z";

struct FakeFile {
    id: &'static str,
    name: &'static str,
    mime: &'static str,
    bytes: Vec<u8>,
}

struct GoogleState {
    access_token: String,
    rejected: bool,
    /// While set, the Drive API answers 503. The token endpoint still works.
    drive_down: bool,
    /// While set, the Drive API answers 403 with a reason that is not a rate limit.
    forbidden: bool,
    files: Vec<FakeFile>,
}

impl GoogleState {
    fn new() -> Self {
        Self {
            access_token: "access-1".to_string(),
            rejected: false,
            drive_down: false,
            forbidden: false,
            files: vec![
                FakeFile {
                    id: "big",
                    name: "big.bin",
                    mime: "application/octet-stream",
                    bytes: pattern(300_000),
                },
                FakeFile {
                    id: "same-1",
                    name: "same.txt",
                    mime: "application/octet-stream",
                    bytes: b"one".to_vec(),
                },
                FakeFile {
                    id: "same-2",
                    name: "same.txt",
                    mime: "application/octet-stream",
                    bytes: b"two".to_vec(),
                },
                FakeFile {
                    id: "slash",
                    name: "a/b.txt",
                    mime: "application/octet-stream",
                    bytes: b"slash".to_vec(),
                },
                FakeFile {
                    id: "doc",
                    name: "Notes",
                    mime: "application/vnd.google-apps.document",
                    bytes: Vec::new(),
                },
            ],
        }
    }
}

struct FakeGoogle {
    state: Arc<Mutex<GoogleState>>,
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index % 251) as u8).collect()
}

fn file_json(file: &FakeFile) -> Value {
    json!({
        "id": file.id,
        "name": file.name,
        "mimeType": file.mime,
        "size": file.bytes.len().to_string(),
        "modifiedTime": MODIFIED,
        "trashed": false,
    })
}

impl Respond for FakeGoogle {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let path = request.url.path();
        let method = request.method.as_str();
        let state = self.state.lock().unwrap();
        if method == "POST" && path == "/token" {
            if state.rejected {
                return ResponseTemplate::new(400).set_body_json(json!({"error": "invalid_grant"}));
            }
            return ResponseTemplate::new(200).set_body_json(json!({
                "access_token": state.access_token,
                "expires_in": 3600,
            }));
        }
        let authorization = request
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if authorization != format!("Bearer {}", state.access_token) {
            return ResponseTemplate::new(401);
        }
        if state.drive_down {
            return ResponseTemplate::new(503);
        }
        if state.forbidden {
            return ResponseTemplate::new(403)
                .set_body_json(json!({"error": {"errors": [{"reason": "domainPolicy"}]}}));
        }
        if method != "GET" {
            return ResponseTemplate::new(404);
        }
        if path == "/drives" {
            return ResponseTemplate::new(200).set_body_json(json!({"drives": []}));
        }
        if path == "/about" {
            return ResponseTemplate::new(200).set_body_json(json!({
                "storageQuota": {"limit": "1000", "usage": "400"}
            }));
        }
        let query: HashMap<String, String> = request
            .url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        if path == "/files" {
            let q = query.get("q").map(String::as_str).unwrap_or("");
            if !q.contains("'root' in parents") {
                return ResponseTemplate::new(200).set_body_json(json!({"files": []}));
            }
            let (shown, next) = if query.get("pageToken").map(String::as_str) == Some("page-2") {
                (&state.files[2..], None)
            } else {
                (&state.files[..2], Some("page-2"))
            };
            let mut body = json!({
                "files": shown.iter().map(file_json).collect::<Vec<_>>(),
            });
            if let Some(next) = next {
                body["nextPageToken"] = json!(next);
            }
            return ResponseTemplate::new(200).set_body_json(body);
        }
        let Some(id) = path.strip_prefix("/files/") else {
            return ResponseTemplate::new(404);
        };
        if id == "root" {
            return ResponseTemplate::new(200).set_body_json(json!({
                "id": "root",
                "name": "My Drive",
                "mimeType": "application/vnd.google-apps.folder",
                "modifiedTime": MODIFIED,
                "trashed": false,
            }));
        }
        let Some(file) = state.files.iter().find(|file| file.id == id) else {
            return ResponseTemplate::new(404);
        };
        if query.get("alt").map(String::as_str) != Some("media") {
            return ResponseTemplate::new(200).set_body_json(file_json(file));
        }
        if file.mime.starts_with("application/vnd.google-apps.") {
            return ResponseTemplate::new(403)
                .set_body_json(json!({"error": {"errors": [{"reason": "fileNotDownloadable"}]}}));
        }
        let range = request
            .headers
            .get("range")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        let Some(rest) = range.strip_prefix("bytes=") else {
            return ResponseTemplate::new(404);
        };
        let Some((start, end)) = rest.split_once('-') else {
            return ResponseTemplate::new(404);
        };
        let (Ok(start), Ok(end)) = (start.parse::<u64>(), end.parse::<u64>()) else {
            return ResponseTemplate::new(404);
        };
        let len = file.bytes.len() as u64;
        if start >= len {
            return ResponseTemplate::new(416);
        }
        let end = end.min(len - 1);
        ResponseTemplate::new(206)
            .set_body_bytes(file.bytes[start as usize..=end as usize].to_vec())
    }
}

async fn fake_google() -> (MockServer, Arc<Mutex<GoogleState>>) {
    let server = MockServer::start().await;
    let state = Arc::new(Mutex::new(GoogleState::new()));
    Mock::given(any())
        .respond_with(FakeGoogle {
            state: Arc::clone(&state),
        })
        .mount(&server)
        .await;
    (server, state)
}

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
        },
        Arc::new(OsAdapter),
        Arc::new(LinkedProviders::new(
            marlin_gdrive::Config {
                api,
                retry_delay: Duration::from_millis(1),
            },
            oauth_client,
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
    let (server, google) = fake_google().await;
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
    let (server, _google) = fake_google().await;
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
    let (server, google) = fake_google().await;
    let uri = server.uri();
    let email = "drive-api-down@example.com";
    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-1");
    google.lock().unwrap().drive_down = true;
    let running = running_with(uri.clone(), oauth(&uri)).await;
    let (code, message) = rpc(mount_gdrive(&running.client, email, "drive")
        .await
        .unwrap_err());
    assert_eq!(code, error_codes::PROVIDER);
    assert_eq!(message, "Google Drive answered 503 Service Unavailable");
    assert!(!running.root.join("drive").exists());

    // A 403 that is not a rate limit, such as a Workspace policy that blocks the app.
    {
        let mut google = google.lock().unwrap();
        google.drive_down = false;
        google.forbidden = true;
    }
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
    let (server, _google) = fake_google().await;
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
                Some(Event::Changed(_)) => {}
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
    let (server, google) = fake_google().await;
    let uri = server.uri();
    let email = "fuse@example.com";
    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-1");
    let running = running_with(uri.clone(), oauth(&uri)).await;

    let status = mount_gdrive(&running.client, email, "drive").await.unwrap();
    assert_eq!(status.state, MountState::Mounted);
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
    google.lock().unwrap().drive_down = true;
    let (code, _) = rpc(mount_gdrive(&running.client, email, "drive")
        .await
        .unwrap_err());
    assert_eq!(code, error_codes::PROVIDER);
    assert!(!point.exists());
    google.lock().unwrap().drive_down = false;
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
    let (server, google) = fake_google().await;
    let uri = server.uri();
    let email = "recover@example.com";
    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-1");
    let mut running = running_with(uri.clone(), oauth(&uri)).await;

    let status = mount_gdrive(&running.client, email, "drive").await.unwrap();
    assert_eq!(status.state, MountState::Mounted);
    let point = PathBuf::from(status.mount_point);

    {
        let mut google = google.lock().unwrap();
        google.rejected = true;
        google.access_token = "access-2".to_string();
    }
    read_big_file(&point).await;
    let failed = changed(&mut running.events, MountState::AuthRequired).await;
    assert_eq!(failed.account, email);

    google.lock().unwrap().rejected = false;
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
