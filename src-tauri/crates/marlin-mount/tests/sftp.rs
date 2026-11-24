//! The helper with its real providers against OpenSSH's `sshd`, run as the user on 127.0.0.1 behind a TCP relay.
//! The tests that mount need FUSE. `crates/marlin-fuse/test-in-docker.sh` runs them on Linux, and `crates/marlin-fuse/test-on-mac.sh` runs them on a Mac.
//! All of them need `/usr/sbin/sshd` and `ssh-keygen`, and one needs `ssh-agent` and `ssh-add`.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use marlin_mount::protocol::{error_codes, MountState, MountStatus, ReadOnly};
use marlin_mount::{
    Adapter, Config, LinkedProviders, MountError, OsAdapter, Server, Session, SmbConfig,
    MOUNT_FOLDER_MARKER,
};
use marlin_mount_client::{mount_name, Client, ClientError, Event, Start};
use marlin_test_sshd::{Proxy, Sshd};
use marlin_vfs::{Options, Vfs};
use serde_json::{json, Value};

/// Fails a test that hangs, instead of holding CI.
async fn within(limit: Duration, test: impl std::future::Future<Output = ()>) {
    tokio::time::timeout(limit, test)
        .await
        .expect("the test did not finish within its time limit");
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| ((index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as u8)
        .collect()
}

/// The adapter for the tests that must not mount: a mount that reaches it has passed its first call. A regression must not mount a server on the developer's machine.
struct NoMount;

const NO_MOUNT: &str = "this test mounts nothing";

#[async_trait::async_trait]
impl Adapter for NoMount {
    async fn mount(&self, _: Arc<Vfs>, _: &Path) -> Result<Box<dyn Session>, MountError> {
        Err(MountError::Adapter(NO_MOUNT.into()))
    }

    async fn is_dead_mount(&self, _: &Path) -> bool {
        false
    }

    async fn unmount_dead(&self, _: &Path) -> Result<(), MountError> {
        Ok(())
    }
}

fn rpc(error: ClientError) -> (i32, String) {
    match error {
        ClientError::Rpc(error) => (error.code, error.message),
        other => panic!("expected a helper error, got {other}"),
    }
}

async fn mount_sftp(
    client: &Client,
    account: &str,
    name: &str,
) -> Result<MountStatus, ClientError> {
    client
        .call(
            "mount",
            json!({"provider": "sftp", "account": account, "name": name}),
        )
        .await
}

fn account_of(user: &str, port: u16) -> String {
    format!("{user}@127.0.0.1:{port}")
}

fn save_key(servers: &Path, user: &str, host: &str, port: u16, key: PathBuf) {
    marlin_sftp::add_server(
        servers,
        host.to_string(),
        port,
        user.to_string(),
        None,
        "key".into(),
        Some(key.to_string_lossy().into_owned()),
    )
    .unwrap();
}

/// One `sshd`, a relay per saved server, and the helper's server in this process with a client on its socket.
struct Lab {
    sshd: Sshd,
    proxies: Vec<Proxy>,
    servers: PathBuf,
    known_hosts: PathBuf,
    root: PathBuf,
    client: Client,
    events: tokio::sync::mpsc::UnboundedReceiver<Event>,
    signal: tokio::sync::oneshot::Sender<()>,
    run: tokio::task::JoinHandle<()>,
    /// Holds the socket, the mount folder, the spool, `servers`, and `known_hosts`.
    _dir: tempfile::TempDir,
}

impl Lab {
    async fn start(adapter: Arc<dyn Adapter>, proxies: usize, options: Options) -> Self {
        marlin_test_keychain::install();
        let sshd = Sshd::start().await;
        let mut started = Vec::with_capacity(proxies);
        for _ in 0..proxies {
            started.push(Proxy::start(sshd.port()).await);
        }
        let dir = tempfile::tempdir().unwrap();
        let servers = dir.path().join("sftp-servers.json");
        let known_hosts = dir.path().join("known_hosts");
        let mut lines = String::new();
        for proxy in &started {
            lines.push_str(&sshd.known_hosts_line(proxy.port()));
        }
        std::fs::write(&known_hosts, lines).unwrap();

        let socket = dir.path().join("mount.sock");
        let root = dir.path().join("Marlin");
        let server = Server::bind(
            Config {
                socket: socket.clone(),
                root: root.clone(),
                spool: dir.path().join("spool"),
                options,
            },
            adapter,
            Arc::new(LinkedProviders::new(
                marlin_gdrive::Config::default(),
                || Err("no OAuth client in this test".into()),
                marlin_sftp::Config {
                    servers: servers.clone(),
                    known_hosts: known_hosts.clone(),
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
        Self {
            sshd,
            proxies: started,
            servers,
            known_hosts,
            root,
            client,
            events,
            signal,
            run,
            _dir: dir,
        }
    }

    async fn stop(self) {
        let _ = self.signal.send(());
        self.run.await.unwrap();
    }

    /// Saves the server behind proxy `index` with the `ed25519` key, and mounts it. Returns the account, the mount's status, and the server's `files` folder as the mount shows it.
    async fn mount(&self, index: usize) -> (String, MountStatus, PathBuf) {
        let port = self.proxies[index].port();
        save_key(
            &self.servers,
            self.sshd.user(),
            "127.0.0.1",
            port,
            self.sshd.key("ed25519"),
        );
        let account = account_of(self.sshd.user(), port);
        let status = mount_sftp(&self.client, &account, &name_of(&account))
            .await
            .unwrap();
        // The mount's top is the server's `/`.
        let files = PathBuf::from(&status.mount_point)
            .join(self.sshd.root().strip_prefix("/").unwrap())
            .join("files");
        (account, status, files)
    }
}

fn name_of(account: &str) -> String {
    mount_name::build("sftp", account, &[])
}

async fn changed(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    state: MountState,
    limit: Duration,
) -> MountStatus {
    tokio::time::timeout(limit, async {
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

async fn read_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::read(path))
        .await
        .unwrap()
}

#[tokio::test]
async fn an_unknown_or_changed_host_key_is_refused_and_leaves_no_folder() {
    within(Duration::from_secs(60), async {
        let lab = Lab::start(Arc::new(NoMount), 1, Options::default()).await;
        let port = lab.proxies[0].port();
        save_key(
            &lab.servers,
            lab.sshd.user(),
            "127.0.0.1",
            port,
            lab.sshd.key("ed25519"),
        );
        std::fs::write(&lab.known_hosts, "").unwrap();
        let account = account_of(lab.sshd.user(), port);
        let name = name_of(&account);
        let failed = mount_sftp(&lab.client, &account, &name).await.unwrap_err();
        let (code, message) = rpc(failed);
        assert_eq!(code, error_codes::SIGN_IN_NEEDED);
        assert_eq!(message, "the account needs a new sign-in");
        assert!(!lab.root.join(&name).exists());

        let other = lab.sshd.root().join("other-host");
        let generated = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&other)
            .output()
            .unwrap();
        assert!(generated.status.success());
        let public = std::fs::read_to_string(lab.sshd.root().join("other-host.pub")).unwrap();
        std::fs::write(
            &lab.known_hosts,
            format!("[127.0.0.1]:{port} {}\n", public.trim_end()),
        )
        .unwrap();
        let failed = mount_sftp(&lab.client, &account, &name).await.unwrap_err();
        let (code, _) = rpc(failed);
        assert_eq!(code, error_codes::SIGN_IN_NEEDED);
        assert!(!lab.root.join(&name).exists());

        // The trusted key is tried at once, and the mount gets past its first call to the adapter.
        std::fs::write(&lab.known_hosts, lab.sshd.known_hosts_line(port)).unwrap();
        let failed = mount_sftp(&lab.client, &account, &name).await.unwrap_err();
        assert_eq!(rpc(failed), (error_codes::ADAPTER, NO_MOUNT.to_string()));
        lab.stop().await;
    })
    .await;
}

#[tokio::test]
async fn a_server_out_of_reach_not_saved_or_refused_fails_the_mount() {
    within(Duration::from_secs(60), async {
        let lab = Lab::start(Arc::new(NoMount), 1, Options::default()).await;
        let user = lab.sshd.user().to_string();
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        save_key(
            &lab.servers,
            &user,
            "127.0.0.1",
            closed,
            lab.sshd.key("ed25519"),
        );
        let unreachable = account_of(&user, closed);
        let unreachable_name = name_of(&unreachable);
        let failed = mount_sftp(&lab.client, &unreachable, &unreachable_name)
            .await
            .unwrap_err();
        assert_eq!(rpc(failed).0, error_codes::UNREACHABLE);
        assert!(!lab.root.join(&unreachable_name).exists());

        let missing = account_of(&user, 9);
        let missing_name = name_of(&missing);
        let failed = mount_sftp(&lab.client, &missing, &missing_name)
            .await
            .unwrap_err();
        assert_eq!(rpc(failed).0, error_codes::SIGN_IN_NEEDED);
        assert!(!lab.root.join(&missing_name).exists());

        let failed = mount_sftp(&lab.client, "no-at-sign", "bad")
            .await
            .unwrap_err();
        let (code, message) = rpc(failed);
        assert_eq!(code, error_codes::PROVIDER);
        assert_eq!(
            message,
            "the account is not an SFTP server: user@host, or user@host:port"
        );
        assert!(!lab.root.join("bad").exists());

        let port = lab.proxies[0].port();
        save_key(
            &lab.servers,
            &user,
            "127.0.0.1",
            port,
            lab.sshd.key("unauthorized"),
        );
        let refused = account_of(&user, port);
        let refused_name = name_of(&refused);
        let failed = mount_sftp(&lab.client, &refused, &refused_name)
            .await
            .unwrap_err();
        assert_eq!(rpc(failed).0, error_codes::SIGN_IN_NEEDED);
        assert!(!lab.root.join(&refused_name).exists());
        lab.stop().await;
    })
    .await;
}

#[tokio::test]
async fn a_refused_sign_in_is_kept_across_spellings_of_one_account() {
    within(Duration::from_secs(60), async {
        let lab = Lab::start(Arc::new(NoMount), 1, Options::default()).await;
        let port = lab.proxies[0].port();
        let user = lab.sshd.user();
        // `localhost` reaches the proxy on 127.0.0.1. The two spellings share one sign-in.
        let line = lab.sshd.known_hosts_line(port);
        let key = line.split_once(' ').unwrap().1;
        std::fs::write(&lab.known_hosts, format!("[localhost]:{port} {key}")).unwrap();
        save_key(
            &lab.servers,
            user,
            "localhost",
            port,
            lab.sshd.key("unauthorized"),
        );
        let first = format!("{user}@localhost:{port}");
        let first_name = name_of(&first);
        let failed = mount_sftp(&lab.client, &first, &first_name)
            .await
            .unwrap_err();
        assert_eq!(rpc(failed).0, error_codes::SIGN_IN_NEEDED);
        assert_eq!(lab.proxies[0].accepted(), 1);

        let second = format!("{user}@LocalHost:{port}");
        let failed = mount_sftp(&lab.client, &second, "sftp-other")
            .await
            .unwrap_err();
        assert_eq!(rpc(failed).0, error_codes::SIGN_IN_NEEDED);
        assert_eq!(lab.proxies[0].accepted(), 1);
        lab.stop().await;
    })
    .await;
}

/// Stops the helper process when a failed assertion ends the test early.
struct KillOnDrop(Option<Child>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct Agent(u32);

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = Command::new("kill").arg(self.0.to_string()).status();
    }
}

fn spawn_helper(
    socket: &Path,
    root: &Path,
    spool: &Path,
    home: &Path,
    auth_sock: Option<&Path>,
    stderr: std::fs::File,
) -> KillOnDrop {
    let mut command = Command::new(env!("CARGO_BIN_EXE_marlin-mount"));
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .arg("--socket")
        .arg(socket)
        .arg("--root")
        .arg(root)
        .arg("--spool")
        .arg(spool)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);
    if let Some(auth_sock) = auth_sock {
        command.env("SSH_AUTH_SOCK", auth_sock);
    }
    KillOnDrop(Some(command.spawn().unwrap()))
}

/// Waits for the helper to exit and returns its exit code. It polls, so the test's own time limit still applies, and the helper stays in its guard until it has exited.
async fn exit_code(child: &mut KillOnDrop) -> Option<i32> {
    loop {
        if let Some(status) = child.0.as_mut().unwrap().try_wait().unwrap() {
            child.0 = None;
            return status.code();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_for_socket(socket: &Path) {
    for _ in 0..500 {
        if tokio::net::UnixStream::connect(socket).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the helper did not take a connection");
}

async fn connect_built(socket: &Path, log: &Path) -> Client {
    let (sender, _events) = tokio::sync::mpsc::unbounded_channel();
    Client::connect(
        &Start {
            // Never the real binary: a client that cannot connect starts one, with the user's real home folder.
            binary: log.with_file_name("not-started"),
            socket: socket.to_path_buf(),
            root: None,
            log: log.to_path_buf(),
        },
        move |event| {
            let _ = sender.send(event);
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn an_agent_sign_in_uses_the_helpers_ssh_auth_sock() {
    // An agent server reads no keychain, which is why a built helper may get `sftp` here. A test never sends `gdrive` to it.
    marlin_test_keychain::install();
    within(Duration::from_secs(60), async {
        let sshd = Sshd::start().await;
        let proxy = Proxy::start(sshd.port()).await;
        let home = tempfile::tempdir().unwrap();
        let known_hosts = home.path().join(".ssh/known_hosts");
        std::fs::create_dir_all(known_hosts.parent().unwrap()).unwrap();
        std::fs::write(&known_hosts, sshd.known_hosts_line(proxy.port())).unwrap();
        // The helper finds its servers file under its own `HOME`, through `dirs::config_dir()`.
        let servers = if cfg!(target_os = "macos") {
            home.path()
                .join("Library/Application Support/marlin/sftp-servers.json")
        } else {
            home.path().join(".config/marlin/sftp-servers.json")
        };
        marlin_sftp::add_server(
            &servers,
            "127.0.0.1".into(),
            proxy.port(),
            sshd.user().to_string(),
            None,
            "agent".into(),
            None,
        )
        .unwrap();

        let agent_dir = tempfile::Builder::new()
            .prefix("marlin-agent-")
            .tempdir_in("/tmp")
            .unwrap();
        let sock = agent_dir.path().join("agent.sock");
        // `-s` asks for Bourne shell lines whatever the user's shell is.
        let started = Command::new("ssh-agent")
            .arg("-s")
            .arg("-a")
            .arg(&sock)
            .output()
            .unwrap();
        assert!(
            started.status.success(),
            "{}",
            String::from_utf8_lossy(&started.stderr)
        );
        let text = String::from_utf8_lossy(&started.stdout);
        let pid = text
            .split("SSH_AGENT_PID=")
            .nth(1)
            .and_then(|rest| rest.split([';', '\n']).next())
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let _agent = Agent(pid);
        let added = Command::new("ssh-add")
            .env("SSH_AUTH_SOCK", &sock)
            .arg(sshd.key("ed25519"))
            .output()
            .unwrap();
        assert!(
            added.status.success(),
            "{}",
            String::from_utf8_lossy(&added.stderr)
        );

        let work = tempfile::tempdir().unwrap();
        let root = work.path().join("Marlin");
        let socket = work.path().join("mount.sock");
        let spool = work.path().join("spool");
        let account = account_of(sshd.user(), proxy.port());
        let name = name_of(&account);
        // A file in the mount point fails the mount with -1003 after its first call, so the sign-in is proved without mounting. The marker makes the mount folder the helper's, though it is not empty.
        std::fs::create_dir_all(root.join(&name)).unwrap();
        std::fs::write(root.join(MOUNT_FOLDER_MARKER), b"").unwrap();
        std::fs::write(root.join(&name).join("not-empty"), b"x").unwrap();

        let stderr = std::fs::File::create(work.path().join("helper.err")).unwrap();
        let mut child = spawn_helper(&socket, &root, &spool, home.path(), Some(&sock), stderr);
        wait_for_socket(&socket).await;
        let client = connect_built(&socket, &work.path().join("helper.log")).await;
        let failed = mount_sftp(&client, &account, &name).await.unwrap_err();
        assert_eq!(rpc(failed).0, error_codes::MOUNT_POINT_NOT_EMPTY);
        client.call::<Value>("shutdown", Value::Null).await.unwrap();
        assert_eq!(exit_code(&mut child).await, Some(0));

        let stderr = std::fs::File::create(work.path().join("helper-again.err")).unwrap();
        let mut child = spawn_helper(&socket, &root, &spool, home.path(), None, stderr);
        wait_for_socket(&socket).await;
        let client = connect_built(&socket, &work.path().join("helper.log")).await;
        let failed = mount_sftp(&client, &account, &name).await.unwrap_err();
        assert_eq!(rpc(failed).0, error_codes::SIGN_IN_NEEDED);
        client.call::<Value>("shutdown", Value::Null).await.unwrap();
        assert_eq!(exit_code(&mut child).await, Some(0));
    })
    .await;
}

fn server_files(root: &Path) {
    let files = root.join("files");
    std::fs::create_dir_all(files.join("sub")).unwrap();
    std::fs::write(files.join("big.bin"), pattern(300_000)).unwrap();
    std::fs::write(files.join("small.txt"), b"small-text\n").unwrap();
}

/// Each name in `dir`, with the bytes of each file.
async fn snapshot(dir: &Path) -> Vec<(String, Option<Vec<u8>>)> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            let bytes = if entry.file_type().unwrap().is_file() {
                Some(std::fs::read(entry.path()).unwrap())
            } else {
                None
            };
            found.push((name, bytes));
        }
        found.sort();
        found
    })
    .await
    .unwrap()
}

/// The OS error a file system call that must fail gets.
async fn os_error(call: impl FnOnce() -> std::io::Result<()> + Send + 'static) -> Option<i32> {
    tokio::task::spawn_blocking(call)
        .await
        .unwrap()
        .unwrap_err()
        .raw_os_error()
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_saved_server_mounts_read_only_and_reads_through_fuse() {
    within(Duration::from_secs(120), async {
        let lab = Lab::start(Arc::new(OsAdapter), 1, Options::default()).await;
        server_files(lab.sshd.root());
        let (account, status, files) = lab.mount(0).await;
        assert_eq!(status.state, MountState::Mounted);
        assert_eq!(status.read_only, Some(ReadOnly::Provider));
        let point = PathBuf::from(&status.mount_point);
        assert_eq!(
            point,
            lab.root.join(format!(
                "sftp-{}@127.0.0.1_{}",
                lab.sshd.user(),
                lab.proxies[0].port()
            ))
        );
        // The folder lists, another program reads a file whole, and a read at an offset gets its piece.
        let (listed, cat, piece) = tokio::task::spawn_blocking({
            let files = files.clone();
            move || {
                use std::io::{Read, Seek, SeekFrom};
                let mut listed: Vec<String> = std::fs::read_dir(&files)
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                    .collect();
                listed.sort();
                let cat = Command::new("cat")
                    .arg(files.join("big.bin"))
                    .output()
                    .unwrap();
                let mut file = std::fs::File::open(files.join("big.bin")).unwrap();
                file.seek(SeekFrom::Start(200_000)).unwrap();
                let mut piece = vec![0; 1_000];
                file.read_exact(&mut piece).unwrap();
                (listed, cat, piece)
            }
        })
        .await
        .unwrap();
        assert_eq!(listed, ["big.bin", "small.txt", "sub"]);
        assert!(cat.status.success());
        assert_eq!(cat.stdout, pattern(300_000));
        assert_eq!(piece, pattern(300_000)[200_000..201_000]);

        let on_server = lab.sshd.root().join("files");
        let before = snapshot(&on_server).await;
        let small = files.join("small.txt");
        let path = files.join("new.txt");
        assert_eq!(
            os_error(move || std::fs::write(path, b"x")).await,
            Some(libc::EROFS)
        );
        let path = small.clone();
        assert_eq!(
            os_error(move || std::fs::write(path, b"changed")).await,
            Some(libc::EROFS)
        );
        let path = small.clone();
        assert_eq!(
            os_error(move || std::fs::remove_file(path)).await,
            Some(libc::EROFS)
        );
        let (from, to) = (small.clone(), files.join("other.txt"));
        assert_eq!(
            os_error(move || std::fs::rename(from, to)).await,
            Some(libc::EROFS)
        );
        let path = files.join("made");
        assert_eq!(
            os_error(move || std::fs::create_dir(path)).await,
            Some(libc::EROFS)
        );
        assert_eq!(snapshot(&on_server).await, before);

        lab.client
            .call::<Value>("unmount", json!({"provider": "sftp", "account": account}))
            .await
            .unwrap();
        assert!(!point.exists());
        // The mount's SSH connection closed with it.
        let deadline = Instant::now() + Duration::from_secs(5);
        while lab.proxies[0].open() != 0 {
            assert!(Instant::now() < deadline, "the SSH connection stayed open");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        lab.stop().await;
    })
    .await;
}

/// It drives Finder, which can take the focus: on a Mac someone is working at, ask before running it.
#[cfg(target_os = "macos")]
#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn finder_lists_and_copies_from_an_sftp_mount() {
    within(Duration::from_secs(120), async {
        let lab = Lab::start(Arc::new(OsAdapter), 1, Options::default()).await;
        server_files(lab.sshd.root());
        let (account, _, files) = lab.mount(0).await;
        let finder = |script: String| {
            tokio::task::spawn_blocking(move || {
                let output = Command::new("osascript")
                    .arg("-e")
                    .arg(script)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                String::from_utf8(output.stdout).unwrap()
            })
        };

        let listed = finder(format!(
            "tell application \"Finder\" to get name of every item of (POSIX file \"{}\" as alias)",
            files.display()
        ))
        .await
        .unwrap();
        for name in ["big.bin", "small.txt", "sub"] {
            assert!(listed.contains(name), "{listed}");
        }
        let copied = tempfile::tempdir().unwrap();
        finder(format!(
            "tell application \"Finder\" to duplicate (POSIX file \"{}\" as alias) to (POSIX file \"{}\" as alias)",
            files.join("small.txt").display(),
            copied.path().display()
        ))
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(copied.path().join("small.txt")).unwrap(),
            b"small-text\n"
        );

        lab
            .client
            .call::<Value>("unmount", json!({"provider": "sftp", "account": account}))
            .await
            .unwrap();
        lab.stop().await;
    })
    .await;
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_hung_server_times_out_and_other_mounts_keep_working() {
    within(Duration::from_secs(120), async {
        let options = Options {
            call_timeout: Duration::from_secs(3),
            ..Options::default()
        };
        let lab = Lab::start(Arc::new(OsAdapter), 2, options).await;
        server_files(lab.sshd.root());
        let (_, _, hung) = lab.mount(0).await;
        let (_, _, other) = lab.mount(1).await;
        // The connection is up before it hangs.
        read_file(&hung.join("small.txt")).await.unwrap();
        lab.proxies[0].freeze();
        let started = Instant::now();
        let reading = tokio::task::spawn_blocking({
            let path = hung.join("big.bin");
            move || std::fs::read(path)
        });
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            read_file(&other.join("big.bin")).await.unwrap(),
            pattern(300_000)
        );
        assert!(
            !reading.is_finished(),
            "the hung read ended before the other mount answered"
        );
        // The hung call fails at the mount's 3-second limit, long before the provider gives up on a silent connection (15 seconds). The open it cancelled retires that connection.
        let read = reading.await.unwrap();
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_secs(2) && elapsed < Duration::from_secs(15),
            "{elapsed:?}"
        );
        // macOS hands the error to the reader.
        #[cfg(target_os = "macos")]
        assert_eq!(read.unwrap_err().raw_os_error(), Some(libc::ETIMEDOUT));
        // Linux reads the page again after the error, and that call runs on a new connection: the relay froze only the connections that were open.
        #[cfg(target_os = "linux")]
        {
            assert_eq!(read.unwrap(), pattern(300_000));
            assert_eq!(lab.proxies[0].accepted(), 2);
        }
        lab.stop().await;
    })
    .await;
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_cut_connection_is_rebuilt_and_an_unreachable_server_shows_offline() {
    within(Duration::from_secs(120), async {
        let mut lab = Lab::start(Arc::new(OsAdapter), 1, Options::default()).await;
        server_files(lab.sshd.root());
        // Files nothing has read yet, so each read below is a provider call.
        for name in ["u1", "u2", "u3", "u4", "u5", "u6"] {
            std::fs::write(lab.sshd.root().join("files").join(name), name).unwrap();
        }
        let (account, _, files) = lab.mount(0).await;
        read_file(&files.join("small.txt")).await.unwrap();
        assert_eq!(lab.proxies[0].accepted(), 1);
        lab.proxies[0].cut();
        assert_eq!(
            read_file(&files.join("big.bin")).await.unwrap(),
            pattern(300_000)
        );
        assert_eq!(lab.proxies[0].accepted(), 2);

        lab.proxies[0].refuse(true);
        lab.proxies[0].cut();
        for name in ["u1", "u2", "u3", "u4", "u5"] {
            let _ = read_file(&files.join(name)).await;
        }
        changed(
            &mut lab.events,
            MountState::Offline,
            Duration::from_secs(20),
        )
        .await;
        lab.proxies[0].refuse(false);
        read_file(&files.join("u6")).await.unwrap();
        changed(
            &mut lab.events,
            MountState::Mounted,
            Duration::from_secs(20),
        )
        .await;
        lab.client
            .call::<Value>("unmount", json!({"provider": "sftp", "account": account}))
            .await
            .unwrap();
        lab.stop().await;
    })
    .await;
}
