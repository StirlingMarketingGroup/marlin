#![cfg(unix)]

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use marlin_mount::protocol::PROTOCOL_VERSION;
use marlin_mount::{
    Adapter, BindError, Config, MountError, Opened, Providers, Server, Session, MOUNT_FOLDER_MARKER,
};
use marlin_vfs::{Access, ItemId, MemoryProvider, Options, Provider, Vfs, VfsError};
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

    /// The call limit the next `open` gives the mount. `None` leaves the helper's own limit.
    max_calls: Mutex<Option<usize>>,
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
    async fn open(&self, provider: &str, account: &str) -> Result<Opened, MountError> {
        if provider != "memory" {
            return Err(MountError::UnknownProvider);
        }
        let memory = Arc::new(MemoryProvider::new());
        self.0
            .providers
            .lock()
            .unwrap()
            .insert(account.to_string(), Arc::clone(&memory));
        let max_calls = *self.0.max_calls.lock().unwrap();
        Ok(Opened {
            provider: memory,
            max_calls,
        })
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

    /// The next `unsentChanged` whose params pass `check`. A missing one fails the test instead of hanging it.
    async fn unsent(&mut self, check: impl Fn(&Value) -> bool) -> Value {
        let found = self.find(|line| line["method"] == "unsentChanged" && check(&line["params"]));
        let mut notification = tokio::time::timeout(Duration::from_secs(30), found)
            .await
            .expect("no such unsentChanged");
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
    spool: PathBuf,
    fake: Arc<Fake>,
    run: tokio::task::JoinHandle<()>,

    /// Stands in for SIGTERM.
    signal: oneshot::Sender<()>,
    dir: tempfile::TempDir,
}

fn start() -> Harness {
    start_with(None, |_, _| {})
}

fn start_on(spool: PathBuf) -> Harness {
    start_with(Some(spool), |_, _| {})
}

fn start_after(prepare: impl FnOnce(&Path, &Fake)) -> Harness {
    start_with(None, prepare)
}

fn harness_options() -> Options {
    Options {
        settle: Duration::from_millis(50),
        ..Options::default()
    }
}

fn start_with(spool: Option<PathBuf>, prepare: impl FnOnce(&Path, &Fake)) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("mount.sock");
    let root = dir.path().join("Marlin");
    let spool = spool.unwrap_or_else(|| dir.path().join("spool"));
    let fake = Fake::default();
    prepare(&root, &fake);
    let fake = Arc::new(fake);
    let server = Server::bind(
        Config {
            socket: socket.clone(),
            root: root.clone(),
            spool: spool.clone(),
            options: harness_options(),
        },
        Arc::new(FakeAdapter(Arc::clone(&fake))),
        Arc::new(MemoryProviders(Arc::clone(&fake))),
    )
    .unwrap();
    let (signal, signaled) = oneshot::channel();
    Harness {
        socket,
        root,
        spool,
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

/// Makes one provider call through the mount named `name` and ignores the result. The refresh comes first, so the core asks the provider and does not reuse the space answer it holds.
async fn call(harness: &Harness, name: &str) {
    let vfs = harness.fake.vfs.lock().unwrap()[name].clone();
    vfs.refresh();
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
    assert!(harness.root.join(MOUNT_FOLDER_MARKER).is_file());
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
async fn built_account_names_mount() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let long = "a".repeat(300);
    let mut changed = long.clone();
    changed.pop();
    changed.push('b');
    let accounts = [
        "Ana@example.com".to_string(),
        "ana@example.com".to_string(),
        "a/b:c*d@example.com".to_string(),
        long,
        changed,
    ];
    let mut taken = Vec::new();
    for account in &accounts {
        let name = marlin_mount_client::mount_name::build("memory", account, &taken);
        taken.push(name.clone());
        let response = client.call("mount", mount(account, &name)).await;
        assert_eq!(response["result"]["state"], "mounted", "{name}");
        assert_eq!(response["result"]["name"], name);
    }
}

#[tokio::test]
async fn a_mount_point_that_holds_a_file_is_left_alone() {
    let harness = start();
    let mount_point = harness.root.join("drive");
    std::fs::create_dir_all(&mount_point).unwrap();
    std::fs::write(harness.root.join(MOUNT_FOLDER_MARKER), "").unwrap();
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
    std::fs::write(harness.root.join(MOUNT_FOLDER_MARKER), "").unwrap();
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
                spool: dir.path().join("spool"),
                options: harness_options(),
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
            spool: dir.path().join("spool"),
            options: harness_options(),
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

#[tokio::test]
async fn a_providers_own_call_limit_replaces_the_helpers() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    // The helper's own limit is the default 16. A provider's number replaces it, above or below.
    for (account, limit, peak) in [
        ("few", Some(2), 2),
        ("default", None, 16),
        ("many", Some(20), 20),
    ] {
        *harness.fake.max_calls.lock().unwrap() = limit;
        client.call("mount", mount(account, account)).await;
        let provider = harness.fake.providers.lock().unwrap()[account].clone();
        let vfs = harness.fake.vfs.lock().unwrap()[account].clone();
        let root = provider.root();
        for index in 0..24 {
            provider.add_file(&root, &format!("f{index}.txt"), b"x");
        }
        let mut nodes = Vec::new();
        for index in 0..24 {
            let entry = vfs
                .lookup(Vfs::ROOT, &format!("f{index}.txt"))
                .await
                .unwrap();
            nodes.push(entry.node);
        }
        vfs.refresh();
        provider.set_delay(Duration::from_millis(200));
        let mut calls = Vec::new();
        for node in nodes {
            let vfs = Arc::clone(&vfs);
            calls.push(tokio::spawn(async move {
                vfs.attributes(node).await.unwrap();
            }));
        }
        for call in calls {
            call.await.unwrap();
        }
        assert_eq!(provider.peak_calls(), peak, "{account}");
    }
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
async fn the_built_helper_answers_refuses_an_unknown_provider_and_exits_on_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("mount.sock");
    let helper = env!("CARGO_BIN_EXE_marlin-mount");
    let mut child = KillOnDrop(
        Command::new(helper)
            .arg("--socket")
            .arg(&socket)
            .arg("--root")
            .arg(dir.path().join("Marlin"))
            .arg("--spool")
            .arg(dir.path().join("spool"))
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
    // The shipped helper knows no provider named `memory`, and it opens the provider before it asks the adapter.
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
        .filter(|line| line["method"] == "mountChanged")
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
    old_vfs.refresh();
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
        std::fs::write(root.join(MOUNT_FOLDER_MARKER), "").unwrap();
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
    assert!(harness.root.join(MOUNT_FOLDER_MARKER).is_file());
    {
        let dead = harness.fake.dead.lock().unwrap();
        assert_eq!(dead.len(), 1);
        assert!(dead.contains(&link));
    }
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
        std::fs::write(root.join(MOUNT_FOLDER_MARKER), "").unwrap();
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

#[tokio::test]
async fn a_mount_folder_the_helper_did_not_make_is_left_alone() {
    let harness = start_after(|root, fake| {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join("README.md"), b"the checkout").unwrap();
        std::fs::create_dir(root.join(".worktrees")).unwrap();
        std::fs::create_dir(root.join("drive")).unwrap();
        let dead = root.join("dead");
        std::fs::create_dir(&dead).unwrap();
        fake.dead.lock().unwrap().insert(dead);
    });

    let mut client = Client::connect(&harness.socket).await;
    assert!(harness.root.join(".worktrees").is_dir());
    assert!(harness.root.join("drive").is_dir());
    assert!(harness
        .fake
        .dead
        .lock()
        .unwrap()
        .contains(&harness.root.join("dead")));
    assert_eq!(
        client.call("deadMounts", Value::Null).await["result"],
        json!([])
    );

    let failed = client.call("mount", mount("ana", "drive")).await;
    assert_eq!(code(&failed), -1017);
    assert!(failed["error"]["message"]
        .as_str()
        .unwrap()
        .contains(&harness.root.display().to_string()));
    let status = client.call("status", account("ana")).await;
    assert_eq!(status["result"]["state"], "failed");
    assert_eq!(status["result"]["error"]["code"], -1017);
    assert!(harness.root.join("drive").is_dir());
    assert!(!harness.root.join(MOUNT_FOLDER_MARKER).exists());
    assert_eq!(
        std::fs::read(harness.root.join("README.md")).unwrap(),
        b"the checkout"
    );
    assert_eq!(*harness.fake.mounts.borrow(), 0);
}

#[tokio::test]
async fn an_empty_mount_folder_becomes_the_helpers() {
    let harness = start_after(|root, _fake| {
        std::fs::create_dir_all(root).unwrap();
    });
    let mut client = Client::connect(&harness.socket).await;
    let mounted = client.call("mount", mount("ana", "drive")).await;
    assert_eq!(mounted["result"]["state"], "mounted");
    assert!(harness.root.join(MOUNT_FOLDER_MARKER).is_file());
}

#[tokio::test]
async fn an_unmount_leaves_its_folder_when_the_mount_folder_lost_its_marker() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mounted = client.call("mount", mount("ana", "drive")).await;
    assert_eq!(mounted["result"]["state"], "mounted");
    std::fs::remove_file(harness.root.join(MOUNT_FOLDER_MARKER)).unwrap();
    let unmounted = client.call("unmount", account("ana")).await;
    assert!(unmounted.get("error").is_none(), "{unmounted}");
    assert_eq!(
        client.call("status", account("ana")).await["result"],
        Value::Null
    );
    assert!(harness.root.join("drive").is_dir());
}

/// Sends this process's stderr to a file until it is dropped. The helper's log is its stderr, and a server that runs inside the test process has no other place a test can read it. Every test in the process logs there, so match on a text only one test writes. Drop this before any assertion: a panic's message would land in the file.
struct CapturedStderr {
    saved: libc::c_int,
    _file: std::fs::File,
}

impl CapturedStderr {
    fn start(path: &Path) -> Self {
        let file = std::fs::File::create(path).unwrap();
        // SAFETY: dup and dup2 only change this process's file-descriptor table.
        let saved = unsafe { libc::dup(libc::STDERR_FILENO) };
        assert!(saved >= 0);
        let redirected = unsafe { libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) };
        assert_eq!(redirected, libc::STDERR_FILENO);
        Self { saved, _file: file }
    }
}

impl Drop for CapturedStderr {
    fn drop(&mut self) {
        // SAFETY: the saved fd came from dup and still belongs to this value.
        unsafe {
            libc::dup2(self.saved, libc::STDERR_FILENO);
            libc::close(self.saved);
        }
    }
}

#[tokio::test]
async fn the_text_of_a_provider_failure_is_logged_once_a_minute_with_secrets_masked() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    let path = harness.dir.path().join("stderr.log");
    let capture = CapturedStderr::start(&path);
    let marker = "marlin-logged-once-7f3c";
    let text = format!("{marker} for Bearer abc123");
    fail(&harness, "ana", Some(VfsError::Other(text.clone())));
    for _ in 0..3 {
        call(&harness, "drive").await;
    }
    // An answered call in between does not bring the line back.
    fail(&harness, "ana", None);
    call(&harness, "drive").await;
    fail(&harness, "ana", Some(VfsError::Other(text.clone())));
    call(&harness, "drive").await;
    // A minute later the same text is news again.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    tokio::time::resume();
    call(&harness, "drive").await;
    // Another text is logged at once.
    let other = format!("{marker} again for Bearer abc123");
    fail(&harness, "ana", Some(VfsError::Other(other)));
    call(&harness, "drive").await;
    drop(capture);

    let logged = std::fs::read_to_string(&path).unwrap();
    assert!(!logged.contains("abc123"), "{logged}");
    let lines: Vec<_> = logged
        .lines()
        .filter(|line| line.contains(marker))
        .collect();
    assert_eq!(lines.len(), 3, "{logged}");
    for line in lines {
        let value: Value = serde_json::from_str(line).expect(line);
        assert_eq!(value["event"], "mount.call_failed");
        assert_eq!(value["provider"], "memory");
        assert_eq!(value["account"], "ana");
        let message = value["message"].as_str().unwrap();
        assert!(message.ends_with("Bearer [redacted]"), "{message}");
    }
}

