#![cfg(unix)]

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use marlin_mount::protocol::PROTOCOL_VERSION;
use marlin_mount::{Adapter, BindError, Config, MountError, Providers, Server, Session};
use marlin_vfs::{MemoryProvider, Provider, Vfs, VfsError};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::{oneshot, watch};

/// What the fake adapter counts and what a test can tell it to do.
#[derive(Default)]
struct Fake {
    /// Mount calls that have started.
    mounts: watch::Sender<usize>,

    /// Unmount calls that have started.
    unmounts: watch::Sender<usize>,

    /// While true, a mount call waits before it returns.
    hold_mount: watch::Sender<bool>,

    /// While true, an unmount call waits before it returns.
    hold_unmount: watch::Sender<bool>,

    /// The next mount call returns this error.
    fail_mount: Mutex<Option<MountError>>,

    /// While true, every unmount call fails.
    fail_unmount: AtomicBool,

    /// Mount points the fake treats as dead mounts.
    dead: Mutex<HashSet<PathBuf>>,

    /// While true, a dead mount cannot be cleared.
    stuck: AtomicBool,

    /// Each mount's core, keyed by the mount folder's name. A test makes a provider call through it.
    vfs: Mutex<HashMap<String, Arc<Vfs>>>,

    /// Each account's provider, keyed by account. A test sets its failures here.
    providers: Mutex<HashMap<String, Arc<MemoryProvider>>>,
}

async fn wait<T>(value: &watch::Sender<T>, check: impl FnMut(&T) -> bool) {
    value.subscribe().wait_for(check).await.unwrap();
}

struct FakeAdapter(Arc<Fake>);

#[async_trait::async_trait]
impl Adapter for FakeAdapter {
    async fn mount(
        &self,
        vfs: Arc<Vfs>,
        mount_point: &Path,
    ) -> Result<Box<dyn Session>, MountError> {
        let name = mount_point
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        self.0.vfs.lock().unwrap().insert(name, vfs);
        self.0.mounts.send_modify(|count| *count += 1);
        wait(&self.0.hold_mount, |hold| !hold).await;
        match self.0.fail_mount.lock().unwrap().take() {
            Some(error) => Err(error),
            None => Ok(Box::new(FakeSession(Arc::clone(&self.0)))),
        }
    }

    async fn is_dead_mount(&self, mount_point: &Path) -> bool {
        self.0.dead.lock().unwrap().contains(mount_point)
    }

    async fn unmount_dead(&self, mount_point: &Path) -> Result<(), MountError> {
        if self.0.stuck.load(Ordering::SeqCst) {
            return Err(MountError::DeadMount("busy".into()));
        }
        self.0.dead.lock().unwrap().remove(mount_point);
        Ok(())
    }
}

struct FakeSession(Arc<Fake>);

#[async_trait::async_trait]
impl Session for FakeSession {
    async fn unmount(&mut self) -> Result<(), MountError> {
        self.0.unmounts.send_modify(|count| *count += 1);
        wait(&self.0.hold_unmount, |hold| !hold).await;
        if self.0.fail_unmount.load(Ordering::SeqCst) {
            return Err(MountError::Unmount("files are open".into()));
        }
        Ok(())
    }
}

/// The core's in-memory provider, one per account. A test reaches an account's provider through [`Fake::providers`].
struct MemoryProviders(Arc<Fake>);

#[async_trait::async_trait]
impl Providers for MemoryProviders {
    async fn open(&self, provider: &str, account: &str) -> Result<Arc<dyn Provider>, MountError> {
        if provider != "memory" {
            return Err(MountError::UnknownProvider);
        }
        let memory = Arc::new(MemoryProvider::new());
        self.0
            .providers
            .lock()
            .unwrap()
            .insert(account.to_string(), Arc::clone(&memory));
        Ok(memory)
    }
}

struct Client {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,

    /// Lines read while waiting for another one.
    inbox: Vec<Value>,
    next_id: u64,
}

impl Client {
    /// Connects without saying `hello`.
    async fn open(socket: &Path) -> Self {
        let (reader, writer) = UnixStream::connect(socket).await.unwrap().into_split();
        Self {
            reader: BufReader::new(reader),
            writer,
            inbox: Vec::new(),
            next_id: 1,
        }
    }

