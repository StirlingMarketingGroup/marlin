//! The SFTP provider against OpenSSH's `sshd`, run as the user on 127.0.0.1, and against a small SSH server inside the test for what OpenSSH cannot show. They need `/usr/sbin/sshd` and `ssh-keygen`.

#![cfg(unix)]

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::future::Future;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use marlin_sftp::{add_server, Config, SftpAccount, SftpProvider};
use marlin_test_sshd::{Proxy, Sshd};
use marlin_vfs::{ItemId, Kind, Provider, Space, VfsError};
use russh::keys::{Algorithm, PrivateKey};
use russh::server::{Auth, Handler, Msg, Server, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{
    Attrs, Data, FileAttributes, Handle, OpenFlags, Packet, Status, StatusCode, Version,
};
use tokio::net::TcpListener;
use tokio::sync::Notify;

/// The bytes the test files hold: byte `i` of a file is `pattern(0, n)[i]`. They never repeat in a way that would hide two pieces of a read swapped or sent twice.
fn pattern(start: u64, len: usize) -> Vec<u8> {
    (start..start + len as u64)
        .map(|i| (i.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as u8)
        .collect()
}

fn id_of(path: &Path) -> ItemId {
    ItemId(path.to_string_lossy().into_owned())
}

/// Fails a test that hangs, instead of holding CI.
async fn within(test: impl Future<Output = ()>) {
    tokio::time::timeout(Duration::from_secs(60), test)
        .await
        .expect("the test did not finish within 60 seconds");
}

/// OpenSSH's `sshd` with key sign-in only, behind a [`Proxy`]. The folder holds the client keys `ed25519` and `rsa`, which `authorized_keys` names, and `unauthorized`, which it does not.
struct Harness {
    sshd: Sshd,
    proxy: Proxy,
    root: PathBuf,
    user: String,
    servers: PathBuf,
    known_hosts: PathBuf,
}

impl Harness {
    async fn start() -> Self {
        let sshd = Sshd::start().await;
        let proxy = Proxy::start(sshd.port()).await;
        let known_hosts = sshd.root().join("known_hosts");
        fs::write(&known_hosts, sshd.known_hosts_line(proxy.port())).unwrap();
        let root = sshd.root().to_path_buf();
        let user = sshd.user().to_string();
        Self {
            sshd,
            proxy,
            user,
            servers: root.join("sftp-servers.json"),
            known_hosts,
            root,
        }
    }

    /// Saves the server with the client key `key`.
    fn save(&self, key: &str) {
        add_server(
            &self.servers,
            "127.0.0.1".into(),
            self.proxy.port(),
            self.user.clone(),
            None,
            "key".into(),
            Some(self.sshd.key(key).to_string_lossy().into_owned()),
        )
        .unwrap();
    }

    fn config(&self, retry_after: Duration) -> Config {
        Config {
            servers: self.servers.clone(),
            known_hosts: self.known_hosts.clone(),
            retry_after,
        }
    }

    fn account(&self, retry_after: Duration) -> Arc<SftpAccount> {
        Arc::new(SftpAccount::new(
            &self.user,
            "127.0.0.1",
            self.proxy.port(),
            self.config(retry_after),
        ))
    }

    fn provider(&self, retry_after: Duration) -> SftpProvider {
        SftpProvider::new(self.account(retry_after))
    }
}

/// Lists `folder` and every folder under it, and adds each path under `folder` to `out`.
async fn walk(provider: &SftpProvider, folder: &str, depth: usize, out: &mut BTreeSet<String>) {
    assert!(depth <= 10, "the walk passed depth 10 at {folder}");
    for item in provider.list(&ItemId(folder.to_string())).await.unwrap() {
        out.insert(item.id.0.clone());
        if item.attributes.kind == Kind::Folder {
            Box::pin(walk(provider, &item.id.0, depth + 1, out)).await;
        }
    }
}

#[tokio::test]
async fn sshd_shows_listings_reads_links_and_failures() {
    marlin_test_keychain::install();
    within(async {
        let harness = Harness::start().await;
        harness.save("ed25519");
        let provider = harness.provider(Duration::from_secs(60));

        let listed = harness.root.join("listed");
        fs::create_dir_all(listed.join("sub")).unwrap();
        let data = listed.join("data.bin");
        fs::write(&data, b"hello").unwrap();
        let modified = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        fs::File::options()
            .write(true)
            .open(&data)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        let odd = listed.join("a name % café.txt");
        fs::write(&odd, "é").unwrap();
        let fifo = Command::new("mkfifo").arg(listed.join("fifo")).status();
        assert!(fifo.unwrap().success());

        let mut items = provider.list(&id_of(&listed)).await.unwrap();
        items.sort_by(|a, b| a.name.cmp(&b.name));
        let sub = listed.join("sub");
        let expected = [
            (&odd, Kind::File, 2),
            (&data, Kind::File, 5),
            (&sub, Kind::Folder, 0),
        ];
        assert_eq!(items.len(), expected.len(), "{items:?}");
        let seconds = |time: SystemTime| time.duration_since(UNIX_EPOCH).unwrap().as_secs();
        for (item, (path, kind, size)) in items.iter().zip(expected) {
            assert_eq!(item.id, id_of(path));
            assert_eq!(item.name, path.file_name().unwrap().to_string_lossy());
            assert_eq!((item.attributes.kind, item.attributes.size), (kind, size));
            let on_disk = fs::metadata(path).unwrap().modified().unwrap();
            assert_eq!(seconds(item.attributes.modified), seconds(on_disk));
        }
        assert_eq!(items[1].attributes.modified, modified);
        assert_eq!(provider.item(&items[1].id).await.unwrap(), items[1]);
        let root = provider.item(&provider.root()).await.unwrap();
        assert_eq!(
            (root.name.as_str(), root.attributes.kind),
            ("", Kind::Folder)
        );
        let space = provider.space().await.unwrap();
        assert!(space.total > 0 && space.free <= space.total, "{space:?}");
        assert_ne!(space.total, 1 << 50, "OpenSSH has statvfs");

        // A read at an offset other than 0, across the end, at the end, and past it.
        let bytes = pattern(0, 5 * 1024 * 1024);
        let big = id_of(&harness.root.join("big.bin"));
        fs::write(&big.0, &bytes).unwrap();
        let read = |offset: usize, length: u32| provider.read(&big, offset as u64, length);
        assert_eq!(
            read(1_234_567, 1 << 20).await.unwrap(),
            bytes[1_234_567..1_234_567 + (1 << 20)]
        );
        let tail = bytes.len() - 100;
        assert_eq!(read(tail, 1_000).await.unwrap(), bytes[tail..]);
        assert_eq!(read(bytes.len(), 10).await.unwrap(), b"");
        assert_eq!(read(bytes.len() + 50, 10).await.unwrap(), b"");

        let links = harness.root.join("links");
        fs::create_dir_all(links.join("dir")).unwrap();
        fs::write(links.join("target.txt"), b"target-bytes").unwrap();
        fs::write(links.join("dir/inner.txt"), b"inner").unwrap();
        for (name, to) in [
            ("link-file", "target.txt"),
            ("link-dir", "dir"),
            ("broken", "missing"),
            ("loop-a", "loop-b"),
            ("loop-b", "loop-a"),
            ("self", "."),
            ("parent", ".."),
        ] {
            symlink(to, links.join(name)).unwrap();
        }
        let shown = provider.list(&id_of(&links)).await.unwrap();
        let by_name: HashMap<_, _> = shown
            .iter()
            .map(|item| (item.name.as_str(), item))
            .collect();
        assert_eq!(
            by_name.keys().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from(["dir", "link-dir", "link-file", "target.txt"])
        );
        assert_eq!(
            (
                by_name["link-file"].attributes.kind,
                by_name["link-file"].attributes.size
            ),
            (Kind::File, 12)
        );
        assert_eq!(by_name["link-dir"].attributes.kind, Kind::Folder);
        let inner = provider
            .list(&id_of(&links.join("link-dir")))
            .await
            .unwrap();
        assert_eq!(
            inner
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            ["inner.txt"]
        );

        // Links from b to c and from c back to b. Through a's link to b, no link of b or c points at the folder listed or above it, so only the rule that a folder reached through a link shows no folder link keeps the walk finite.
        let tree = harness.root.join("tree");
        let links = [("a", "b"), ("b", "c"), ("c", "b")];
        for (folder, to) in links {
            fs::create_dir_all(tree.join(folder)).unwrap();
            fs::write(tree.join(folder).join("file"), folder).unwrap();
            symlink(
                format!("../{to}"),
                tree.join(folder).join(format!("to-{to}")),
            )
            .unwrap();
        }
        let mut walked = BTreeSet::new();
        walk(&provider, &tree.to_string_lossy(), 0, &mut walked).await;
        let tree = tree.to_string_lossy();
        let expected: BTreeSet<_> = [
            "a",
            "a/file",
            "a/to-b",
            "a/to-b/file",
            "b",
            "b/file",
            "b/to-c",
            "b/to-c/file",
            "c",
            "c/file",
            "c/to-b",
            "c/to-b/file",
        ]
        .iter()
        .map(|path| format!("{tree}/{path}"))
        .collect();
        assert_eq!(walked, expected);

        assert_eq!(
            provider.list(&id_of(&data)).await,
            Err(VfsError::NotAFolder)
        );
        let missing = id_of(&harness.root.join("missing"));
        assert_eq!(provider.item(&missing).await, Err(VfsError::NotFound));
        assert_eq!(provider.list(&missing).await, Err(VfsError::NotFound));
        assert_eq!(provider.read(&missing, 0, 1).await, Err(VfsError::NotFound));

        // Every listing, read, and failure above ran on one connection: a close or an error does not cost a new sign-in.
        assert_eq!(harness.proxy.accepted(), 1);
    })
    .await;
}

#[tokio::test]
async fn an_rsa_key_signs_in_and_an_unknown_or_changed_host_key_does_not() {
    marlin_test_keychain::install();
    within(async {
        let harness = Harness::start().await;
        harness.save("rsa");
        let provider = harness.provider(Duration::from_secs(60));
        provider.list(&id_of(&harness.root)).await.unwrap();

        fs::write(&harness.known_hosts, "").unwrap();
        let unknown = harness.provider(Duration::from_secs(60));
        assert_eq!(
            unknown.item(&unknown.root()).await,
            Err(VfsError::SignInNeeded)
        );

        let other = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let line = format!(
            "[127.0.0.1]:{} {}\n",
            harness.proxy.port(),
            other.public_key().to_openssh().unwrap()
        );
        fs::write(&harness.known_hosts, line).unwrap();
        let changed = harness.provider(Duration::from_secs(60));
        assert_eq!(
            changed.item(&changed.root()).await,
            Err(VfsError::SignInNeeded)
        );
    })
    .await;
}

#[tokio::test]
async fn a_refused_key_waits_and_a_changed_key_is_tried_at_once() {
    marlin_test_keychain::install();
    within(async {
        let harness = Harness::start().await;
        harness.save("unauthorized");
        let provider = harness.provider(Duration::from_secs(2));
        let root = provider.root();
        assert_eq!(provider.item(&root).await, Err(VfsError::SignInNeeded));

        // The key now works, but the provider does not try it before the wait ends.
        let connections = harness.proxy.accepted();
        let key = fs::read(harness.root.join("unauthorized.pub")).unwrap();
        let mut authorized = fs::read(harness.root.join("authorized_keys")).unwrap();
        authorized.extend(key);
        fs::write(harness.root.join("authorized_keys"), authorized).unwrap();
        assert_eq!(provider.item(&root).await, Err(VfsError::SignInNeeded));
        assert_eq!(harness.proxy.accepted(), connections);
        tokio::time::sleep(Duration::from_secs(2)).await;
        provider.item(&root).await.unwrap();

        // A fresh server, where the key is refused again: saving another key works at once.
        let harness = Harness::start().await;
        harness.save("unauthorized");
        let provider = harness.provider(Duration::from_secs(60));
        assert_eq!(provider.item(&root).await, Err(VfsError::SignInNeeded));
        harness.save("ed25519");
        provider.item(&root).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_cut_connection_opens_again_and_a_refused_one_is_offline() {
    marlin_test_keychain::install();
    within(async {
        let mut harness = Harness::start().await;
        harness.save("ed25519");
        let provider = harness.provider(Duration::from_secs(60));
        let root = provider.root();
        provider.item(&root).await.unwrap();
        harness.proxy.cut();
        provider.item(&root).await.unwrap();
        assert_eq!(harness.proxy.accepted(), 2);

        harness.proxy.stop().await;
        harness.proxy.cut();
        assert_eq!(provider.item(&root).await, Err(VfsError::Offline));
    })
    .await;
}

#[tokio::test]
async fn a_connection_that_goes_silent_ends_and_the_call_runs_again() {
    marlin_test_keychain::install();
    within(async {
        let harness = Harness::start().await;
        harness.save("ed25519");
        let provider = harness.provider(Duration::from_secs(60));
        let root = provider.root();
        provider.item(&root).await.unwrap();

        // The provider notices within 20 seconds, inside the core's 30-second call limit, and runs the call again on a new connection.
        harness.proxy.freeze();
        let started = Instant::now();
        provider.item(&root).await.unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(harness.proxy.accepted(), 2);
    })
    .await;
}

/// An SSH server inside the test, with password sign-in. Its SFTP server has no extensions, so no `statvfs` and no `limits`, and it answers one request at a time.
struct Fake {
    port: u16,
    server: FakeServer,
    servers: PathBuf,
    known_hosts: PathBuf,
    _dir: tempfile::TempDir,
}

#[derive(Clone)]
struct FakeServer {
    password: Arc<Mutex<String>>,
    passwords: Arc<AtomicUsize>,
    connections: Arc<AtomicUsize>,
    /// Told when an open of `/hang` arrives. That open never answers.
    hang: Arc<Notify>,
    /// Told when a client ends its SFTP channel.
    ended: Arc<Notify>,
    /// Told when a password arrives.
    password_seen: Arc<Notify>,
    /// A password gets its answer only while this is not held for writing.
    answers: Arc<tokio::sync::RwLock<()>>,
    lost_once: Arc<AtomicBool>,
    /// The SFTP server offers `statvfs@openssh.com` when this is set.
    offers_statvfs: Arc<AtomicBool>,
    /// How many `statvfs` requests came. Each answers that the user may not ask.
    statvfs_asked: Arc<AtomicUsize>,
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
}

impl Server for FakeServer {
    type Handler = Self;

    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        self.connections.fetch_add(1, Ordering::SeqCst);
        Self {
            channels: Arc::default(),
            ..self.clone()
        }
    }
}

impl Handler for FakeServer {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        self.passwords.fetch_add(1, Ordering::SeqCst);
        self.password_seen.notify_one();
        let _answer = self.answers.read().await;
        if *self.password.lock().unwrap() == password {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        self.channels.lock().unwrap().insert(channel.id(), channel);
        Ok(true)
    }

    async fn channel_eof(
        &mut self,
        _channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.ended.notify_one();
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        _name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let stream = self
            .channels
            .lock()
            .unwrap()
            .remove(&channel)
            .unwrap()
            .into_stream();
        session.channel_success(channel)?;
        russh_sftp::server::run(
            stream,
            FakeSftp {
                hang: Arc::clone(&self.hang),
                lost_once: Arc::clone(&self.lost_once),
                offers_statvfs: Arc::clone(&self.offers_statvfs),
                statvfs_asked: Arc::clone(&self.statvfs_asked),
            },
        )
        .await;
        Ok(())
    }
}

/// `/` is a folder. `/short` is the 100,000 bytes of `pattern`, read at most 1,000 bytes per answer. `/denied` is a path the user may not read. Listing `/gone` or `/hidden` answers a generic failure: a stat says `/gone` is not there and `/hidden` may not be read.
struct FakeSftp {
    hang: Arc<Notify>,
    /// The first stat of `/lost` answers that the connection was lost. Every later one answers a folder.
    lost_once: Arc<AtomicBool>,
    offers_statvfs: Arc<AtomicBool>,
    statvfs_asked: Arc<AtomicUsize>,
}

impl russh_sftp::server::Handler for FakeSftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        let mut version = Version::new();
        if self.offers_statvfs.load(Ordering::SeqCst) {
            let statvfs = ("statvfs@openssh.com".to_string(), "2".to_string());
            version.extensions.extend([statvfs]);
        }
        Ok(version)
    }

    async fn extended(
        &mut self,
        _id: u32,
        _request: String,
        _data: Vec<u8>,
    ) -> Result<Packet, Self::Error> {
        self.statvfs_asked.fetch_add(1, Ordering::SeqCst);
        Err(StatusCode::PermissionDenied)
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let mut attrs = FileAttributes::default();
        attrs.set_dir(true);
        match path.as_str() {
            "/" => Ok(Attrs { id, attrs }),
            "/lost" if !self.lost_once.swap(true, Ordering::SeqCst) => {
                Err(StatusCode::ConnectionLost)
            }
            "/lost" => Ok(Attrs { id, attrs }),
            "/denied" | "/hidden" => Err(StatusCode::PermissionDenied),
            _ => Err(StatusCode::NoSuchFile),
        }
    }

    async fn opendir(&mut self, _id: u32, path: String) -> Result<Handle, Self::Error> {
        match path.as_str() {
            "/denied" => Err(StatusCode::PermissionDenied),
            "/gone" | "/hidden" => Err(StatusCode::Failure),
            _ => Err(StatusCode::OpUnsupported),
        }
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        _flags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        match filename.as_str() {
            "/hang" => {
                self.hang.notify_one();
                std::future::pending::<()>().await;
            }
            "/denied" => return Err(StatusCode::PermissionDenied),
            _ => {}
        }
        Ok(Handle {
            id,
            handle: filename,
        })
    }

    async fn read(
        &mut self,
        id: u32,
        _handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let end = 100_000.min(offset + u64::from(len.min(1_000)));
        if offset < end {
            Ok(Data {
                id,
                data: pattern(offset, (end - offset) as usize),
            })
        } else {
            Err(StatusCode::Eof)
        }
    }

    async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: String::new(),
            language_tag: String::new(),
        })
    }
}