fn provider(harness: &Harness, account: &str) -> Arc<MemoryProvider> {
    harness.fake.providers.lock().unwrap()[account].clone()
}

fn account_spool(harness: &Harness, account: &str) -> PathBuf {
    harness.spool.join("memory").join(account)
}

fn mode_bits(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn entry_names(path: &Path) -> Vec<String> {
    let mut names = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn numbered_files(path: &Path) -> Vec<String> {
    entry_names(path)
        .into_iter()
        .filter(|name| name.parse::<u64>().is_ok())
        .collect()
}

fn unsent_call(account: &str, mount_id: &str, node: u64) -> Value {
    json!({"provider": "memory", "account": account, "mountId": mount_id, "node": node})
}

fn refused_access(params: &Value) -> bool {
    params["files"].as_array().is_some_and(|files| {
        files
            .iter()
            .any(|file| file["state"] == "refused" && file["error"] == "access denied")
    })
}

fn failed_offline(params: &Value) -> bool {
    params["files"].as_array().is_some_and(|files| {
        files
            .iter()
            .any(|file| file["state"] == "failed" && file["error"] == "offline")
    })
}

fn no_files(params: &Value) -> bool {
    params["files"]
        .as_array()
        .is_some_and(|files| files.is_empty())
}

fn uploading(params: &Value) -> bool {
    params["files"]
        .as_array()
        .is_some_and(|files| files.iter().any(|file| file["state"] == "uploading"))
}

async fn write_bytes(vfs: &Vfs, name: &str, bytes: &[u8]) {
    let (entry, handle) = vfs.create(Vfs::ROOT, name, Access::Write).await.unwrap();
    vfs.write(handle, 0, bytes.to_vec()).await.unwrap();
    vfs.release(handle);
    // `create` counts one lookup. The OS forgets it when it drops the name. A held lookup keeps the spool file after a discard.
    vfs.forget(entry.node, 1);
}

async fn stored(provider: &MemoryProvider, name: &str) -> Option<Vec<u8>> {
    let items = provider.list(&ItemId("root".to_string())).await.unwrap();
    let item = items.into_iter().find(|item| item.name == name)?;
    Some(provider.contents(&item.id))
}

/// Creates `a.txt` while the provider is healthy, then fails the upload.
async fn leave_failed(
    harness: &Harness,
    watcher: &mut Client,
    account: &str,
    mount_name: &str,
) -> Value {
    let vfs = harness.fake.vfs.lock().unwrap()[mount_name].clone();
    let (entry, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    fail(harness, account, Some(VfsError::Offline));
    vfs.write(handle, 0, b"hello".to_vec()).await.unwrap();
    vfs.release(handle);
    // `create` counts one lookup. The OS forgets it when it drops the name. A held lookup keeps the spool file after a discard.
    vfs.forget(entry.node, 1);
    watcher.unsent(failed_offline).await
}

#[tokio::test]
async fn a_mount_makes_a_private_spool_and_uploads_a_written_file() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    let mounted = client.call("mount", mount("ana", "drive")).await;
    assert_eq!(mounted["result"]["state"], "mounted");
    assert!(mounted["result"].get("readOnly").is_none());
    let folder = account_spool(&harness, "ana");
    assert!(folder.is_dir());
    assert_eq!(mode_bits(&folder), 0o700);

    let remote = provider(&harness, "ana");
    remote.set_upload_delay(Duration::from_secs(2));
    let vfs = harness.fake.vfs.lock().unwrap()["drive"].clone();
    write_bytes(&vfs, "a.txt", b"hello").await;
    watcher.unsent(uploading).await;
    watcher.unsent(no_files).await;
    assert_eq!(stored(&remote, "a.txt").await.unwrap(), b"hello");
}

#[tokio::test]
async fn a_failed_upload_is_reported_and_a_retry_sends_it() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    let reported = leave_failed(&harness, &mut watcher, "ana", "drive").await;
    let listed = client.call("unsent", account("ana")).await;
    assert_eq!(listed["result"], reported);

    fail(&harness, "ana", None);
    let node = reported["files"][0]["node"].as_u64().unwrap();
    let mount_id = reported["mountId"].as_str().unwrap();
    let retry = client
        .call("retryUnsent", unsent_call("ana", mount_id, node))
        .await;
    assert!(retry["error"].is_null());
    // The automatic retry comes 5 seconds after the failure, so this proves the asked-for one.
    tokio::time::timeout(Duration::from_secs(3), watcher.unsent(no_files))
        .await
        .expect("the retry did not send the file");
    assert_eq!(
        stored(&provider(&harness, "ana"), "a.txt").await.unwrap(),
        b"hello"
    );
}

#[tokio::test]
async fn a_refused_upload_is_reported_and_a_retry_sends_it() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    let vfs = harness.fake.vfs.lock().unwrap()["drive"].clone();
    let (entry, handle) = vfs.create(Vfs::ROOT, "a.txt", Access::Write).await.unwrap();
    fail(&harness, "ana", Some(VfsError::AccessDenied));
    vfs.write(handle, 0, b"hello".to_vec()).await.unwrap();
    vfs.release(handle);
    vfs.forget(entry.node, 1);
    let reported = watcher.unsent(refused_access).await;
    let remote = provider(&harness, "ana");
    fail(&harness, "ana", None);
    let node = reported["files"][0]["node"].as_u64().unwrap();
    let mount_id = reported["mountId"].as_str().unwrap();
    let retry = client
        .call("retryUnsent", unsent_call("ana", mount_id, node))
        .await;
    assert!(retry["error"].is_null());
    tokio::time::timeout(Duration::from_secs(3), watcher.unsent(no_files))
        .await
        .expect("the retry did not send the file");
    assert_eq!(stored(&remote, "a.txt").await.unwrap(), b"hello");
}

