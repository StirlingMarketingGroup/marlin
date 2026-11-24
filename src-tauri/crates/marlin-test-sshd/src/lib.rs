//! OpenSSH's `sshd` and a TCP relay for Marlin's tests only.
//!
//! The tests need `/usr/sbin/sshd` and `ssh-keygen`. The server runs as the current user on 127.0.0.1 with key sign-in only, as `app-tests/sftp.test.ts` runs it.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

const SSHD: &str = "/usr/sbin/sshd";

/// OpenSSH's `sshd`, with the client keys `ed25519` and `rsa` (named by `authorized_keys`) and `unauthorized` (not named).
pub struct Sshd {
    root: PathBuf,
    port: u16,
    user: String,
    child: Child,
    _dir: tempfile::TempDir,
}

impl Sshd {
    pub async fn start() -> Self {
        assert!(
            Path::new(SSHD).exists(),
            "Install OpenSSH's server: {SSHD} does not exist."
        );
        let dir = tempfile::Builder::new()
            .prefix("marlin-sftp-")
            .tempdir_in("/tmp")
            .unwrap();
        // On macOS `/tmp` is a link to `/private/tmp`, and the provider shows a link to a folder only in a folder whose path is its real path.
        let root = dir.path().canonicalize().unwrap();

        for (name, kind) in [
            ("host_key", "ed25519"),
            ("ed25519", "ed25519"),
            ("rsa", "rsa"),
            ("unauthorized", "ed25519"),
        ] {
            let output = Command::new("ssh-keygen")
                .args(["-q", "-t", kind, "-N", "", "-f"])
                .arg(root.join(name))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "ssh-keygen failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let mut authorized = fs::read(root.join("ed25519.pub")).unwrap();
        authorized.extend(fs::read(root.join("rsa.pub")).unwrap());
        fs::write(root.join("authorized_keys"), authorized).unwrap();

        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let root_text = root.display();
        let config = root.join("sshd_config");
        fs::write(
            &config,
            format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {root_text}/host_key\nPidFile {root_text}/sshd.pid\nAuthorizedKeysFile {root_text}/authorized_keys\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nUsePAM no\nStrictModes no\nSubsystem sftp internal-sftp\n"
            ),
        )
        .unwrap();

        let log_path = root.join("sshd.log");
        let log = fs::File::create(&log_path).unwrap();
        let child = Command::new(SSHD)
            .args(["-D", "-e", "-f"])
            .arg(&config)
            .stdin(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
        let user = Command::new("id").arg("-un").output().unwrap().stdout;
        let mut sshd = Self {
            root,
            port,
            user: String::from_utf8(user).unwrap().trim().to_string(),
            child,
            _dir: dir,
        };

        let deadline = Instant::now() + Duration::from_secs(10);
        while TcpStream::connect(("127.0.0.1", sshd.port)).await.is_err() {
            let ended = sshd.child.try_wait().unwrap().is_some();
            assert!(
                !ended && Instant::now() < deadline,
                "sshd did not accept a connection on port {}.\n{}",
                sshd.port,
                fs::read_to_string(&log_path).unwrap_or_default()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        sshd
    }

    /// The canonical folder. It also holds the keys.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    /// The private key file `name` (`ed25519`, `rsa`, `unauthorized`, or `host_key`).
    pub fn key(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// `[127.0.0.1]:<port> <host key>`, with a trailing newline.
    pub fn known_hosts_line(&self, port: u16) -> String {
        let host_key = fs::read_to_string(self.root.join("host_key.pub")).unwrap();
        format!("[127.0.0.1]:{port} {}\n", host_key.trim_end())
    }
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A TCP relay, so a test can count connections, cut them, freeze them, or refuse new ones.
pub struct Proxy {
    port: u16,
    accepted: Arc<AtomicUsize>,
    relays: Arc<Mutex<Vec<JoinHandle<()>>>>,
    listening: JoinHandle<()>,
    frozen: Arc<Notify>,
    refuse: Arc<AtomicBool>,
}

impl Proxy {
    /// Relays new connections to `to`.
    pub async fn start(to: u16) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let relays = Arc::new(Mutex::new(Vec::new()));
        let frozen = Arc::new(Notify::new());
        let refuse = Arc::new(AtomicBool::new(false));
        let listening = tokio::spawn({
            let (accepted, relays, frozen, refuse) = (
                Arc::clone(&accepted),
                Arc::clone(&relays),
                Arc::clone(&frozen),
                Arc::clone(&refuse),
            );
            async move {
                while let Ok((mut client, _)) = listener.accept().await {
                    accepted.fetch_add(1, Ordering::SeqCst);
                    if refuse.load(Ordering::SeqCst) {
                        continue;
                    }
                    let frozen = Arc::clone(&frozen);
                    relays.lock().unwrap().push(tokio::spawn(async move {
                        let mut server = TcpStream::connect(("127.0.0.1", to)).await.unwrap();
                        let freeze = tokio::select! {
                            _ = copy_bidirectional(&mut client, &mut server) => false,
                            () = frozen.notified() => true,
                        };
                        if freeze {
                            // Both sockets stay open and say nothing, as on a network that died with no word, until the relay is cut.
                            std::future::pending::<()>().await;
                        }
                    }));
                }
            }
        });
        Self {
            port,
            accepted,
            relays,
            listening,
            frozen,
            refuse,
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    /// Relays running now. A relay ends when either side closes.
    pub fn open(&self) -> usize {
        self.relays
            .lock()
            .unwrap()
            .iter()
            .filter(|relay| !relay.is_finished())
            .count()
    }

    /// Stops every open connection without closing it.
    pub fn freeze(&self) {
        self.frozen.notify_waiters();
    }

    pub fn cut(&self) {
        for relay in self.relays.lock().unwrap().drain(..) {
            relay.abort();
        }
    }

    /// While on, a new connection is accepted and closed at once, as by a server that is going away.
    pub fn refuse(&self, on: bool) {
        self.refuse.store(on, Ordering::SeqCst);
    }

    /// Stops listening, so a new connection is refused.
    pub async fn stop(&mut self) {
        self.listening.abort();
        let _ = (&mut self.listening).await;
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.listening.abort();
        self.cut();
    }
}