impl Fake {
    async fn start(password: &str) -> Self {
        let host_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let host_line = host_key.public_key().to_openssh().unwrap();
        let mut config = russh::server::Config {
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        };
        config.keys.push(host_key);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut server = FakeServer {
            password: Arc::new(Mutex::new(password.to_string())),
            passwords: Arc::new(AtomicUsize::new(0)),
            connections: Arc::new(AtomicUsize::new(0)),
            hang: Arc::new(Notify::new()),
            ended: Arc::new(Notify::new()),
            password_seen: Arc::new(Notify::new()),
            answers: Arc::new(tokio::sync::RwLock::new(())),
            lost_once: Arc::new(AtomicBool::new(false)),
            offers_statvfs: Arc::new(AtomicBool::new(false)),
            statvfs_asked: Arc::new(AtomicUsize::new(0)),
            channels: Arc::new(Mutex::new(HashMap::new())),
        };
        let fake = server.clone();
        tokio::spawn(async move { server.run_on_socket(Arc::new(config), &listener).await });

        let dir = tempfile::tempdir().unwrap();
        let known_hosts = dir.path().join("known_hosts");
        fs::write(&known_hosts, format!("[127.0.0.1]:{port} {host_line}\n")).unwrap();
        Self {
            port,
            server: fake,
            servers: dir.path().join("sftp-servers.json"),
            known_hosts,
            _dir: dir,
        }
    }