    /// Connects and says `hello`.
    async fn connect(socket: &Path) -> Self {
        let mut client = Self::open(socket).await;
        let hello = client
            .call("hello", json!({"protocol": PROTOCOL_VERSION}))
            .await;
        assert_eq!(hello["result"]["protocol"], PROTOCOL_VERSION);
        client
    }

    /// Sends a request and returns its id.
    async fn send(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.writer
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        id
    }

    async fn response(&mut self, id: u64) -> Value {
        self.find(|line| line["id"] == id).await
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params).await;
        self.response(id).await
    }

    /// The status in the next `mountChanged` notification with this state.
    async fn event(&mut self, state: &str) -> Value {
        let mut notification = self
            .find(|line| line["method"] == "mountChanged" && line["params"]["state"] == state)
            .await;
        notification["params"].take()
    }

    async fn find(&mut self, wanted: impl Fn(&Value) -> bool) -> Value {
        loop {
            if let Some(index) = self.inbox.iter().position(&wanted) {
                return self.inbox.remove(index);
            }
            let mut line = String::new();
            self.reader.read_line(&mut line).await.unwrap();
            self.inbox.push(serde_json::from_str(&line).expect(&line));
        }
    }
}

struct Harness {
    socket: PathBuf,
    root: PathBuf,
    fake: Arc<Fake>,
    run: tokio::task::JoinHandle<()>,

    /// Stands in for SIGTERM.
    signal: oneshot::Sender<()>,
    dir: tempfile::TempDir,
}

fn start() -> Harness {
    start_after(|_, _| {})
}

fn start_after(prepare: impl FnOnce(&Path, &Fake)) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("mount.sock");
    let root = dir.path().join("Marlin");
    let fake = Fake::default();
    prepare(&root, &fake);
    let fake = Arc::new(fake);
    let server = Server::bind(
        Config {
            socket: socket.clone(),
            root: root.clone(),
        },
        Arc::new(FakeAdapter(Arc::clone(&fake))),
        Arc::new(MemoryProviders(Arc::clone(&fake))),
    )
    .unwrap();
    let (signal, signaled) = oneshot::channel();
    Harness {
        socket,
        root,
        fake,
        run: tokio::spawn(server.run(async {
            let _ = signaled.await;
        })),
        signal,
        dir,
    }
}

fn mount(account: &str, name: &str) -> Value {
    json!({"provider": "memory", "account": account, "name": name})
}

fn account(account: &str) -> Value {
    json!({"provider": "memory", "account": account})
}

fn code(response: &Value) -> i64 {
    response["error"]["code"].as_i64().expect("an error")
}

/// Makes one provider call through the mount named `name` and ignores the result.
async fn call(harness: &Harness, name: &str) {
    let vfs = harness.fake.vfs.lock().unwrap()[name].clone();
    let _ = vfs.space().await;
}

fn fail(harness: &Harness, account: &str, error: Option<VfsError>) {
    harness.fake.providers.lock().unwrap()[account].fail_with(error);
}

/// Mounts `account` at `name` and drops the `starting` and `mounted` notifications, so a later `mounted` can only be a recovery.
async fn mount_and_settle(client: &mut Client, watcher: &mut Client, account: &str, name: &str) {
    client.call("mount", mount(account, name)).await;
    watcher.event("starting").await;
    watcher.event("mounted").await;
}

#[tokio::test]
async fn hello_is_required_first_and_returns_the_protocol_and_version() {
    let harness = start();
    let mut client = Client::open(&harness.socket).await;
    assert_eq!(code(&client.call("list", Value::Null).await), -1010);
    let hello = client
        .call("hello", json!({"protocol": PROTOCOL_VERSION}))
        .await;
    assert_eq!(
        hello["result"],
        json!({"protocol": PROTOCOL_VERSION, "version": env!("CARGO_PKG_VERSION")})
    );
}

#[tokio::test]
async fn another_protocol_version_may_only_shut_the_helper_down() {
    let harness = start();
    let mut client = Client::open(&harness.socket).await;
    let hello = client.call("hello", json!({"protocol": 99})).await;
    assert_eq!(hello["result"]["protocol"], PROTOCOL_VERSION);
    assert_eq!(code(&client.call("list", Value::Null).await), -1011);
    let shutdown = client.call("shutdown", Value::Null).await;
    assert!(shutdown["error"].is_null());
    harness.run.await.unwrap();
}

