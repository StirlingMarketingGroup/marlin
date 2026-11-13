#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use marlin_mount::protocol::{
    error_codes, methods, AccountParams, MountParams, MountState, MountStatus, PROTOCOL_VERSION,
};
use marlin_mount_client::{Client, ClientError, Event, Start};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};

/// Stops a helper the test started, including when an assertion fails first.
struct Stop(PathBuf);

impl Drop for Stop {
    fn drop(&mut self) {
        let socket = self.0.clone();
        let _ = std::thread::Builder::new()
            .name("stop-helper".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(async {
                    let _ = tokio::time::timeout(
                        Duration::from_secs(5),
                        marlin_mount_client::shutdown(&socket),
                    )
                    .await;
                });
            })
            .unwrap()
            .join();
    }
}

fn helper_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_marlin-mount"))
}

fn start_for(dir: &Path, binary: PathBuf) -> Start {
    Start {
        binary,
        socket: dir.join("mount.sock"),
        root: Some(dir.join("Marlin")),
        log: dir.join("helper.log"),
    }
}

fn watch() -> (
    impl Fn(Event) + Send + 'static,
    tokio::sync::mpsc::UnboundedReceiver<Event>,
) {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    (
        move |event| {
            let _ = sender.send(event);
        },
        receiver,
    )
}

async fn read_json(reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>) -> Value {
    let mut line = String::new();
    let read = reader.read_line(&mut line).await.unwrap();
    assert!(read > 0, "the client closed before the fake helper read");
    serde_json::from_str(&line).expect(&line)
}

async fn write_json(writer: &mut OwnedWriteHalf, value: &Value) {
    let mut line = serde_json::to_string(value).unwrap();
    line.push('\n');
    writer.write_all(line.as_bytes()).await.unwrap();
}

fn answered(id: u64, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

#[tokio::test]
async fn the_first_connect_starts_the_helper_and_a_second_connect_reuses_it() {
    let dir = tempfile::tempdir().unwrap();
    let start = start_for(dir.path(), helper_binary());
    let _stop = Stop(start.socket.clone());
    let first = Client::connect(&start, |_| {}).await.unwrap();

    let reused = Start {
        binary: dir.path().join("missing-binary"),
        ..start
    };
    let second = Client::connect(&reused, |_| {}).await.unwrap();
    for client in [first, second] {
        let list: Vec<MountStatus> = client.call(methods::LIST, ()).await.unwrap();
        assert!(list.is_empty());
    }
}

#[tokio::test]
async fn the_helper_keeps_running_after_its_client_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let start = start_for(dir.path(), helper_binary());
    let _stop = Stop(start.socket.clone());
    drop(Client::connect(&start, |_| {}).await.unwrap());

    let reused = Start {
        binary: dir.path().join("missing-binary"),
        ..start
    };
    let client = Client::connect(&reused, |_| {}).await.unwrap();
    let list: Vec<MountStatus> = client.call(methods::LIST, ()).await.unwrap();
    assert!(list.is_empty());
}

#[tokio::test]
async fn shutdown_stops_the_helper_and_removes_the_socket_and_returns_false_when_no_helper_runs() {
    let dir = tempfile::tempdir().unwrap();
    let start = start_for(dir.path(), helper_binary());
    let socket = start.socket.clone();
    assert!(!marlin_mount_client::shutdown(&socket).await.unwrap());

    drop(Client::connect(&start, |_| {}).await.unwrap());
    assert!(marlin_mount_client::shutdown(&socket).await.unwrap());
    assert!(!socket.exists());
    assert!(!marlin_mount_client::shutdown(&socket).await.unwrap());
}

#[tokio::test]
async fn a_helper_with_another_protocol_version_is_shut_down_and_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let start = start_for(dir.path(), helper_binary());
    let _stop = Stop(start.socket.clone());
    let socket = start.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let fake = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let hello = read_json(&mut reader).await;
        write_json(
            &mut writer,
            &answered(
                hello["id"].as_u64().unwrap(),
                json!({"protocol": PROTOCOL_VERSION + 1, "version": "fake"}),
            ),
        )
        .await;
        let shutdown = read_json(&mut reader).await;
        write_json(
            &mut writer,
            &answered(shutdown["id"].as_u64().unwrap(), Value::Null),
        )
        .await;
        std::fs::remove_file(&socket).unwrap();
        drop(writer);
    });

    let client = Client::connect(&start, |_| {}).await.unwrap();
    fake.await.unwrap();
    let list: Vec<MountStatus> = client.call(methods::LIST, ()).await.unwrap();
    assert!(list.is_empty());
}

