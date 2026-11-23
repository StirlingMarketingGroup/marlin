//! A Samba server for Marlin's tests only.
//!
//! Samba must be installed: `brew install samba` on a Mac, and the `samba` and `smbclient` packages on Debian. Linux also needs `pkill`, from `procps`.
//! Each [`Server`] is its own `smbd`, run as the current user on a free local port.

// Samba's smbd and Marlin's SMB code run on macOS and Linux only. Empty elsewhere, so a workspace check on Windows still builds.
#![cfg(unix)]

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const PASSWORD: &str = "marlin-test-password";
/// The share the user may read.
pub const SHARE: &str = "data";
/// A share the user signs in to but may not open.
pub const DENIED_SHARE: &str = "denied";

const INSTALL: &str = "Install Samba (`brew install samba` on a Mac, `samba smbclient` on Debian).";

pub struct Server {
    dir: tempfile::TempDir,
    port: u16,
    user: String,
    pdbedit: PathBuf,
    smbd: std::process::Child,
}

impl Server {
    pub fn start() -> Self {
        let smbd_program = tool(
            "MARLIN_TEST_SMBD",
            if cfg!(target_os = "macos") {
                &[
                    "/opt/homebrew/sbin/samba-dot-org-smbd",
                    "/usr/local/sbin/samba-dot-org-smbd",
                ]
            } else {
                &["/usr/sbin/smbd"]
            },
            "Samba's smbd",
        );
        let pdbedit = tool(
            "MARLIN_TEST_PDBEDIT",
            &[
                "/opt/homebrew/bin/pdbedit",
                "/usr/local/bin/pdbedit",
                "/usr/bin/pdbedit",
            ],
            "pdbedit",
        );

        // smbd's socket paths must stay under 104 bytes, and the default temp folder on a Mac is too long.
        let dir = tempfile::Builder::new()
            .prefix("msmb")
            .tempdir_in("/tmp")
            .expect("temp dir for smbd");
        for name in [
            "lock", "state", "cache", "private", "pid", "ncalrpc", "data",
        ] {
            std::fs::create_dir(dir.path().join(name)).expect("smbd directory");
        }

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
        let port = listener.local_addr().expect("bound port").port();
        drop(listener);

        let user = Command::new("id").arg("-un").output().expect("id -un");
        assert!(user.status.success(), "id -un failed");
        let user = String::from_utf8(user.stdout)
            .expect("user name")
            .trim()
            .to_string();

        let conf = dir.path().join("smb.conf");
        let root = dir.path().display();
        std::fs::write(
            &conf,
            format!(
                "\
[global]
  smb ports = {port}
  bind interfaces only = yes
  interfaces = 127.0.0.1
  lock directory = {root}/lock
  state directory = {root}/state
  cache directory = {root}/cache
  private dir = {root}/private
  pid directory = {root}/pid
  ncalrpc dir = {root}/ncalrpc
  passdb backend = tdbsam:{root}/private/passdb.tdb
  disable spoolss = yes
  load printers = no
  printcap name = /dev/null
  server role = standalone server
  map to guest = never
[{SHARE}]
  path = {root}/data
  read only = yes
[{DENIED_SHARE}]
  path = {root}/data
  valid users = marlin-nobody
"
            ),
        )
        .expect("smb.conf");

        // `-t` reads the new password twice from stdin.
        let mut added = Command::new(&pdbedit)
            .arg(format!("--configfile={}", conf.display()))
            .args(["-a", "-u", &user, "-t"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn pdbedit");
        added
            .stdin
            .take()
            .expect("pdbedit stdin")
            .write_all(format!("{PASSWORD}\n{PASSWORD}\n").as_bytes())
            .expect("pdbedit password");
        let added = added.wait_with_output().expect("pdbedit");
        assert!(
            added.status.success(),
            "pdbedit exited {}\n{}{}",
            added.status,
            String::from_utf8_lossy(&added.stdout),
            String::from_utf8_lossy(&added.stderr)
        );

        let log_path = dir.path().join("smbd.log");
        let log = std::fs::File::create(&log_path).expect("smbd log");
        let smbd = Command::new(&smbd_program)
            .arg(format!("--configfile={}", conf.display()))
            .args(["-F", "--debug-stdout", "-d", "1", "--no-process-group"])
            .stdin(Stdio::null())
            .stdout(log.try_clone().expect("smbd log"))
            .stderr(log)
            .process_group(0)
            .spawn()
            .unwrap_or_else(|error| panic!("failed to start {}: {error}", smbd_program.display()));

        // Built before the wait, so a failed start still stops smbd.
        let mut server = Server {
            dir,
            port,
            user,
            pdbedit,
            smbd,
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            let exited = server.smbd.try_wait().expect("smbd status");
            assert!(
                exited.is_none() && Instant::now() < deadline,
                "smbd did not listen on port {port} (exit: {exited:?})\n{}",
                std::fs::read_to_string(&log_path).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        server
    }

    /// `127.0.0.1:<port>`: the host as a saved server holds it.
    pub fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    /// A path in the shared folder, for a test to make files and folders in.
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join("data").join(name)
    }

    /// Disables the user's account, as an administrator would. The next sign-in fails with `NT_STATUS_ACCOUNT_DISABLED`.
    pub fn disable_account(&self) {
        let status = Command::new(&self.pdbedit)
            .arg(format!(
                "--configfile={}",
                self.dir.path().join("smb.conf").display()
            ))
            .args(["-r", "-u", &self.user, "-c", "[D]"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("pdbedit");
        assert!(status.success(), "pdbedit could not disable the account");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // smbd's children share its process group, so a SIGTERM to the group leaves none behind.
        if let Ok(pid) = i32::try_from(self.smbd.id()) {
            // SAFETY: kill(2) takes plain integers and touches no memory of ours.
            unsafe {
                libc::kill(-pid, libc::SIGTERM);
            }
        }
        let _ = self.smbd.wait();
        // smbd starts samba-dcerpcd to list shares. It leads a process group of its own, which outlives smbd and keeps restarting its workers. Every one of them names this server's config file.
        let _ = Command::new("pkill")
            .args(["-TERM", "-f"])
            .arg(self.dir.path().join("smb.conf"))
            .status();
    }
}

/// The program in `env` when it is set, else the first of `candidates` that exists.
fn tool(env: &str, candidates: &[&str], name: &str) -> PathBuf {
    if let Some(path) = std::env::var_os(env) {
        let path = PathBuf::from(path);
        assert!(
            path.is_file(),
            "{name} was not found at {}. {INSTALL}",
            path.display()
        );
        return path;
    }
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .unwrap_or_else(|| {
            panic!(
                "{name} was not found. Looked for {}. {INSTALL}",
                candidates.join(", ")
            )
        })
}