    fn save(&self, password: &str) {
        let saved = Some(password.to_string());
        add_server(
            &self.servers,
            "127.0.0.1".into(),
            self.port,
            "user".into(),
            saved,
            "password".into(),
            None,
        )
        .unwrap();
    }

    /// A fake that takes the saved password, and a provider for it.
    async fn signed_in() -> (Self, SftpProvider) {
        let fake = Fake::start("right").await;
        fake.save("right");
        let provider = fake.provider();
        (fake, provider)
    }

    fn account(&self) -> Arc<SftpAccount> {
        Arc::new(SftpAccount::new(
            "user",
            "127.0.0.1",
            self.port,
            Config {
                servers: self.servers.clone(),
                known_hosts: self.known_hosts.clone(),
                retry_after: Duration::from_secs(60),
            },
        ))
    }

    fn provider(&self) -> SftpProvider {
        SftpProvider::new(self.account())
    }
}

#[tokio::test]
async fn a_refused_password_is_never_sent_again() {
    marlin_test_keychain::install();
    within(async {
        let fake = Fake::start("right").await;
        fake.save("wrong");
        let provider = fake.provider();
        let root = provider.root();
        assert_eq!(provider.item(&root).await, Err(VfsError::SignInNeeded));
        assert_eq!(provider.item(&root).await, Err(VfsError::SignInNeeded));
        assert_eq!(fake.server.passwords.load(Ordering::SeqCst), 1);

        // A second wrong password does not make the provider forget the first.
        fake.save("also wrong");
        assert_eq!(provider.item(&root).await, Err(VfsError::SignInNeeded));
        fake.save("wrong");
        assert_eq!(provider.item(&root).await, Err(VfsError::SignInNeeded));
        assert_eq!(fake.server.passwords.load(Ordering::SeqCst), 2);

        fake.save("right");
        provider.item(&root).await.unwrap();
        assert_eq!(fake.server.passwords.load(Ordering::SeqCst), 3);
    })
    .await;
}