#[tokio::test]
async fn mount_makes_the_folder_and_reports_each_state() {
    let harness = start();
    harness.fake.hold_mount.send_replace(true);
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    let id = client.send("mount", mount("ana", "drive")).await;
    wait(&harness.fake.mounts, |count| *count == 1).await;
    let list = watcher.call("list", Value::Null).await;
    assert_eq!(list["result"][0]["state"], "starting");
    harness.fake.hold_mount.send_replace(false);

    let mounted = client.response(id).await;
    assert_eq!(mounted["result"]["state"], "mounted");
    assert_eq!(
        mounted["result"]["mountPoint"],
        harness.root.join("drive").to_str().unwrap()
    );
    assert!(harness.root.join("drive").is_dir());
    let status = client.call("status", account("ana")).await;
    assert_eq!(status["result"]["state"], "mounted");
    watcher.event("starting").await;
    watcher.event("mounted").await;
}

#[tokio::test]
async fn mounts_of_one_account_at_the_same_time_call_the_adapter_once() {
    let harness = start();
    harness.fake.hold_mount.send_replace(true);
    let mut client = Client::connect(&harness.socket).await;
    let first = client.send("mount", mount("ana", "drive")).await;
    let second = client.send("mount", mount("ana", "other")).await;
    wait(&harness.fake.mounts, |count| *count == 1).await;
    harness.fake.hold_mount.send_replace(false);

    // A live mount keeps its name, whatever a later request asks for.
    for id in [first, second] {
        let response = client.response(id).await;
        assert_eq!(response["result"]["state"], "mounted");
        assert_eq!(response["result"]["name"], "drive");
    }
    let later = client.call("mount", mount("ana", "drive")).await;
    assert_eq!(later["result"]["state"], "mounted");
    assert_eq!(*harness.fake.mounts.borrow(), 1);
}

#[tokio::test]
async fn unmount_removes_the_folder_and_can_be_repeated() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    let unmount = client.call("unmount", account("ana")).await;
    assert!(unmount["error"].is_null());
    client.event("unmounting").await;
    client.event("unmounted").await;
    assert!(!harness.root.join("drive").exists());
    assert_eq!(client.call("list", Value::Null).await["result"], json!([]));

    let again = client.call("unmount", account("ana")).await;
    assert!(again["error"].is_null());
    let never_mounted = client.call("unmount", account("nobody")).await;
    assert!(never_mounted["error"].is_null());
}

#[tokio::test]
async fn two_accounts_mount_together_and_a_name_that_differs_by_case_is_refused() {
    let harness = start();
    harness.fake.hold_mount.send_replace(true);
    let mut client = Client::connect(&harness.socket).await;
    let ana = client.send("mount", mount("ana", "Drive")).await;
    let bob = client.send("mount", mount("bob", "photos")).await;
    // Both are inside the adapter at once.
    wait(&harness.fake.mounts, |count| *count == 2).await;
    harness.fake.hold_mount.send_replace(false);
    assert_eq!(client.response(ana).await["result"]["state"], "mounted");
    assert_eq!(client.response(bob).await["result"]["state"], "mounted");

    let clash = client.call("mount", mount("cam", "drive")).await;
    assert_eq!(code(&clash), -1002);
    assert!(client.call("status", account("cam")).await["result"].is_null());
}

#[tokio::test]
async fn a_name_that_is_not_one_plain_folder_name_is_refused() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    for name in ["", ".", "..", "a/b", "a\0b", &"a".repeat(256)] {
        let response = client.call("mount", mount("ana", name)).await;
        assert_eq!(code(&response), -1001, "{name:?}");
    }
    assert!(!harness.root.exists());
}

#[tokio::test]
async fn a_mount_point_that_holds_a_file_is_left_alone() {
    let harness = start();
    let mount_point = harness.root.join("drive");
    std::fs::create_dir_all(&mount_point).unwrap();
    std::fs::write(mount_point.join("keep.txt"), b"keep me").unwrap();
    let mut client = Client::connect(&harness.socket).await;
    let failed = client.call("mount", mount("ana", "drive")).await;
    assert_eq!(code(&failed), -1003);
    assert_eq!(
        std::fs::read(mount_point.join("keep.txt")).unwrap(),
        b"keep me"
    );
    let status = client.call("status", account("ana")).await;
    assert_eq!(status["result"]["state"], "failed");
    assert_eq!(status["result"]["error"]["code"], -1003);
    assert_eq!(*harness.fake.mounts.borrow(), 0);

    std::fs::remove_file(mount_point.join("keep.txt")).unwrap();
    let mounted = client.call("mount", mount("ana", "drive")).await;
    assert_eq!(mounted["result"]["state"], "mounted");
    // The helper did not make this folder, but it mounted on it, so it removes it.
    client.call("unmount", account("ana")).await;
    assert!(!mount_point.exists());
}