#[tokio::test]
async fn a_helper_with_another_protocol_version_that_refuses_to_shut_down_is_left_running_and_no_other_helper_is_started(
) {
    let dir = tempfile::tempdir().unwrap();
    let start = start_for(dir.path(), helper_binary());
    let socket = start.socket.clone();
    let _stop = Stop(socket.clone());
    let listener = UnixListener::bind(&socket).unwrap();
    let fake = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let hello = read_json(&mut reader).await;
        write_json(
            &mut writer,
            &answered(
                hello["id"].as_u64().unwrap(),
                json!({"protocol": PROTOCOL_VERSION + 1, "version": "fake"}),
            ),
        )
        .await;
        let shutdown = read_json(&mut reader).await;
        write_json(
            &mut writer,
            &json!({
                "jsonrpc": "2.0",
                "id": shutdown["id"].as_u64().unwrap(),
                "error": {"code": error_codes::UNMOUNT, "message": "could not unmount"}
            }),
        )
        .await;
        listener
    });

    let Err(error) = Client::connect(&start, |_| {}).await else {
        panic!("connect was expected to fail");
    };
    assert!(
        matches!(error, ClientError::OtherVersion(ref reason) if reason == "could not unmount"),
        "{error}"
    );
    let listener = fake.await.unwrap();
    // A new connection to the path still reaches this listener, so no helper replaced it.
    tokio::time::timeout(Duration::from_secs(2), async {
        let (incoming, connected) = tokio::join!(listener.accept(), UnixStream::connect(&socket));
        connected.unwrap();
        incoming.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_missing_binary_is_reported_and_nothing_is_started() {
    let dir = tempfile::tempdir().unwrap();
    let start = start_for(dir.path(), dir.path().join("missing-binary"));
    let Err(error) = Client::connect(&start, |_| {}).await else {
        panic!("connect was expected to fail");
    };
    assert!(
        matches!(error, ClientError::MissingBinary(ref path) if path == &start.binary),
        "{error}"
    );
    assert!(!start.socket.exists());
    assert!(!start.log.exists());
    assert!(!marlin_mount_client::shutdown(&start.socket).await.unwrap());
}

#[tokio::test]
async fn an_empty_placeholder_is_reported_as_a_missing_binary() {
    let dir = tempfile::tempdir().unwrap();
    let start = start_for(dir.path(), dir.path().join("marlin-mount"));
    std::fs::write(&start.binary, "").unwrap();
    let Err(error) = Client::connect(&start, |_| {}).await else {
        panic!("connect was expected to fail");
    };
    assert!(
        matches!(error, ClientError::MissingBinary(ref path) if path == &start.binary),
        "{error}"
    );
}

#[tokio::test]
async fn a_socket_that_cannot_be_opened_for_another_reason_starts_no_helper() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    let start = Start {
        socket: locked.join("mount.sock"),
        ..start_for(dir.path(), helper_binary())
    };
    // No search permission on the folder: the connect fails with "permission denied", which says nothing about a helper.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let connected = Client::connect(&start, |_| {}).await;
    let shutdown = marlin_mount_client::shutdown(&start.socket).await;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();

    let Err(error) = connected else {
        panic!("connect was expected to fail");
    };
    assert!(matches!(error, ClientError::Unreachable(_)), "{error}");
    assert!(
        matches!(shutdown, Err(ClientError::Unreachable(_))),
        "{shutdown:?}"
    );
    assert!(!start.log.exists());
}

#[tokio::test]
async fn shutdown_reads_past_notifications_to_its_answer() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("mount.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let fake = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let hello = read_json(&mut reader).await;
        write_json(
            &mut writer,
            &answered(
                hello["id"].as_u64().unwrap(),
                json!({"protocol": PROTOCOL_VERSION, "version": "fake"}),
            ),
        )
        .await;
        let shutdown = read_json(&mut reader).await;
        write_json(
            &mut writer,
            &json!({
                "jsonrpc": "2.0",
                "method": "mountChanged",
                "params": {"provider": "gdrive", "account": "a", "name": "a", "mountPoint": "/tmp/a", "state": "failed"}
            }),
        )
        .await;
        write_json(
            &mut writer,
            &answered(shutdown["id"].as_u64().unwrap(), Value::Null),
        )
        .await;
    });
    assert!(marlin_mount_client::shutdown(&socket).await.unwrap());
    fake.await.unwrap();
}