#[tokio::test]
async fn a_password_refused_after_its_call_was_cancelled_is_never_sent_again() {
    marlin_test_keychain::install();
    within(async {
        let fake = Fake::start("right").await;
        fake.save("wrong");
        let provider = fake.provider();
        let root = provider.root();
        let held = fake.server.answers.write().await;
        tokio::select! {
            answer = provider.item(&root) => panic!("the sign-in answered: {answer:?}"),
            () = fake.server.password_seen.notified() => {}
        }
        // The call that sent the password is gone, as when the core ends a call at its time limit. The attempt goes on and keeps the server's no.
        drop(held);
        assert_eq!(provider.item(&root).await, Err(VfsError::SignInNeeded));
        assert_eq!(provider.item(&root).await, Err(VfsError::SignInNeeded));
        assert_eq!(fake.server.passwords.load(Ordering::SeqCst), 1);
        assert_eq!(fake.server.connections.load(Ordering::SeqCst), 1);
    })
    .await;
}

#[tokio::test]
async fn a_connection_lost_during_a_call_runs_the_call_again() {
    marlin_test_keychain::install();
    within(async {
        let (fake, provider) = Fake::signed_in().await;
        provider.item(&provider.root()).await.unwrap();
        let lost = provider.item(&ItemId("/lost".into())).await.unwrap();
        assert_eq!(lost.attributes.kind, Kind::Folder);
        assert_eq!(fake.server.connections.load(Ordering::SeqCst), 2);
    })
    .await;
}

