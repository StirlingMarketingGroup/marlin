use marlin_smb_client::protocol::{
    error_codes, methods, ListSharesParams, ListSharesResult, ReadDirectoryParams,
    ReadDirectoryResult, ReadParams, ReadResult, SmbCredentials, SpaceParams, SpaceResult,
    READ_MAX,
};
use marlin_test_samba::{Server, DENIED_SHARE, PASSWORD, SHARE};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs::Permissions;
use std::io::{BufRead, Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};

fn credentials(server: &Server, password: &str) -> SmbCredentials {
    SmbCredentials {
        hostname: server.host(),
        username: server.user().into(),
        password: password.into(),
        domain: None,
    }
}

/// Starts `marlin-smb`, sends one request, and returns the process and its one reply. The caller reaps the process.
fn exchange(method: &str, params: impl Serialize) -> (Child, serde_json::Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_marlin-smb"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn marlin-smb");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    writeln!(stdin, "{request}").expect("write request");

    let mut line = String::new();
    std::io::BufReader::new(stdout)
        .read_line(&mut line)
        .expect("read response");
    drop(stdin);
    (child, serde_json::from_str(&line).expect("parse response"))
}

fn call(method: &str, params: impl Serialize) -> serde_json::Value {
    let (mut child, response) = exchange(method, params);
    let status = child.wait().expect("wait for marlin-smb");
    assert!(status.success(), "marlin-smb exited {status}");
    response
}

/// Like `call`, and also returns the helper's peak memory in bytes.
fn measured_call(method: &str, params: impl Serialize) -> (serde_json::Value, u64) {
    let (child, response) = exchange(method, params);
    // wait4 reaps the child and reports its peak memory, which `Child::wait` does not.
    let pid = child.id() as libc::pid_t;
    let mut status = 0;
    // SAFETY: rusage is plain integers, so all zeros is a valid value.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are to locals that outlive the call.
    let waited = unsafe { libc::wait4(pid, &mut status, 0, &mut usage) };
    assert_eq!(waited, pid, "wait4");
    assert!(
        libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
        "exit status {status}"
    );
    // ru_maxrss counts bytes on macOS and KiB on Linux.
    let peak = usage.ru_maxrss as u64;
    #[cfg(target_os = "linux")]
    let peak = peak * 1024;
    (response, peak)
}

fn result<R: DeserializeOwned>(response: &serde_json::Value) -> R {
    if let Some(error) = response.get("error") {
        panic!("error response: {error}");
    }
    serde_json::from_value(response["result"].clone()).unwrap_or_else(|error| {
        panic!("result {error} in {response}");
    })
}

fn code(response: &serde_json::Value) -> i64 {
    match response.get("error") {
        Some(error) => error["code"]
            .as_i64()
            .unwrap_or_else(|| panic!("error has no code: {response}")),
        None => panic!("success response: {response}"),
    }
}