#[tokio::test]
async fn discard_drops_a_failed_file_and_it_is_not_uploaded_later() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    let reported = leave_failed(&harness, &mut watcher, "ana", "drive").await;
    let node = reported["files"][0]["node"].as_u64().unwrap();
    let mount_id = reported["mountId"].as_str().unwrap();
    let discard = client
        .call("discardUnsent", unsent_call("ana", mount_id, node))
        .await;
    assert!(discard["error"].is_null());
    watcher.unsent(no_files).await;
    let retry = client
        .call("retryUnsent", unsent_call("ana", mount_id, node))
        .await;
    assert_eq!(code(&retry), -1015);
    assert!(numbered_files(&account_spool(&harness, "ana")).is_empty());
}

#[tokio::test]
async fn retry_and_discard_refuse_a_mount_id_or_a_node_that_is_not_this_mounts() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    let first = client.call("unsent", account("ana")).await;
    let old_id = first["result"]["mountId"].as_str().unwrap().to_string();
    client.call("unmount", account("ana")).await;
    client.call("mount", mount("ana", "drive")).await;
    let reported = leave_failed(&harness, &mut watcher, "ana", "drive").await;
    let current = reported["mountId"].as_str().unwrap();
    let node = reported["files"][0]["node"].as_u64().unwrap();
    assert_ne!(current, old_id);

    for params in [
        unsent_call("ana", &old_id, node),
        unsent_call("ana", "not-a-uuid", node),
        unsent_call("ana", current, node + 1000),
    ] {
        assert_eq!(
            code(&client.call("retryUnsent", params.clone()).await),
            -1015
        );
        assert_eq!(code(&client.call("discardUnsent", params).await), -1015);
    }
    let listed = client.call("unsent", account("ana")).await;
    assert!(failed_offline(&listed["result"]));
    let discard = client
        .call("discardUnsent", unsent_call("ana", current, node))
        .await;
    assert!(discard["error"].is_null());

    let mut older = Client::open(&harness.socket).await;
    older
        .call("hello", json!({"protocol": PROTOCOL_VERSION - 1}))
        .await;
    assert_eq!(code(&older.call("unsent", account("ana")).await), -1011);
}