#[tokio::test]
async fn an_agent_that_cannot_sign_is_not_asked_again_at_once() {
    marlin_test_keychain::install();
    within(async {
        // No agent listens here, so it cannot sign, as when the user declines its prompt. Only this test signs in with an agent.
        std::env::set_var("SSH_AUTH_SOCK", "/nonexistent/marlin-sftp-agent");
        let fake = Fake::start("right").await;
        let (host, user) = ("127.0.0.1".to_string(), "user".to_string());
        add_server(
            &fake.servers,
            host,
            fake.port,
            user,
            None,
            "agent".into(),
            None,
        )
        .unwrap();
        let provider = fake.provider();
        let root = provider.root();
        assert_eq!(provider.item(&root).await, Err(VfsError::SignInNeeded));
        assert_eq!(provider.item(&root).await, Err(VfsError::SignInNeeded));
        assert_eq!(fake.server.connections.load(Ordering::SeqCst), 1);
    })
    .await;
}

#[tokio::test]
async fn free_space_the_server_does_not_give_is_one_pib() {
    marlin_test_keychain::install();
    within(async {
        let unknown = Space {
            total: 1 << 50,
            free: 1 << 50,
        };
        let (_fake, provider) = Fake::signed_in().await;
        assert_eq!(provider.space().await, Ok(unknown));

        // A server that offers statvfs and answers it with an error status.
        let (fake, provider) = Fake::signed_in().await;
        fake.server.offers_statvfs.store(true, Ordering::SeqCst);
        assert_eq!(provider.space().await, Ok(unknown));
        assert_eq!(fake.server.statvfs_asked.load(Ordering::SeqCst), 1);
    })
    .await;
}

