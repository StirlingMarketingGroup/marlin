//! The helper with its real SMB provider. Two tests run anywhere: one with no marlin-smb, and one with a fake marlin-smb that refuses every sign-in. The tests that mount need FUSE, Samba, and a marlin-smb built beside the helper (`cargo build -p marlin-smb`). `crates/marlin-fuse/test-in-docker.sh` runs them on Linux, and `crates/marlin-fuse/test-on-mac.sh` runs them on a Mac.

#![cfg(unix)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use marlin_mount::protocol::{error_codes, MountState, MountStatus};
use marlin_mount::{Config, LinkedProviders, OsAdapter, Server, SmbConfig};
use marlin_mount_client::{Client, ClientError, Event, Start};
use marlin_test_samba::{DENIED_SHARE, SHARE, URL_CHARACTERS_SHARE};
use marlin_vfs::Options;
use serde_json::{json, Value};

struct Running {
    root: PathBuf,
    client: Client,
    /// Only the hung-server test reads events.
    events: tokio::sync::mpsc::UnboundedReceiver<Event>,
    signal: tokio::sync::oneshot::Sender<()>,
    run: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

async fn running_with(binary: Option<PathBuf>, options: Options) -> (Running, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("mount.sock");
    let root = dir.path().join("Marlin");
    let servers = dir.path().join("smb-servers.json");
    let server = Server::bind(
        Config {
            socket: socket.clone(),
            root: root.clone(),
            spool: dir.path().join("spool"),
            options,
        },
        Arc::new(OsAdapter),
        Arc::new(LinkedProviders::new(
            marlin_gdrive::Config::default(),
            || Err("no Google client in this test".into()),
            marlin_sftp::Config {
                servers: dir.path().join("sftp-servers.json"),
                known_hosts: dir.path().join("known_hosts"),
                retry_after: Duration::from_secs(60),
            },
            SmbConfig {
                binary,
                servers: servers.clone(),
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
    (
        Running {
            root,
            client,
            events,
            signal,
            run,
            _dir: dir,
        },
        servers,
    )
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

/// The file beside the built helper, where the helper itself looks.
fn marlin_smb() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_marlin-mount")).with_file_name("marlin-smb");
    assert!(
        marlin_smb_client::is_installed(&path),
        "build it first: cargo build -p marlin-smb"
    );
    path
}

/// A FUSE mount opens a folder locally and asks the server only when the listing is read, so a failure can come from the iterator.
async fn list(path: &Path) -> std::io::Result<Vec<String>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(path)? {
            names.push(entry?.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        Ok(names)
    })
    .await
    .unwrap()
}

async fn names(path: &Path) -> Vec<String> {
    list(path).await.unwrap()
}

/// The helper runs in this test process, so its `marlin-smb` processes are children of it.
fn marlin_smb_children() -> Vec<String> {
    let output = Command::new("pgrep")
        .arg("-P")
        .arg(std::process::id().to_string())
        .args(["-x", "marlin-smb"])
        .output()
        .unwrap();
    if output.status.code() == Some(1) {
        return Vec::new();
    }
    assert!(output.status.success(), "pgrep: {}", output.status);
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Waits up to 5 seconds for every `marlin-smb` process to end.
async fn no_marlin_smb_left() {
    let started = std::time::Instant::now();
    while !marlin_smb_children().is_empty() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a marlin-smb process outlived its mount"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A dead child stays visible to `kill -0` until its parent waits. `ps` shows `Z` once it has exited. Waits up to 5 seconds.
fn wait_until_exited(pid: &str) {
    for _ in 0..500 {
        let output = Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() || stat.trim().is_empty() || stat.contains('Z') {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("process {pid} did not exit");
}

/// A `sh` child writes the script, so this test process never holds it open for writing.
/// On Linux a process that another test is starting at that moment would inherit the
/// handle, and running the script would then fail with "Text file busy".
fn write_script(path: &Path, body: &str) {
    let mut writer = Command::new("/bin/sh")
        .args(["-c", r#"cat > "$1" && chmod 755 "$1""#, "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    write!(writer.stdin.take().unwrap(), "#!/bin/sh\n{body}").unwrap();
    assert!(
        writer.wait().unwrap().success(),
        "could not write {}",
        path.display()
    );
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

#[tokio::test]
async fn a_server_with_no_marlin_smb_is_refused_and_leaves_no_folder() {
    marlin_test_keychain::install();
    let missing = tempfile::tempdir().unwrap();
    let binary = missing.path().join("marlin-smb");
    let (running, _servers) = running_with(Some(binary.clone()), Options::default()).await;
    let mount = json!({"provider": "smb", "account": "nas.example", "name": "smb-nas.example"});

    let (code, message) = rpc(running
        .client
        .call::<MountStatus>("mount", mount.clone())
        .await
        .unwrap_err());
    assert_eq!(code, error_codes::PROVIDER);
    assert_eq!(
        message,
        marlin_smb_client::StartError::NotInstalled.to_string()
    );
    assert!(!running.root.join("smb-nas.example").exists());

    std::fs::write(&binary, b"").unwrap();
    let (code, message) = rpc(running
        .client
        .call::<MountStatus>("mount", mount)
        .await
        .unwrap_err());
    assert_eq!(code, error_codes::PROVIDER);
    assert_eq!(
        message,
        marlin_smb_client::StartError::NotInstalled.to_string()
    );
    assert!(!running.root.join("smb-nas.example").exists());

    stop(running).await;
}

#[tokio::test]
async fn a_refused_sign_in_stays_refused_for_the_server_in_any_letter_case() {
    marlin_test_keychain::install();
    let scripts = tempfile::tempdir().unwrap();
    let binary = scripts.path().join("marlin-smb");
    let log = scripts.path().join("requests.log");
    let quoted = format!("'{}'", log.display().to_string().replace('\'', "'\\''"));
    let code = marlin_smb_client::protocol::error_codes::SMB_AUTH_FAILED;
    write_script(
        &binary,
        &format!(
            r#"while IFS= read -r line; do
  printf '%s\n' "$line" >> {quoted}
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  printf '{{"jsonrpc":"2.0","id":%s,"error":{{"code":{code},"message":"refused"}}}}\n' "$id"
done
"#
        ),
    );
    let (running, servers) = running_with(Some(binary), Options::default()).await;
    // The password each request to the fake carried, in order.
    let sent = || -> Vec<String> {
        std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|line| {
                let request: Value = serde_json::from_str(line).unwrap();
                request["params"]["credentials"]["password"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    };
    marlin_smb_client::add_server(
        &servers,
        "nas.example".into(),
        "ana".into(),
        "first".into(),
        None,
    )
    .unwrap();

    let (code, message) = rpc(running
        .client
        .call::<MountStatus>(
            "mount",
            json!({"provider": "smb", "account": "nas.example", "name": "smb-nas"}),
        )
        .await
        .unwrap_err());
    assert_eq!(code, error_codes::SIGN_IN_NEEDED);
    assert_eq!(message, "the account needs a new sign-in");
    assert_eq!(sent(), ["first"]);
    assert!(!running.root.join("smb-nas").exists());

    let (code, message) = rpc(running
        .client
        .call::<MountStatus>(
            "mount",
            json!({"provider": "smb", "account": "NAS.EXAMPLE", "name": "smb-upper"}),
        )
        .await
        .unwrap_err());
    assert_eq!(code, error_codes::SIGN_IN_NEEDED);
    assert_eq!(message, "the account needs a new sign-in");
    assert_eq!(sent(), ["first"]);
    assert!(!running.root.join("smb-upper").exists());

    marlin_test_keychain::set("marlin-smb", "nas.example", "second");
    let (code, message) = rpc(running
        .client
        .call::<MountStatus>(
            "mount",
            json!({"provider": "smb", "account": "nas.example", "name": "smb-nas"}),
        )
        .await
        .unwrap_err());
    assert_eq!(code, error_codes::SIGN_IN_NEEDED);
    assert_eq!(message, "the account needs a new sign-in");
    assert_eq!(sent(), ["first", "second"]);
    assert!(!running.root.join("smb-nas").exists());

    stop(running).await;
}

/// libsmbclient 4.23 and later can replace a file on a server that is not on port 445. `smbclient --version` prints the library's version, such as `Version 4.17.12-Debian`.
fn libsmbclient_replaces_off_port_445() -> bool {
    let output = Command::new("smbclient").arg("--version").output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(version) = text
        .split(|character: char| !character.is_ascii_digit() && character != '.')
        .find(|part| part.contains('.'))
    else {
        return false;
    };
    let mut numbers = version
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0));
    (numbers.next().unwrap_or(0), numbers.next().unwrap_or(0)) >= (4, 23)
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_saved_server_mounts_and_takes_writes_through_fuse() {
    marlin_test_keychain::install();
    let server = marlin_test_samba::Server::start();
    std::fs::write(server.path("notes.txt"), b"hello").unwrap();
    std::fs::create_dir(server.path("sub")).unwrap();
    let size = 20 * 1024 * 1024;
    let bytes: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    std::fs::write(server.path("big.bin"), &bytes).unwrap();
    let shared = ["big.bin", "notes.txt", "sub"].map(String::from);

    let (running, servers) = running_with(
        Some(marlin_smb()),
        Options {
            max_listings: 0,
            attributes_ttl: Duration::ZERO,
            space_ttl: Duration::ZERO,
            ..Options::default()
        },
    )
    .await;
    marlin_smb_client::add_server(
        &servers,
        server.host(),
        server.user().into(),
        "wrong-password".into(),
        None,
    )
    .unwrap();
    let name = marlin_mount_client::mount_name::build("smb", &server.host(), &[]);

    let (code, _message) = rpc(running
        .client
        .call::<MountStatus>(
            "mount",
            json!({"provider": "smb", "account": server.host(), "name": name}),
        )
        .await
        .unwrap_err());
    assert_eq!(code, error_codes::SIGN_IN_NEEDED);
    assert!(!running.root.join(&name).exists());
    // The refused mount's provider is gone, and its processes with it.
    no_marlin_smb_left().await;

    marlin_test_keychain::set("marlin-smb", &server.host(), marlin_test_samba::PASSWORD);
    let status = running
        .client
        .call::<MountStatus>(
            "mount",
            json!({"provider": "smb", "account": server.host(), "name": name}),
        )
        .await
        .unwrap();
    assert_eq!(status.state, MountState::Mounted);
    assert_eq!(status.read_only, None);
    let point = PathBuf::from(&status.mount_point);

    let mut shares = [SHARE, DENIED_SHARE, URL_CHARACTERS_SHARE].map(String::from);
    shares.sort();
    assert_eq!(names(&point).await, shares);
    let data = point.join(SHARE);
    assert_eq!(names(&data).await, shared);
    // The same folder, shared under a name that an `smb://` URL would change.
    assert_eq!(names(&point.join(URL_CHARACTERS_SHARE)).await, shared);

    // A program other than this test reads through the mount.
    let printed = tokio::task::spawn_blocking({
        let notes = data.join("notes.txt");
        move || Command::new("cat").arg(notes).output().unwrap()
    })
    .await
    .unwrap();
    assert!(printed.status.success());
    assert_eq!(printed.stdout, b"hello");

    let big = data.join("big.bin");
    let offset = 16 * 1024 * 1024 + 3;
    let got = tokio::task::spawn_blocking({
        let big = big.clone();
        move || {
            use std::io::{Read, Seek, SeekFrom};
            let mut file = std::fs::File::open(big).unwrap();
            file.seek(SeekFrom::Start(offset as u64)).unwrap();
            let mut got = vec![0; 1000];
            file.read_exact(&mut got).unwrap();
            got
        }
    })
    .await
    .unwrap();
    assert_eq!(
        got.iter()
            .zip(&bytes[offset..offset + 1000])
            .position(|(got, want)| got != want),
        None,
        "the first wrong byte"
    );
    let read = tokio::task::spawn_blocking({
        let big = big.clone();
        move || std::fs::read(big).unwrap()
    })
    .await
    .unwrap();
    assert_eq!(read.len(), bytes.len());
    assert_eq!(
        read.iter().zip(&bytes).position(|(got, want)| got != want),
        None,
        "the first wrong byte"
    );

    let denied = list(&point.join(DENIED_SHARE)).await.unwrap_err();
    assert_eq!(denied.raw_os_error(), Some(libc::EACCES));

    // libsmbclient before 4.23 reads a file's access only on port 445. This server is not, so a replace is refused and this test does not overwrite.
    let replaces = libsmbclient_replaces_off_port_445();
    tokio::task::spawn_blocking({
        let data = data.clone();
        move || {
            std::fs::write(data.join("new.txt"), b"made here").unwrap();
            std::fs::create_dir(data.join("box")).unwrap();
            std::fs::rename(data.join("notes.txt"), data.join("box").join("notes.txt")).unwrap();
            if replaces {
                std::fs::write(data.join("big.bin"), b"replaced").unwrap();
            }
        }
    })
    .await
    .unwrap();
    // Uploads start 2 seconds after the last close. A rename and a new folder reach the server at once.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let ready = tokio::task::spawn_blocking({
            let server = server.path("");
            move || {
                std::fs::read(server.join("new.txt")).ok().as_deref() == Some(b"made here")
                    && server.join("box").is_dir()
                    && std::fs::read(server.join("box").join("notes.txt"))
                        .ok()
                        .as_deref()
                        == Some(b"hello")
                    && !server.join("notes.txt").exists()
                    && (!replaces
                        || std::fs::read(server.join("big.bin")).ok().as_deref()
                            == Some(b"replaced"))
            }
        })
        .await
        .unwrap();
        if ready {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the server did not show the writes"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::task::spawn_blocking({
        let path = data.join("new.txt");
        move || std::fs::remove_file(path).unwrap()
    })
    .await
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while server.path("new.txt").exists() {
        assert!(
            Instant::now() < deadline,
            "the remove did not reach the server"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let pids = marlin_smb_children();
    assert!(!pids.is_empty());
    for pid in &pids {
        let id: i32 = pid.parse().unwrap();
        // SAFETY: kill(2) takes plain integers and touches no memory of ours.
        unsafe {
            libc::kill(id, libc::SIGKILL);
        }
        wait_until_exited(pid);
    }
    let after = ["big.bin", "box", "sub"].map(String::from);
    // macOS writes a `._name` file beside each new file, and those stay on this computer.
    let user_names: Vec<String> = names(&data)
        .await
        .into_iter()
        .filter(|name| !marlin_fuse::stays_local(name))
        .collect();
    assert_eq!(user_names, after);
    // A new process answered: a killed one can still show as a zombie until its parent reaps it.
    assert!(marlin_smb_children().iter().any(|pid| !pids.contains(pid)));

    // The kill ended the call processes. One finished upload starts an upload process, and the unmount has to end that too.
    tokio::task::spawn_blocking({
        let path = data.join("after.txt");
        move || std::fs::write(path, b"after").unwrap()
    })
    .await
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if std::fs::read(server.path("after.txt")).ok().as_deref() == Some(b"after") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the upload after the killed processes did not finish"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    running
        .client
        .call::<Value>(
            "unmount",
            json!({"provider": "smb", "account": server.host()}),
        )
        .await
        .unwrap();
    assert!(!point.exists());
    no_marlin_smb_left().await;

    stop(running).await;
}

#[ignore = "mounts for real: run crates/marlin-fuse/test-in-docker.sh on Linux, crates/marlin-fuse/test-on-mac.sh on a Mac"]
#[tokio::test(flavor = "multi_thread")]
async fn a_hung_server_times_out_and_leaves_the_other_mount_alone() {
    marlin_test_keychain::install();
    let quiet = marlin_test_samba::Server::start();
    let hung = marlin_test_samba::Server::start();
    std::fs::write(quiet.path("notes.txt"), b"quiet").unwrap();
    std::fs::write(hung.path("notes.txt"), b"hung").unwrap();

    let (mut running, servers) = running_with(
        Some(marlin_smb()),
        // `space_ttl` keeps its default: macOS asks a writable mount for its free space many times a second, and with no cache each ask opens a new connection to the paused server, until it refuses new ones and the calls fail fast instead of timing out.
        Options {
            max_listings: 0,
            attributes_ttl: Duration::ZERO,
            call_timeout: Duration::from_secs(10),
            ..Options::default()
        },
    )
    .await;
    marlin_smb_client::add_server(
        &servers,
        quiet.host(),
        quiet.user().into(),
        marlin_test_samba::PASSWORD.into(),
        None,
    )
    .unwrap();
    marlin_smb_client::add_server(
        &servers,
        hung.host(),
        hung.user().into(),
        marlin_test_samba::PASSWORD.into(),
        None,
    )
    .unwrap();
    let quiet_name = marlin_mount_client::mount_name::build("smb", &quiet.host(), &[]);
    let hung_name = marlin_mount_client::mount_name::build(
        "smb",
        &hung.host(),
        std::slice::from_ref(&quiet_name),
    );

    let quiet_status = running
        .client
        .call::<MountStatus>(
            "mount",
            json!({"provider": "smb", "account": quiet.host(), "name": quiet_name}),
        )
        .await
        .unwrap();
    let hung_status = running
        .client
        .call::<MountStatus>(
            "mount",
            json!({"provider": "smb", "account": hung.host(), "name": hung_name}),
        )
        .await
        .unwrap();
    assert_eq!(quiet_status.state, MountState::Mounted);
    assert_eq!(hung_status.state, MountState::Mounted);
    let quiet_point = PathBuf::from(&quiet_status.mount_point);
    let hung_point = PathBuf::from(&hung_status.mount_point);

    hung.pause();
    let hung_data = hung_point.join(SHARE);
    let task = tokio::spawn(async move {
        let started = std::time::Instant::now();
        // Twice the limit: well short of the 30-second default, so a mount that ignored its own limit fails here.
        let result = tokio::time::timeout(Duration::from_secs(20), list(&hung_data))
            .await
            .expect("the call outlived the mount's 10-second limit");
        (started.elapsed(), result)
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        names(&quiet_point.join(SHARE)).await,
        ["notes.txt".to_string()]
    );
    assert!(!task.is_finished());
    let (elapsed, result) = task.await.unwrap();
    assert!(elapsed >= Duration::from_secs(10));
    assert_eq!(result.unwrap_err().raw_os_error(), Some(libc::ETIMEDOUT));

    // A mount turns `degraded` after three failed calls in a row, and the listing above was one. Two names, because the kernel sends parallel lookups of one name to the mount as one request.
    let (data, denied) = (hung_point.join(SHARE), hung_point.join(DENIED_SHARE));
    let (one, two) = tokio::join!(list(&data), list(&denied));
    assert_eq!(one.unwrap_err().raw_os_error(), Some(libc::ETIMEDOUT));
    assert_eq!(two.unwrap_err().raw_os_error(), Some(libc::ETIMEDOUT));
    let degraded = changed(&mut running.events, MountState::Degraded).await;
    assert_eq!(degraded.account, hung.host());

    hung.resume();
    assert_eq!(
        names(&hung_point.join(SHARE)).await,
        ["notes.txt".to_string()]
    );
    let mounted = changed(&mut running.events, MountState::Mounted).await;
    assert_eq!(mounted.account, hung.host());

    running
        .client
        .call::<Value>(
            "unmount",
            json!({"provider": "smb", "account": quiet.host()}),
        )
        .await
        .unwrap();
    running
        .client
        .call::<Value>(
            "unmount",
            json!({"provider": "smb", "account": hung.host()}),
        )
        .await
        .unwrap();
    stop(running).await;
}