#[tokio::test]
async fn an_unmount_leaves_a_failed_file_and_the_next_mount_uploads_it() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    leave_failed(&harness, &mut watcher, "ana", "drive").await;
    let id = client.send("unmount", account("ana")).await;
    let mut last = None;
    loop {
        let line = watcher
            .find(|line| {
                line["method"] == "unsentChanged"
                    || (line["method"] == "mountChanged" && line["params"]["state"] == "unmounted")
            })
            .await;
        if line["method"] == "unsentChanged" {
            last = Some(line["params"].clone());
            continue;
        }
        break;
    }
    assert!(failed_offline(&last.unwrap()));
    assert!(client.response(id).await["error"].is_null());
    let names = entry_names(&account_spool(&harness, "ana"));
    assert!(names.iter().any(|name| name.parse::<u64>().is_ok()));
    assert!(names.iter().any(|name| name.ends_with(".record")));

    harness.fake.hold_mount.send_replace(true);
    let id = client.send("mount", mount("ana", "drive")).await;
    wait(&harness.fake.mounts, |count| *count == 2).await;
    provider(&harness, "ana").set_upload_delay(Duration::from_secs(2));
    harness.fake.hold_mount.send_replace(false);
    assert_eq!(client.response(id).await["result"]["state"], "mounted");
    watcher.unsent(uploading).await;
    watcher.unsent(no_files).await;
    assert_eq!(
        stored(&provider(&harness, "ana"), "a.txt").await.unwrap(),
        b"hello"
    );
}