#[tokio::test]
async fn a_short_answer_is_asked_again_for_the_missing_bytes() {
    marlin_test_keychain::install();
    within(async {
        let (_fake, provider) = Fake::signed_in().await;
        let short = ItemId("/short".into());
        let middle = provider.read(&short, 50_000, 40_000).await.unwrap();
        assert_eq!(middle, pattern(50_000, 40_000));
        let tail = provider.read(&short, 99_500, 1_000).await.unwrap();
        assert_eq!(tail, pattern(99_500, 500));
    })
    .await;
}

#[tokio::test]
async fn a_denied_path_is_access_denied() {
    marlin_test_keychain::install();
    within(async {
        let (_fake, provider) = Fake::signed_in().await;
        let denied = ItemId("/denied".into());
        assert_eq!(provider.item(&denied).await, Err(VfsError::AccessDenied));
        assert_eq!(provider.list(&denied).await, Err(VfsError::AccessDenied));
        assert_eq!(
            provider.read(&denied, 0, 1).await,
            Err(VfsError::AccessDenied)
        );
    })
    .await;
}

#[tokio::test]
async fn a_generic_failure_to_list_takes_its_reason_from_a_stat() {
    marlin_test_keychain::install();
    within(async {
        let (_fake, provider) = Fake::signed_in().await;
        let gone = provider.list(&ItemId("/gone".into())).await;
        assert_eq!(gone, Err(VfsError::NotFound));
        let hidden = provider.list(&ItemId("/hidden".into())).await;
        assert_eq!(hidden, Err(VfsError::AccessDenied));
    })
    .await;
}