#[tokio::test]
async fn a_mount_point_that_is_a_symlink_is_refused_and_left_in_place() {
    let harness = start();
    let target = harness.dir.path().join("elsewhere");
    std::fs::create_dir(&target).unwrap();
    std::fs::create_dir_all(&harness.root).unwrap();
    let link = harness.root.join("drive");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let mut client = Client::connect(&harness.socket).await;
    let failed = client.call("mount", mount("ana", "drive")).await;
    assert_eq!(code(&failed), -1003);
    assert!(link.symlink_metadata().unwrap().is_symlink());
    assert!(target.is_dir());
}

#[tokio::test]
async fn a_failed_adapter_leaves_no_folder_and_the_next_mount_works() {
    let harness = start();
    *harness.fake.fail_mount.lock().unwrap() = Some(MountError::Adapter("no driver".into()));
    let mut client = Client::connect(&harness.socket).await;
    let failed = client.call("mount", mount("ana", "drive")).await;
    assert_eq!(code(&failed), -1006);
    assert!(!harness.root.join("drive").exists());
    assert_eq!(
        client.event("failed").await["error"]["message"],
        "no driver"
    );
    let status = client.call("status", account("ana")).await;
    assert_eq!(status["result"]["state"], "failed");

    let mounted = client.call("mount", mount("ana", "drive")).await;
    assert_eq!(mounted["result"]["state"], "mounted");
    assert!(mounted["result"]["error"].is_null());
}

#[tokio::test]
async fn an_unknown_provider_is_refused() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let failed = client
        .call(
            "mount",
            json!({"provider": "gdrive", "account": "ana", "name": "drive"}),
        )
        .await;
    assert_eq!(code(&failed), -1004);
    assert!(!harness.root.exists());
}

#[tokio::test]
async fn a_mount_finishes_after_its_client_has_gone() {
    let harness = start();
    harness.fake.hold_mount.send_replace(true);
    let mut client = Client::connect(&harness.socket).await;
    client.send("mount", mount("ana", "drive")).await;
    wait(&harness.fake.mounts, |count| *count == 1).await;
    let mut watcher = Client::connect(&harness.socket).await;
    drop(client);
    // A round trip on another connection gives the helper time to see that the client has gone.
    let list = watcher.call("list", Value::Null).await;
    assert_eq!(list["result"][0]["state"], "starting");
    harness.fake.hold_mount.send_replace(false);
    watcher.event("mounted").await;

    let mut client = Client::connect(&harness.socket).await;
    let list = client.call("list", Value::Null).await;
    assert_eq!(list["result"][0]["state"], "mounted");
}

#[tokio::test]
async fn a_failed_unmount_keeps_the_mount_and_the_helper() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    harness.fake.fail_unmount.store(true, Ordering::SeqCst);
    let unmount = client.call("unmount", account("ana")).await;
    assert_eq!(code(&unmount), -1007);
    let status = client.call("status", account("ana")).await;
    assert_eq!(status["result"]["state"], "mounted");
    assert!(harness.root.join("drive").is_dir());

    let shutdown = client.call("shutdown", Value::Null).await;
    assert_eq!(code(&shutdown), -1007);
    let list = client.call("list", Value::Null).await;
    assert_eq!(list["result"][0]["state"], "mounted");
}

#[tokio::test]
async fn shutdown_unmounts_everything_and_removes_the_socket() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    client.call("mount", mount("bob", "photos")).await;
    let shutdown = client.call("shutdown", Value::Null).await;
    assert!(shutdown["error"].is_null());
    harness.run.await.unwrap();
    assert_eq!(*harness.fake.unmounts.borrow(), 2);
    assert!(!harness.root.join("drive").exists());
    assert!(!harness.socket.exists());
}