#[tokio::test]
async fn an_unmount_waits_for_a_running_upload() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    let remote = provider(&harness, "ana");
    remote.set_upload_delay(Duration::from_secs(2));
    let vfs = harness.fake.vfs.lock().unwrap()["drive"].clone();
    write_bytes(&vfs, "a.txt", b"hello").await;
    watcher.unsent(uploading).await;
    let id = client.send("unmount", account("ana")).await;
    watcher.event("unmounting").await;
    assert!(stored(&remote, "a.txt").await.is_none());
    assert!(client.response(id).await["error"].is_null());
    assert_eq!(stored(&remote, "a.txt").await.unwrap(), b"hello");
    assert!(numbered_files(&account_spool(&harness, "ana")).is_empty());
}

#[tokio::test]
async fn a_shutdown_from_another_protocol_version_waits_for_the_upload() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    let remote = provider(&harness, "ana");
    remote.set_upload_delay(Duration::from_secs(2));
    let vfs = harness.fake.vfs.lock().unwrap()["drive"].clone();
    write_bytes(&vfs, "a.txt", b"hello").await;
    watcher.unsent(uploading).await;
    let mut older = Client::open(&harness.socket).await;
    older
        .call("hello", json!({"protocol": PROTOCOL_VERSION - 1}))
        .await;
    let id = older.send("shutdown", Value::Null).await;
    assert!(stored(&remote, "a.txt").await.is_none());
    assert!(older.response(id).await["error"].is_null());
    harness.run.await.unwrap();
    assert_eq!(stored(&remote, "a.txt").await.unwrap(), b"hello");
}