#[tokio::test]
async fn an_open_cancelled_before_its_answer_drops_the_connection() {
    marlin_test_keychain::install();
    within(async {
        let (fake, provider) = Fake::signed_in().await;
        let hang = ItemId("/hang".into());
        let read = provider.read(&hang, 0, 10);
        tokio::select! {
            answer = read => panic!("the open answered: {answer:?}"),
            () = fake.server.hang.notified() => {}
        }
        // The read was dropped when `select!` ended. The provider closes the connection stuck on the open with no other call, so the server can free the handle.
        tokio::time::timeout(Duration::from_secs(10), fake.server.ended.notified())
            .await
            .expect("the connection stuck on the open was not closed");
        provider.item(&provider.root()).await.unwrap();
        assert_eq!(fake.server.connections.load(Ordering::SeqCst), 2);
    })
    .await;
}

#[tokio::test]
async fn an_unknown_host_key_is_not_asked_again_until_known_hosts_changes() {
    marlin_test_keychain::install();
    within(async {
        let harness = Harness::start().await;
        fs::write(&harness.known_hosts, "").unwrap();
        harness.save("ed25519");
        let provider = harness.provider(Duration::from_secs(60));
        assert_eq!(
            provider.item(&provider.root()).await,
            Err(VfsError::SignInNeeded)
        );
        assert_eq!(harness.proxy.accepted(), 1);
        for _ in 0..5 {
            assert_eq!(
                provider.item(&provider.root()).await,
                Err(VfsError::SignInNeeded)
            );
        }
        assert_eq!(harness.proxy.accepted(), 1);
        fs::write(
            &harness.known_hosts,
            harness.sshd.known_hosts_line(harness.proxy.port()),
        )
        .unwrap();
        provider.item(&provider.root()).await.unwrap();
        assert_eq!(harness.proxy.accepted(), 2);
    })
    .await;
}