#[test]
fn ping() {
    let response = call(methods::PING, ());
    assert_eq!(response["id"], 1);
    assert_eq!(response["result"]["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn read_returns_the_asked_range() {
    let server = Server::start();
    std::fs::write(server.path("small.txt"), "hello, marlin\n").unwrap();

    let raw_read = |offset, length| {
        call(
            methods::READ,
            ReadParams {
                credentials: credentials(&server, PASSWORD),
                share: SHARE.into(),
                path: "/small.txt".into(),
                offset,
                length,
            },
        )
    };
    let read = |offset, length| result::<ReadResult>(&raw_read(offset, length)).data;

    assert_eq!(read(0, 1000), b"hello, marlin\n");
    assert_eq!(read(7, 6), b"marlin");
    // The bytes travel as base64 text: "marlin".
    assert_eq!(raw_read(7, 6)["result"]["data"], "bWFybGlu");
    assert_eq!(read(14, 10), b"");
    assert_eq!(read(1000, 10), b"");
}

#[test]
fn read_in_the_middle_of_a_large_file() {
    let server = Server::start();
    let mut file = std::fs::File::create(server.path("large.bin")).unwrap();
    file.set_len(160 * 1024 * 1024).unwrap();
    file.seek(SeekFrom::Start(80 * 1024 * 1024)).unwrap();
    let pattern: Vec<u8> = (0..2 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    file.write_all(&pattern).unwrap();

    let (response, peak) = measured_call(
        methods::READ,
        ReadParams {
            credentials: credentials(&server, PASSWORD),
            share: SHARE.into(),
            path: "/large.bin".into(),
            offset: 80 * 1024 * 1024 + 4099,
            length: 1024 * 1024,
        },
    );
    let body: ReadResult = result(&response);
    assert_eq!(body.data, pattern[4099..4099 + 1024 * 1024]);
    assert!(
        peak < 80 * 1024 * 1024,
        "peak memory {peak} bytes: the read held the file, not the range"
    );

    let response = call(
        methods::READ,
        ReadParams {
            credentials: credentials(&server, PASSWORD),
            share: SHARE.into(),
            path: "/large.bin".into(),
            offset: 0,
            length: READ_MAX,
        },
    );
    let body: ReadResult = result(&response);
    assert_eq!(body.data, vec![0; READ_MAX as usize]);

    let response = call(
        methods::READ,
        ReadParams {
            credentials: credentials(&server, PASSWORD),
            share: SHARE.into(),
            path: "/large.bin".into(),
            offset: 0,
            length: READ_MAX + 1,
        },
    );
    assert_eq!(code(&response), i64::from(error_codes::INVALID_PARAMS));
}

#[test]
fn read_of_a_folder() {
    let server = Server::start();
    std::fs::create_dir(server.path("folder")).unwrap();
    let response = call(
        methods::READ,
        ReadParams {
            credentials: credentials(&server, PASSWORD),
            share: SHARE.into(),
            path: "/folder".into(),
            offset: 0,
            length: 10,
        },
    );
    assert_eq!(code(&response), i64::from(error_codes::SMB_IS_A_FOLDER));
}

#[test]
fn missing_path() {
    let server = Server::start();
    let response = call(
        methods::READ,
        ReadParams {
            credentials: credentials(&server, PASSWORD),
            share: SHARE.into(),
            path: "/none.txt".into(),
            offset: 0,
            length: 10,
        },
    );
    assert_eq!(code(&response), i64::from(error_codes::SMB_PATH_NOT_FOUND));

    let response = call(
        methods::SPACE,
        SpaceParams {
            credentials: credentials(&server, PASSWORD),
            share: "nosuchshare".into(),
        },
    );
    assert_eq!(code(&response), i64::from(error_codes::SMB_PATH_NOT_FOUND));
}

#[test]
fn space_reports_the_share_size() {
    let server = Server::start();
    let response = call(
        methods::SPACE,
        SpaceParams {
            credentials: credentials(&server, PASSWORD),
            share: SHARE.into(),
        },
    );
    let space: Option<SpaceResult> = result(&response);
    let space = space.expect("share size");
    assert!(space.free <= space.total);

    // The share is a local folder, so its size is the size of the disk that holds it.
    let path = std::ffi::CString::new(server.path("").as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: statvfs is plain integers, so all zeros is a valid value.
    let mut local: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are to locals that outlive the call.
    assert_eq!(unsafe { libc::statvfs(path.as_ptr(), &mut local) }, 0);
    let local_total = local.f_blocks as u64 * local.f_frsize as u64;
    assert!(
        space.total.abs_diff(local_total) <= local_total / 100,
        "the share reports {} bytes, the disk {local_total}",
        space.total
    );
}

#[test]
fn wrong_password() {
    let server = Server::start();
    std::fs::write(server.path("small.txt"), "hello, marlin\n").unwrap();

    let response = call(
        methods::READ,
        ReadParams {
            credentials: credentials(&server, "wrong-password"),
            share: SHARE.into(),
            path: "/small.txt".into(),
            offset: 0,
            length: 10,
        },
    );
    assert_eq!(
        code(&response),
        i64::from(error_codes::SMB_AUTH_FAILED),
        "{response}"
    );

    let response = call(
        methods::SPACE,
        SpaceParams {
            credentials: credentials(&server, "wrong-password"),
            share: SHARE.into(),
        },
    );
    assert_eq!(code(&response), i64::from(error_codes::SMB_AUTH_FAILED));
}

#[test]
fn share_or_file_the_user_may_not_open() {
    let server = Server::start();
    std::fs::write(server.path("small.txt"), "hello, marlin\n").unwrap();
    let response = call(
        methods::READ,
        ReadParams {
            credentials: credentials(&server, PASSWORD),
            share: DENIED_SHARE.into(),
            path: "/small.txt".into(),
            offset: 0,
            length: 10,
        },
    );
    assert_eq!(
        code(&response),
        i64::from(error_codes::SMB_PERMISSION_DENIED)
    );

    // smbd runs as the test's user, so a file that user cannot read is refused. Root would read it anyway.
    std::fs::write(server.path("locked.txt"), "secret\n").unwrap();
    std::fs::set_permissions(server.path("locked.txt"), Permissions::from_mode(0o000)).unwrap();
    let response = call(
        methods::READ,
        ReadParams {
            credentials: credentials(&server, PASSWORD),
            share: SHARE.into(),
            path: "/locked.txt".into(),
            offset: 0,
            length: 10,
        },
    );
    assert_eq!(
        code(&response),
        i64::from(error_codes::SMB_PERMISSION_DENIED),
        "{response}"
    );
}

#[test]
fn disabled_account() {
    let server = Server::start();
    std::fs::write(server.path("small.txt"), "hello, marlin\n").unwrap();
    server.disable_account();

    let response = call(
        methods::READ,
        ReadParams {
            credentials: credentials(&server, PASSWORD),
            share: SHARE.into(),
            path: "/small.txt".into(),
            offset: 0,
            length: 10,
        },
    );
    assert_eq!(
        code(&response),
        i64::from(error_codes::SMB_AUTH_FAILED),
        "{response}"
    );

    let response = call(
        methods::LIST_SHARES,
        ListSharesParams {
            credentials: credentials(&server, PASSWORD),
        },
    );
    assert_eq!(
        code(&response),
        i64::from(error_codes::SMB_AUTH_FAILED),
        "{response}"
    );
}

#[test]
fn read_directory_errors() {
    let server = Server::start();
    std::fs::write(server.path("small.txt"), "hello, marlin\n").unwrap();

    let listed = |share: &str, path: &str, password: &str| {
        call(
            methods::READ_DIRECTORY,
            ReadDirectoryParams {
                credentials: credentials(&server, password),
                share: share.into(),
                path: path.into(),
            },
        )
    };

    // A folder the server can stat but not list. The mode goes back before the asserts, so the temporary folder can be removed.
    std::fs::create_dir(server.path("locked")).unwrap();
    std::fs::set_permissions(server.path("locked"), Permissions::from_mode(0o000)).unwrap();
    let response = listed(SHARE, "/locked", PASSWORD);
    std::fs::set_permissions(server.path("locked"), Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        code(&response),
        i64::from(error_codes::SMB_PERMISSION_DENIED),
        "{response}"
    );

    let response = listed(SHARE, "/none", PASSWORD);
    assert_eq!(code(&response), i64::from(error_codes::SMB_PATH_NOT_FOUND));

    let response = listed(SHARE, "/small.txt", PASSWORD);
    assert_eq!(
        code(&response),
        i64::from(error_codes::SMB_NOT_A_FOLDER),
        "{response}"
    );

    let response = listed(DENIED_SHARE, "/", PASSWORD);
    assert_eq!(
        code(&response),
        i64::from(error_codes::SMB_PERMISSION_DENIED)
    );

    let response = listed(SHARE, "/", "wrong-password");
    assert_eq!(
        code(&response),
        i64::from(error_codes::SMB_AUTH_FAILED),
        "{response}"
    );

    let response = listed(SHARE, "/", PASSWORD);
    let body: ReadDirectoryResult = result(&response);
    assert!(body.entries.iter().any(|entry| entry.name == "small.txt"));
}

#[test]
fn server_that_cannot_be_reached() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let credentials = SmbCredentials {
        hostname: format!("127.0.0.1:{port}"),
        username: "marlin".into(),
        password: PASSWORD.into(),
        domain: None,
    };

    let response = call(
        methods::READ,
        ReadParams {
            credentials: credentials.clone(),
            share: SHARE.into(),
            path: "/small.txt".into(),
            offset: 0,
            length: 10,
        },
    );
    assert_eq!(code(&response), i64::from(error_codes::SMB_CONNECTION_LOST));

    let response = call(
        methods::READ_DIRECTORY,
        ReadDirectoryParams {
            credentials: credentials.clone(),
            share: SHARE.into(),
            path: "/".into(),
        },
    );
    assert_eq!(
        code(&response),
        i64::from(error_codes::SMB_CONNECTION_LOST),
        "{response}"
    );

    let response = call(methods::LIST_SHARES, ListSharesParams { credentials });
    assert_eq!(code(&response), i64::from(error_codes::SMB_CONNECTION_LOST));
}

#[test]
fn list_shares_signs_in_through_stdin() {
    let server = Server::start();

    let response = call(
        methods::LIST_SHARES,
        ListSharesParams {
            credentials: credentials(&server, PASSWORD),
        },
    );
    let listed: ListSharesResult = result(&response);
    let names: Vec<&str> = listed
        .shares
        .iter()
        .map(|share| share.name.as_str())
        .collect();
    assert!(names.contains(&SHARE));
    assert!(names.contains(&DENIED_SHARE));

    let response = call(
        methods::LIST_SHARES,
        ListSharesParams {
            credentials: credentials(&server, "wrong-password"),
        },
    );
    assert_eq!(
        code(&response),
        i64::from(error_codes::SMB_AUTH_FAILED),
        "{response}"
    );
}