#[tokio::test]
async fn a_signal_stops_the_wait_and_the_next_start_uploads_what_is_left() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    provider(&harness, "ana").set_upload_delay(Duration::from_secs(60));
    let vfs = harness.fake.vfs.lock().unwrap()["drive"].clone();
    write_bytes(&vfs, "a.txt", b"hello").await;
    watcher.unsent(uploading).await;
    client.send("unmount", account("ana")).await;
    watcher.event("unmounting").await;
    let folder = account_spool(&harness, "ana");
    let spool = harness.spool.clone();
    harness.signal.send(()).unwrap();
    let run = harness.run;
    tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("the helper did not exit")
        .unwrap();
    let names = entry_names(&folder);
    assert!(names.iter().any(|name| name.parse::<u64>().is_ok()));
    assert!(names.iter().any(|name| name.ends_with(".record")));

    let next = start_on(spool);
    let mut client = Client::connect(&next.socket).await;
    let mut watcher = Client::connect(&next.socket).await;
    next.fake.hold_mount.send_replace(true);
    let id = client.send("mount", mount("ana", "drive")).await;
    wait(&next.fake.mounts, |count| *count == 1).await;
    provider(&next, "ana").set_upload_delay(Duration::from_secs(2));
    next.fake.hold_mount.send_replace(false);
    assert_eq!(client.response(id).await["result"]["state"], "mounted");
    watcher.unsent(uploading).await;
    watcher.unsent(no_files).await;
    assert_eq!(
        stored(&provider(&next, "ana"), "a.txt").await.unwrap(),
        b"hello"
    );
}

#[tokio::test]
async fn a_file_where_the_spool_folder_should_be_mounts_read_only() {
    let harness = start();
    std::fs::create_dir_all(&harness.spool).unwrap();
    std::fs::write(harness.spool.join("memory"), b"not a folder").unwrap();
    let mut client = Client::connect(&harness.socket).await;
    let mounted = client.call("mount", mount("ana", "drive")).await;
    assert_eq!(mounted["result"]["state"], "mounted");
    assert_eq!(mounted["result"]["readOnly"]["reason"], "spool");
    let vfs = harness.fake.vfs.lock().unwrap()["drive"].clone();
    let error = vfs
        .create(Vfs::ROOT, "a.txt", Access::Write)
        .await
        .unwrap_err();
    assert_eq!(error, VfsError::ReadOnly);
    assert!(client.call("unsent", account("ana")).await["result"].is_null());
    // A read-only mount answers the requests about its items too.
    let top = client.call("identify", item_call("ana", "path", "")).await;
    assert_eq!(top["result"], json!({"item": "remote"}));
}

#[tokio::test]
async fn an_open_spool_folder_is_closed_and_a_symlink_stays_read_only() {
    let harness = start();
    let folder = account_spool(&harness, "ana");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut client = Client::connect(&harness.socket).await;
    let mounted = client.call("mount", mount("ana", "drive")).await;
    assert!(mounted["result"].get("readOnly").is_none());
    assert_eq!(mode_bits(&folder), 0o700);

    // A folder the user cannot write in takes writes once it is 0700 again.
    let closed = account_spool(&harness, "carol");
    std::fs::create_dir_all(&closed).unwrap();
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o500)).unwrap();
    let mounted = client.call("mount", mount("carol", "music")).await;
    assert!(mounted["result"].get("readOnly").is_none());
    assert_eq!(mode_bits(&closed), 0o700);

    let link = account_spool(&harness, "bob");
    std::os::unix::fs::symlink(&folder, &link).unwrap();
    let mounted = client.call("mount", mount("bob", "photos")).await;
    assert_eq!(mounted["result"]["readOnly"]["reason"], "spool");
}

#[tokio::test]
async fn two_helpers_cannot_share_one_accounts_spool() {
    let first = start();
    let second = start_on(first.spool.clone());
    let mut one = Client::connect(&first.socket).await;
    let mut other = Client::connect(&second.socket).await;
    let mounted = one.call("mount", mount("ana", "drive")).await;
    assert!(mounted["result"].get("readOnly").is_none());
    let mounted = other.call("mount", mount("ana", "drive")).await;
    assert_eq!(mounted["result"]["readOnly"]["reason"], "spool");
    assert!(other.call("unsent", account("ana")).await["result"].is_null());

    assert!(other.call("unmount", account("ana")).await["error"].is_null());
    assert!(one.call("unmount", account("ana")).await["error"].is_null());
    let mounted = other.call("mount", mount("ana", "drive")).await;
    assert!(mounted["result"].get("readOnly").is_none());
    assert!(other.call("unsent", account("ana")).await["result"]["mountId"].is_string());
}