#[tokio::test]
async fn a_status_of_an_account_with_no_mount_answers_none() {
    let dir = tempfile::tempdir().unwrap();
    let start = start_for(dir.path(), helper_binary());
    let _stop = Stop(start.socket.clone());
    let client = Client::connect(&start, |_| {}).await.unwrap();
    let status: Option<MountStatus> = client
        .call(
            methods::STATUS,
            AccountParams {
                provider: "gdrive".to_string(),
                account: "a".to_string(),
            },
        )
        .await
        .unwrap();
    assert!(status.is_none());
}

#[tokio::test]
async fn a_mount_state_change_reaches_on_event() {
    let dir = tempfile::tempdir().unwrap();
    let start = start_for(dir.path(), helper_binary());
    let _stop = Stop(start.socket.clone());
    let (on_event, mut events) = watch();
    let client = Client::connect(&start, on_event).await.unwrap();
    let error = client
        .call::<MountStatus>(
            methods::MOUNT,
            MountParams {
                provider: "gdrive".to_string(),
                account: "a".to_string(),
                name: "a".to_string(),
            },
        )
        .await;
    let Err(error) = error else {
        panic!("mount was expected to fail");
    };
    assert!(
        matches!(&error, ClientError::Rpc(error) if error.code == error_codes::UNKNOWN_PROVIDER),
        "{error}"
    );
    // An unknown provider is refused after the mount is recorded, so the helper sends `starting` and then `failed`.
    let failed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await {
                Some(Event::Changed(status)) if status.state == MountState::Failed => {
                    return status
                }
                Some(_) => {}
                None => panic!("event channel closed"),
            }
        }
    })
    .await
    .expect("no failed mount event");
    assert_eq!(failed.provider, "gdrive");
    assert_eq!(failed.account, "a");
    assert_eq!(failed.name, "a");
    assert_eq!(
        failed.error.as_ref().map(|error| error.code),
        Some(error_codes::UNKNOWN_PROVIDER)
    );
}

#[tokio::test]
async fn the_client_learns_that_the_helper_has_gone() {
    let dir = tempfile::tempdir().unwrap();
    let start = start_for(dir.path(), helper_binary());
    let socket = start.socket.clone();
    let (on_event, mut events) = watch();
    let client = Client::connect(&start, on_event).await.unwrap();
    assert!(marlin_mount_client::shutdown(&socket).await.unwrap());

    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap();
    assert!(matches!(event, Some(Event::Closed)));
    assert!(client.is_closed());
    let started = std::time::Instant::now();
    let error = client.call::<Vec<MountStatus>>(methods::LIST, ()).await;
    let Err(error) = error else {
        panic!("the call was expected to fail");
    };
    assert!(matches!(error, ClientError::Closed), "{error}");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn the_helpers_log_lines_land_in_the_log_file() {
    let dir = tempfile::tempdir().unwrap();
    let start = start_for(dir.path(), helper_binary());
    let _stop = Stop(start.socket.clone());
    Client::connect(&start, |_| {}).await.unwrap();
    let log = std::fs::read_to_string(&start.log).unwrap();
    assert!(log.contains("helper.start"), "{log}");
}

#[tokio::test]
async fn a_log_over_the_size_limit_is_moved_aside_when_the_helper_starts() {
    let dir = tempfile::tempdir().unwrap();
    let start = start_for(dir.path(), helper_binary());
    let _stop = Stop(start.socket.clone());
    std::fs::write(&start.log, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    Client::connect(&start, |_| {}).await.unwrap();
    let old = std::fs::metadata(dir.path().join("helper.log.old")).unwrap();
    assert_eq!(old.len(), 1024 * 1024 + 1);
    let log = std::fs::read_to_string(&start.log).unwrap();
    assert!(log.starts_with('{'), "{log}");
}

#[tokio::test]
async fn the_helper_runs_in_its_own_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let start = start_for(dir.path(), helper_binary());
    let _stop = Stop(start.socket.clone());
    Client::connect(&start, |_| {}).await.unwrap();

    // Only the helper has the socket path on its command line.
    let listed = std::process::Command::new("pgrep")
        .arg("-f")
        .arg(&start.socket)
        .output()
        .unwrap();
    let helper: u32 = String::from_utf8(listed.stdout)
        .unwrap()
        .trim()
        .parse()
        .expect("one helper process");
    let helper_group = pgid(helper);
    let own_group = pgid(std::process::id());
    assert_ne!(helper_group, own_group);
}

fn pgid(pid: u32) -> String {
    let output = std::process::Command::new("ps")
        .args(["-o", "pgid=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}