#[tokio::test]
async fn a_signal_stops_the_helper_even_when_an_unmount_fails() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    harness.fake.fail_unmount.store(true, Ordering::SeqCst);
    harness.signal.send(()).unwrap();
    harness.run.await.unwrap();
    assert_eq!(*harness.fake.unmounts.borrow(), 1);
    assert!(!harness.socket.exists());
}

#[tokio::test]
async fn a_mount_sent_during_a_shutdown_is_refused_and_a_failed_shutdown_takes_mounts_again() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    harness.fake.fail_unmount.store(true, Ordering::SeqCst);
    harness.fake.hold_unmount.send_replace(true);
    let shutdown = client.send("shutdown", Value::Null).await;
    wait(&harness.fake.unmounts, |count| *count == 1).await;
    let refused = client.call("mount", mount("bob", "photos")).await;
    assert_eq!(code(&refused), -1009);

    harness.fake.hold_unmount.send_replace(false);
    assert_eq!(code(&client.response(shutdown).await), -1007);
    let mounted = client.call("mount", mount("bob", "photos")).await;
    assert_eq!(mounted["result"]["state"], "mounted");
}

#[tokio::test]
async fn mounts_queued_behind_an_unmount_end_as_one_mount() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    harness.fake.hold_unmount.send_replace(true);
    let unmount = client.send("unmount", account("ana")).await;
    wait(&harness.fake.unmounts, |count| *count == 1).await;
    let first = client.send("mount", mount("ana", "drive")).await;
    let second = client.send("mount", mount("ana", "drive")).await;
    // The test runtime has one thread and runs tasks in the order they were spawned, so once `list` is answered both mounts are waiting behind the unmount.
    let list = client.call("list", Value::Null).await;
    assert_eq!(list["result"][0]["state"], "unmounting");
    assert_eq!(*harness.fake.mounts.borrow(), 1);
    harness.fake.hold_unmount.send_replace(false);

    assert!(client.response(unmount).await["error"].is_null());
    assert_eq!(client.response(first).await["result"]["state"], "mounted");
    assert_eq!(client.response(second).await["result"]["state"], "mounted");
    let list = client.call("list", Value::Null).await;
    assert_eq!(list["result"].as_array().unwrap().len(), 1);
    assert_eq!(*harness.fake.mounts.borrow(), 2);
    assert_eq!(*harness.fake.unmounts.borrow(), 1);
}