#[tokio::test]
async fn a_discard_that_waits_on_the_change_lock_holds_up_the_unmount() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    let reported = leave_failed(&harness, &mut watcher, "ana", "drive").await;
    fail(&harness, "ana", None);
    let remote = provider(&harness, "ana");
    remote.set_delay(Duration::from_secs(1));
    let vfs = harness.fake.vfs.lock().unwrap()["drive"].clone();
    let creating = tokio::spawn({
        let vfs = Arc::clone(&vfs);
        async move {
            vfs.create_folder(Vfs::ROOT, "x").await.unwrap();
        }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if remote.in_flight() >= 1 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let node = reported["files"][0]["node"].as_u64().unwrap();
    let mount_id = reported["mountId"].as_str().unwrap();
    let discard_id = client
        .send("discardUnsent", unsent_call("ana", mount_id, node))
        .await;
    let unmount_id = client.send("unmount", account("ana")).await;
    let first = client
        .find(|line| line["id"] == discard_id || line["id"] == unmount_id)
        .await;
    assert_eq!(first["id"], discard_id);
    assert!(first["error"].is_null());
    assert!(client.response(unmount_id).await["error"].is_null());
    creating.await.unwrap();

    client.call("mount", mount("ana", "drive")).await;
    let listed = client.call("unsent", account("ana")).await;
    assert!(listed["result"]["files"].as_array().unwrap().is_empty());
    assert!(numbered_files(&account_spool(&harness, "ana")).is_empty());
}

#[tokio::test]
async fn a_spool_file_with_no_record_is_unplaced() {
    let harness = start();
    let folder = account_spool(&harness, "ana");
    std::fs::create_dir_all(&folder).unwrap();
    let orphan = folder.join("1");
    std::fs::write(&orphan, b"bytes").unwrap();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    let reported = watcher
        .unsent(|params| {
            no_files(params)
                && params["unplaced"]
                    .as_array()
                    .is_some_and(|paths| paths.len() == 1)
        })
        .await;
    assert_eq!(reported["unplaced"][0], orphan.display().to_string());
    let listed = client.call("unsent", account("ana")).await;
    assert_eq!(listed["result"], reported);
}

#[tokio::test]
async fn a_discard_while_an_unmount_waits_stops_the_upload_and_ends_the_wait() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    let remote = provider(&harness, "ana");
    remote.set_upload_delay(Duration::from_secs(60));
    let vfs = harness.fake.vfs.lock().unwrap()["drive"].clone();
    write_bytes(&vfs, "a.txt", b"hello").await;
    let reported = watcher.unsent(uploading).await;
    let id = client.send("unmount", account("ana")).await;
    watcher.event("unmounting").await;

    let node = reported["files"][0]["node"].as_u64().unwrap();
    let mount_id = reported["mountId"].as_str().unwrap();
    let discard = client
        .call("discardUnsent", unsent_call("ana", mount_id, node))
        .await;
    assert!(discard["error"].is_null());
    let unmounted = tokio::time::timeout(Duration::from_secs(10), client.response(id))
        .await
        .expect("the unmount still waited for the discarded upload");
    assert!(unmounted["error"].is_null());
    assert!(stored(&remote, "a.txt").await.is_none());
    assert!(numbered_files(&account_spool(&harness, "ana")).is_empty());
}

#[tokio::test]
async fn a_mount_during_a_shutdown_that_waits_for_uploads_is_refused_at_once() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    let mut watcher = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    provider(&harness, "ana").set_upload_delay(Duration::from_secs(60));
    let vfs = harness.fake.vfs.lock().unwrap()["drive"].clone();
    write_bytes(&vfs, "a.txt", b"hello").await;
    watcher.unsent(uploading).await;
    client.send("shutdown", Value::Null).await;
    watcher.event("unmounting").await;

    let again = tokio::time::timeout(
        Duration::from_secs(5),
        client.call("mount", mount("ana", "drive")),
    )
    .await
    .expect("the mount waited behind the shutdown");
    assert_eq!(code(&again), -1009);
    harness.signal.send(()).unwrap();
    harness.run.await.unwrap();
}

fn item_call(account: &str, key: &str, value: &str) -> Value {
    json!({"provider": "memory", "account": account, key: value})
}

#[tokio::test]
async fn locate_and_identify_name_an_item_as_the_listing_shows_it() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    let memory = provider(&harness, "ana");
    let projects = memory.add_folder(&memory.root(), "Projects");
    let plans = memory.add_folder(&projects, "Plans");
    let first = memory.add_file(&plans, "a.txt", b"a");
    let second = memory.add_file(&plans, "a.txt", b"b");

    let second_path = client
        .call("locate", item_call("ana", "ref", &second.0))
        .await;
    assert_eq!(second_path["result"], "Projects/Plans/a (2).txt");
    let first_path = client
        .call("locate", item_call("ana", "ref", &first.0))
        .await;
    assert_eq!(first_path["result"], "Projects/Plans/a.txt");

    let identified = client
        .call(
            "identify",
            item_call("ana", "path", "Projects/Plans/a (2).txt"),
        )
        .await;
    assert_eq!(
        identified["result"],
        json!({"item": "remote", "ref": second.0})
    );
    let again = client
        .call("locate", item_call("ana", "ref", &second.0))
        .await;
    assert_eq!(again["result"], "Projects/Plans/a (2).txt");
    let top = client.call("identify", item_call("ana", "path", "")).await;
    assert_eq!(top["result"], json!({"item": "remote"}));
}