#[tokio::test]
async fn a_key_file_that_does_not_load_waits_before_it_is_tried_again() {
    marlin_test_keychain::install();
    within(async {
        let harness = Harness::start().await;
        harness.save("no-such-key");
        let provider = harness.provider(Duration::from_secs(2));
        let root = provider.root();
        assert_eq!(provider.item(&root).await, Err(VfsError::SignInNeeded));
        assert_eq!(harness.proxy.accepted(), 1);
        assert_eq!(provider.item(&root).await, Err(VfsError::SignInNeeded));
        assert_eq!(harness.proxy.accepted(), 1);
        tokio::time::sleep(Duration::from_millis(2100)).await;
        assert_eq!(provider.item(&root).await, Err(VfsError::SignInNeeded));
        assert_eq!(harness.proxy.accepted(), 2);
    })
    .await;
}

#[tokio::test]
async fn a_provider_signs_in_only_as_its_accounts_user() {
    marlin_test_keychain::install();
    within(async {
        let harness = Harness::start().await;
        harness.save("ed25519");
        let provider = SftpProvider::new(Arc::new(SftpAccount::new(
            "someone-else",
            "127.0.0.1",
            harness.proxy.port(),
            harness.config(Duration::from_secs(60)),
        )));
        assert_eq!(
            provider.item(&provider.root()).await,
            Err(VfsError::SignInNeeded)
        );
        assert_eq!(harness.proxy.accepted(), 0);
    })
    .await;
}

#[tokio::test]
async fn providers_that_share_an_account_never_send_a_refused_password_again() {
    marlin_test_keychain::install();
    within(async {
        let fake = Fake::start("right").await;
        fake.save("wrong");
        let account = fake.account();
        let first = SftpProvider::new(Arc::clone(&account));
        assert_eq!(first.item(&first.root()).await, Err(VfsError::SignInNeeded));
        assert_eq!(fake.server.passwords.load(Ordering::SeqCst), 1);
        drop(first);
        let second = SftpProvider::new(account);
        assert_eq!(
            second.item(&second.root()).await,
            Err(VfsError::SignInNeeded)
        );
        assert_eq!(fake.server.passwords.load(Ordering::SeqCst), 1);
    })
    .await;
}

#[tokio::test]
async fn a_second_provider_waits_for_the_first_ones_sign_in() {
    marlin_test_keychain::install();
    within(async {
        let fake = Fake::start("right").await;
        fake.save("wrong");
        let account = fake.account();
        let first = SftpProvider::new(Arc::clone(&account));
        let root = first.root();
        let held = fake.server.answers.write().await;
        tokio::select! {
            answer = first.item(&root) => panic!("the sign-in answered: {answer:?}"),
            () = fake.server.password_seen.notified() => {}
        }
        drop(first);
        let second = SftpProvider::new(account);
        let second_root = second.root();
        let call = tokio::spawn(async move { second.item(&second_root).await });
        // Long enough for the second attempt to reach the server, had nothing held it back.
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(held);
        assert_eq!(call.await.unwrap(), Err(VfsError::SignInNeeded));
        assert_eq!(fake.server.passwords.load(Ordering::SeqCst), 1);
        assert_eq!(fake.server.connections.load(Ordering::SeqCst), 1);
    })
    .await;
}

#[tokio::test]
async fn dropping_a_provider_closes_its_connection() {
    marlin_test_keychain::install();
    within(async {
        let harness = Harness::start().await;
        harness.save("ed25519");
        // The account outlives the provider, as it outlives a mount in the helper.
        let account = harness.account(Duration::from_secs(60));
        let provider = SftpProvider::new(Arc::clone(&account));
        provider.item(&provider.root()).await.unwrap();
        assert_eq!(harness.proxy.open(), 1);
        drop(provider);
        let deadline = Instant::now() + Duration::from_secs(5);
        while harness.proxy.open() != 0 {
            assert!(
                Instant::now() < deadline,
                "the connection was still open after the provider was dropped"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        drop(account);
    })
    .await;
}