#[tokio::test]
async fn hello_and_a_request_in_one_write_are_answered_in_order() {
    let harness = start();
    let mut client = Client::open(&harness.socket).await;
    let hello = json!({"jsonrpc": "2.0", "id": 1, "method": "hello", "params": {"protocol": PROTOCOL_VERSION}});
    let list = json!({"jsonrpc": "2.0", "id": 2, "method": "list"});
    client
        .writer
        .write_all(format!("{hello}\n{list}\n").as_bytes())
        .await
        .unwrap();
    let mut first = String::new();
    client.reader.read_line(&mut first).await.unwrap();
    assert!(first.contains(r#""id":1"#), "{first}");
    assert_eq!(client.response(2).await["result"], json!([]));
}

#[tokio::test]
async fn a_second_helper_cannot_bind_while_the_first_one_lives() {
    let dir = tempfile::tempdir().unwrap();
    let bind = || {
        Server::bind(
            Config {
                socket: dir.path().join("mount.sock"),
                root: dir.path().join("Marlin"),
            },
            Arc::new(FakeAdapter(Arc::default())),
            Arc::new(MemoryProviders(Arc::default())),
        )
    };
    let first = bind().unwrap();
    assert!(matches!(bind(), Err(BindError::AlreadyRunning)));

    // A helper that dies leaves its socket file. The lock is gone with the process, so the next helper starts.
    drop(first);
    assert!(dir.path().join("mount.sock").exists());
    bind().unwrap();
}

#[tokio::test]
async fn a_file_at_the_socket_path_is_not_removed() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("notes.txt");
    std::fs::write(&socket, b"keep me").unwrap();
    let bound = Server::bind(
        Config {
            socket: socket.clone(),
            root: dir.path().join("Marlin"),
        },
        Arc::new(FakeAdapter(Arc::default())),
        Arc::new(MemoryProviders(Arc::default())),
    );
    assert!(matches!(bound, Err(BindError::Io(_))));
    assert_eq!(std::fs::read(&socket).unwrap(), b"keep me");
}

#[tokio::test]
async fn the_socket_is_open_to_its_owner_only() {
    let harness = start();
    let mode = std::fs::metadata(&harness.socket)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[tokio::test]
async fn a_line_over_the_cap_closes_only_that_connection() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut long_line = vec![b'a'; 64 * 1024 + 1];
    long_line.push(b'\n');
    // The helper may close before the last byte is written.
    let _ = client.writer.write_all(&long_line).await;
    // The helper closes with part of the line unread, which Linux reports to the client as a reset.
    let mut rest = String::new();
    assert!(matches!(
        client.reader.read_line(&mut rest).await,
        Ok(0) | Err(_)
    ));
    Client::connect(&harness.socket).await;
}

#[tokio::test]
async fn a_token_in_an_adapter_error_never_reaches_the_client() {
    let harness = start();
    *harness.fake.fail_mount.lock().unwrap() =
        Some(MountError::Adapter("401 for Bearer abc123".into()));
    let mut client = Client::connect(&harness.socket).await;
    let failed = client.call("mount", mount("ana", "drive")).await;
    assert_eq!(failed["error"]["message"], "401 for Bearer [redacted]");
    let status = client.call("status", account("ana")).await;
    assert_eq!(
        status["result"]["error"]["message"],
        "401 for Bearer [redacted]"
    );
    assert_eq!(
        client.event("failed").await["error"]["message"],
        "401 for Bearer [redacted]"
    );
}

/// Stops the helper process when a failed assertion ends the test early.
struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn the_built_helper_answers_refuses_every_mount_and_exits_on_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("mount.sock");
    let helper = env!("CARGO_BIN_EXE_marlin-mount");
    let mut child = KillOnDrop(
        Command::new(helper)
            .arg("--socket")
            .arg(&socket)
            .arg("--root")
            .arg(dir.path().join("Marlin"))
            .spawn()
            .unwrap(),
    );
    // The one place a test has to poll: a process that is still starting has no socket yet.
    for _ in 0..500 {
        if UnixStream::connect(&socket).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut client = Client::connect(&socket).await;
    assert_eq!(client.call("list", Value::Null).await["result"], json!([]));
    // The shipped helper knows no provider yet (#529), and it opens the provider before it asks the adapter.
    let refused = client.call("mount", mount("ana", "drive")).await;
    assert_eq!(code(&refused), -1004);
    assert!(!dir.path().join("Marlin").exists());
    let shutdown = client.call("shutdown", Value::Null).await;
    assert!(shutdown["error"].is_null());
    assert_eq!(child.0.wait().unwrap().code(), Some(0));
    assert!(!socket.exists());

    let unknown_flag = Command::new(helper).arg("--nope").output().unwrap();
    assert_eq!(unknown_flag.status.code(), Some(2));
}

#[tokio::test]
async fn sign_in_needed_moves_a_mount_to_auth_required_at_once() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    mount_and_settle(&mut client, &mut watcher, "ana", "drive").await;

    fail(&harness, "ana", Some(VfsError::SignInNeeded));
    call(&harness, "drive").await;
    watcher.event("auth-required").await;
    assert!(watcher.inbox.is_empty());
    assert_eq!(
        client.call("status", account("ana")).await["result"]["state"],
        "auth-required"
    );
    assert_eq!(
        client.call("list", Value::Null).await["result"][0]["state"],
        "auth-required"
    );

    for _ in 0..4 {
        call(&harness, "drive").await;
    }
    fail(&harness, "ana", None);
    call(&harness, "drive").await;
    watcher.event("mounted").await;
    assert!(watcher.inbox.is_empty());
}

#[tokio::test]
async fn offline_takes_three_failed_calls_in_a_row_and_one_answer_ends_it() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    mount_and_settle(&mut client, &mut watcher, "ana", "drive").await;

    fail(&harness, "ana", Some(VfsError::Offline));
    call(&harness, "drive").await;
    call(&harness, "drive").await;
    assert_eq!(
        client.call("status", account("ana")).await["result"]["state"],
        "mounted"
    );
    call(&harness, "drive").await;
    watcher.event("offline").await;
    assert!(watcher.inbox.is_empty());

    call(&harness, "drive").await;
    call(&harness, "drive").await;
    fail(&harness, "ana", None);
    call(&harness, "drive").await;
    watcher.event("mounted").await;
    assert!(watcher.inbox.is_empty());
}