#[tokio::test]
async fn identify_of_a_new_file_says_it_is_not_on_the_remote_yet() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    let vfs = harness.fake.vfs.lock().unwrap()["drive"].clone();
    let (_entry, _handle) = vfs
        .create(Vfs::ROOT, "draft.txt", Access::Write)
        .await
        .unwrap();
    let identified = client
        .call("identify", item_call("ana", "path", "draft.txt"))
        .await;
    assert_eq!(identified["result"], json!({"item": "new"}));
    let through = client
        .call("identify", item_call("ana", "path", "draft.txt/x"))
        .await;
    assert_eq!(code(&through), -1019);

    // The new file keeps its name, so a remote file with the same name shows with a suffix.
    let memory = provider(&harness, "ana");
    let remote = memory.add_file(&memory.root(), "draft.txt", b"r");
    let found = client
        .call("locate", item_call("ana", "ref", &remote.0))
        .await;
    assert_eq!(found["result"], "draft (2).txt");
}

#[tokio::test]
async fn a_missing_item_answers_no_such_item_and_an_item_without_a_place_answers_null() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    let missing = client
        .call("locate", item_call("ana", "ref", "item-99"))
        .await;
    assert_eq!(code(&missing), -1019);
    let no_folder = client
        .call("identify", item_call("ana", "path", "Nope/a.txt"))
        .await;
    assert_eq!(code(&no_folder), -1019);

    let memory = provider(&harness, "ana");
    let folder = memory.add_folder(&memory.root(), "Gone");
    let file = memory.add_file(&folder, "a.txt", b"a");
    memory.remote_remove(&folder);
    let nowhere = client
        .call("locate", item_call("ana", "ref", &file.0))
        .await;
    assert!(nowhere["error"].is_null(), "{nowhere}");
    assert!(nowhere["result"].is_null());
}

#[tokio::test]
async fn locate_lists_again_when_a_held_listing_is_older_than_the_item() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    let memory = provider(&harness, "ana");
    let a = memory.add_file(&memory.root(), "a.txt", b"a");
    let known = client
        .call("identify", item_call("ana", "path", "a.txt"))
        .await;
    assert_eq!(known["result"]["ref"], a.0);
    let b = memory.add_file(&memory.root(), "b.txt", b"b");
    let found = client.call("locate", item_call("ana", "ref", &b.0)).await;
    assert_eq!(found["result"], "b.txt");
}

#[tokio::test]
async fn refresh_makes_the_next_listing_show_a_remote_change() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    client.call("mount", mount("ana", "drive")).await;
    let memory = provider(&harness, "ana");
    memory.add_file(&memory.root(), "a.txt", b"a");
    client
        .call("identify", item_call("ana", "path", "a.txt"))
        .await;
    let b = memory.add_file(&memory.root(), "b.txt", b"b");
    let stale = client
        .call("identify", item_call("ana", "path", "b.txt"))
        .await;
    assert_eq!(code(&stale), -1019);
    let refreshed = client.call("refresh", account("ana")).await;
    assert!(refreshed["result"].is_null());
    assert!(refreshed["error"].is_null(), "{refreshed}");
    let found = client
        .call("identify", item_call("ana", "path", "b.txt"))
        .await;
    assert_eq!(found["result"], json!({"item": "remote", "ref": b.0}));
}

#[tokio::test]
async fn requests_about_items_need_a_mount_that_is_up() {
    let harness = start();
    let mut client = Client::connect(&harness.socket).await;
    assert_eq!(
        code(&client.call("locate", item_call("ana", "ref", "x")).await),
        -1018
    );
    assert_eq!(
        code(
            &client
                .call("identify", item_call("ana", "path", "a.txt"))
                .await
        ),
        -1018
    );
    assert_eq!(code(&client.call("refresh", account("ana")).await), -1018);

    *harness.fake.fail_mount.lock().unwrap() = Some(MountError::Adapter("no".into()));
    assert_eq!(
        code(&client.call("mount", mount("ana", "drive")).await),
        -1006
    );
    assert_eq!(
        code(&client.call("locate", item_call("ana", "ref", "x")).await),
        -1018
    );
    assert_eq!(
        code(
            &client
                .call("identify", item_call("ana", "path", "a.txt"))
                .await
        ),
        -1018
    );
    assert_eq!(code(&client.call("refresh", account("ana")).await), -1018);

    let mounted = client.call("mount", mount("ana", "drive")).await;
    assert!(mounted["error"].is_null(), "{mounted}");
    fail(&harness, "ana", Some(VfsError::Offline));
    assert_eq!(
        code(&client.call("locate", item_call("ana", "ref", "x")).await),
        -1014
    );
}