#[tokio::test]
async fn one_failed_call_between_answers_changes_nothing() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    mount_and_settle(&mut client, &mut watcher, "ana", "drive").await;

    call(&harness, "drive").await;
    fail(&harness, "ana", Some(VfsError::Offline));
    call(&harness, "drive").await;
    fail(&harness, "ana", None);
    call(&harness, "drive").await;
    fail(&harness, "ana", Some(VfsError::Offline));
    call(&harness, "drive").await;
    call(&harness, "drive").await;
    fail(&harness, "ana", None);
    call(&harness, "drive").await;
    assert_eq!(
        client.call("status", account("ana")).await["result"]["state"],
        "mounted"
    );

    client.call("unmount", account("ana")).await;
    watcher.event("unmounted").await;
    let states: Vec<_> = watcher
        .inbox
        .iter()
        .map(|line| &line["params"]["state"])
        .collect();
    assert_eq!(states, ["unmounting"]);
}

#[tokio::test]
async fn degraded_is_three_timeouts_or_unnamed_errors_in_a_row() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    mount_and_settle(&mut client, &mut watcher, "ana", "drive").await;

    fail(&harness, "ana", Some(VfsError::TimedOut));
    call(&harness, "drive").await;
    fail(&harness, "ana", Some(VfsError::Other("quota".into())));
    call(&harness, "drive").await;
    assert_eq!(
        client.call("status", account("ana")).await["result"]["state"],
        "mounted"
    );
    fail(&harness, "ana", Some(VfsError::TimedOut));
    call(&harness, "drive").await;
    watcher.event("degraded").await;
    assert!(watcher.inbox.is_empty());

    fail(&harness, "ana", None);
    call(&harness, "drive").await;
    watcher.event("mounted").await;
    assert!(watcher.inbox.is_empty());

    fail(&harness, "ana", Some(VfsError::NotFound));
    call(&harness, "drive").await;
    call(&harness, "drive").await;
    call(&harness, "drive").await;
    assert_eq!(
        client.call("status", account("ana")).await["result"]["state"],
        "mounted"
    );
}

#[tokio::test]
async fn a_run_of_failures_only_makes_the_state_worse() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    mount_and_settle(&mut client, &mut watcher, "ana", "drive").await;

    fail(&harness, "ana", Some(VfsError::TimedOut));
    for _ in 0..3 {
        call(&harness, "drive").await;
    }
    watcher.event("degraded").await;
    assert!(watcher.inbox.is_empty());

    fail(&harness, "ana", Some(VfsError::Offline));
    call(&harness, "drive").await;
    watcher.event("offline").await;
    assert!(watcher.inbox.is_empty());

    fail(&harness, "ana", Some(VfsError::TimedOut));
    call(&harness, "drive").await;
    assert_eq!(
        client.call("status", account("ana")).await["result"]["state"],
        "offline"
    );

    fail(&harness, "ana", Some(VfsError::SignInNeeded));
    call(&harness, "drive").await;
    watcher.event("auth-required").await;
    assert!(watcher.inbox.is_empty());

    fail(&harness, "ana", Some(VfsError::Offline));
    call(&harness, "drive").await;
    assert_eq!(
        client.call("status", account("ana")).await["result"]["state"],
        "auth-required"
    );
}

#[tokio::test]
async fn one_mounts_health_never_changes_another_mount() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    mount_and_settle(&mut client, &mut watcher, "ana", "drive").await;
    mount_and_settle(&mut client, &mut watcher, "ben", "share").await;

    fail(&harness, "ana", Some(VfsError::Offline));
    for _ in 0..3 {
        call(&harness, "drive").await;
    }
    watcher.event("offline").await;
    assert_eq!(
        client.call("status", account("ben")).await["result"]["state"],
        "mounted"
    );

    for _ in 0..3 {
        call(&harness, "share").await;
    }
    assert_eq!(
        client.call("status", account("ana")).await["result"]["state"],
        "offline"
    );
    assert_eq!(
        client.call("status", account("ben")).await["result"]["state"],
        "mounted"
    );
}

#[tokio::test]
async fn a_failed_unmount_keeps_the_health_and_an_unhealthy_mount_can_unmount() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    mount_and_settle(&mut client, &mut watcher, "ana", "drive").await;

    fail(&harness, "ana", Some(VfsError::Offline));
    for _ in 0..3 {
        call(&harness, "drive").await;
    }
    watcher.event("offline").await;

    harness.fake.fail_unmount.store(true, Ordering::SeqCst);
    let unmount = client.call("unmount", account("ana")).await;
    assert_eq!(code(&unmount), -1007);
    assert_eq!(
        client.call("status", account("ana")).await["result"]["state"],
        "offline"
    );

    harness.fake.fail_unmount.store(false, Ordering::SeqCst);
    let unmount = client.call("unmount", account("ana")).await;
    assert!(unmount["error"].is_null());
    watcher.event("unmounted").await;
}

#[tokio::test]
async fn a_call_from_an_earlier_mount_of_the_account_changes_nothing() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    mount_and_settle(&mut client, &mut watcher, "ana", "drive").await;
    let old_vfs = harness.fake.vfs.lock().unwrap()["drive"].clone();
    let old_provider = harness.fake.providers.lock().unwrap()["ana"].clone();

    client.call("unmount", account("ana")).await;
    watcher.event("unmounted").await;
    mount_and_settle(&mut client, &mut watcher, "ana", "drive").await;

    old_provider.fail_with(Some(VfsError::SignInNeeded));
    let _ = old_vfs.space().await;
    assert_eq!(
        client.call("status", account("ana")).await["result"]["state"],
        "mounted"
    );
}

#[tokio::test]
async fn the_sweep_clears_dead_mounts_and_empty_folders_and_leaves_the_rest() {
    let harness = start_after(|root, fake| {
        std::fs::create_dir_all(root).unwrap();
        let dead = root.join("dead");
        std::fs::create_dir(&dead).unwrap();
        std::fs::create_dir(root.join("empty")).unwrap();
        std::fs::create_dir(root.join("files")).unwrap();
        std::fs::write(root.join("files").join("keep.txt"), b"keep").unwrap();
        std::fs::write(root.join("note.txt"), b"note").unwrap();
        let outside = root.parent().unwrap().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let mut dead_set = fake.dead.lock().unwrap();
        dead_set.insert(dead);
        dead_set.insert(link);
    });

    let mut client = Client::connect(&harness.socket).await;
    assert!(!harness.root.join("dead").exists());
    assert!(!harness.root.join("empty").exists());
    assert_eq!(
        std::fs::read(harness.root.join("files").join("keep.txt")).unwrap(),
        b"keep"
    );
    assert_eq!(
        std::fs::read(harness.root.join("note.txt")).unwrap(),
        b"note"
    );
    let link = harness.root.join("link");
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(std::fs::read_link(&link).unwrap().is_dir());
    let dead = harness.fake.dead.lock().unwrap();
    assert_eq!(dead.len(), 1);
    assert!(dead.contains(&link));
    drop(dead);
    assert_eq!(client.call("list", Value::Null).await["result"], json!([]));
    assert_eq!(
        client.call("deadMounts", Value::Null).await["result"],
        json!([])
    );
}

#[tokio::test]
async fn a_dead_mount_that_cannot_be_cleared_is_reported_and_fails_its_mount_until_it_clears() {
    let harness = start_after(|root, fake| {
        let drive = root.join("drive");
        std::fs::create_dir_all(&drive).unwrap();
        fake.dead.lock().unwrap().insert(drive);
        fake.stuck.store(true, Ordering::SeqCst);
    });

    let mut client = Client::connect(&harness.socket).await;
    let drive = harness.root.join("drive");
    assert!(drive.is_dir());
    assert_eq!(
        client.call("deadMounts", Value::Null).await["result"],
        json!([{
            "name": "drive",
            "mountPoint": drive.display().to_string(),
        }])
    );
    let failed = client.call("mount", mount("ana", "drive")).await;
    assert_eq!(code(&failed), -1012);
    let status = client.call("status", account("ana")).await;
    assert_eq!(status["result"]["state"], "failed");
    assert_eq!(status["result"]["error"]["code"], -1012);
    assert_eq!(*harness.fake.mounts.borrow(), 0);

    harness.fake.stuck.store(false, Ordering::SeqCst);
    let mounted = client.call("mount", mount("ana", "drive")).await;
    assert_eq!(mounted["result"]["state"], "mounted");
    assert_eq!(
        client.call("deadMounts", Value::Null).await["result"],
        json!([])
    );
}
