use marlin_smb_client::protocol::{
    error_codes, methods, CopyParams, CreateDirectoryParams, DeleteParams, DownloadFileParams,
    DownloadFileResult, DownloadPartialParams, DownloadPartialResult, FileMetadataResult,
    GetFileMetadataParams, ListSharesParams, ListSharesResult, ReadDirectoryParams,
    ReadDirectoryResult, ReadParams, ReadResult, RenameParams, SmbCredentials, SpaceParams,
    SpaceResult, UploadFileParams, UploadFileResult, READ_MAX,
};
use marlin_test_samba::{Server, DENIED_SHARE, PASSWORD, SHARE, URL_CHARACTERS_SHARE};
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

    // A path under a file is missing, not a file.
    let response = listed(SHARE, "/small.txt/more", PASSWORD);
    assert_eq!(
        code(&response),
        i64::from(error_codes::SMB_PATH_NOT_FOUND),
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
fn names_with_url_characters_reach_their_own_file() {
    let server = Server::start();
    let files = [
        ("a%20b.txt", &b"percent twenty"[..]),
        ("c%2fd.txt", &b"slash"[..]),
        ("100%.txt", &b"lone percent"[..]),
        ("x%00y.txt", &b"nul"[..]),
        ("a#b.txt", &b"hash"[..]),
        ("a b.txt", &b"space"[..]),
    ];
    for (name, bytes) in files {
        std::fs::write(server.path(name), bytes).unwrap();
    }
    // Folders by the same rules, each holding one file named after it. `x` and `d e` are what `x%00y.txt` and `d%20e` would reach if libsmbclient decoded them, as `a b.txt` is for `a%20b.txt`.
    let folders = ["d%20e", "d e", "f%2fg", "50%"];
    for folder in folders {
        std::fs::create_dir(server.path(folder)).unwrap();
        std::fs::write(server.path(folder).join(format!("in {folder}")), b"").unwrap();
    }
    std::fs::write(server.path("x"), b"decoy x").unwrap();

    for (name, bytes) in files {
        let response = call(
            methods::READ,
            ReadParams {
                credentials: credentials(&server, PASSWORD),
                share: SHARE.into(),
                path: format!("/{name}"),
                offset: 0,
                length: 100,
            },
        );
        let body: ReadResult = result(&response);
        assert_eq!(body.data, bytes, "{name}");

        let response = call(
            methods::GET_FILE_METADATA,
            GetFileMetadataParams {
                credentials: credentials(&server, PASSWORD),
                share: SHARE.into(),
                path: format!("/{name}"),
            },
        );
        let meta: FileMetadataResult = result(&response);
        assert_eq!(meta.name, name, "{name}");
        assert_eq!(meta.size, bytes.len() as u64, "{name}");
    }

    let listed = |share: &str, path: &str| -> Vec<(String, u64)> {
        let response = call(
            methods::READ_DIRECTORY,
            ReadDirectoryParams {
                credentials: credentials(&server, PASSWORD),
                share: share.into(),
                path: path.into(),
            },
        );
        result::<ReadDirectoryResult>(&response)
            .entries
            .into_iter()
            .map(|entry| (entry.name, entry.size))
            .collect()
    };

    let top = listed(SHARE, "/");
    for (name, bytes) in files {
        assert!(
            top.contains(&(name.to_string(), bytes.len() as u64)),
            "{name} in {top:?}"
        );
    }
    for folder in folders {
        assert_eq!(
            listed(SHARE, &format!("/{folder}")),
            [(format!("in {folder}"), 0)]
        );
    }
    let top = listed(URL_CHARACTERS_SHARE, "/");
    assert!(top.contains(&("a%20b.txt".to_string(), 14)), "{top:?}");

    let response = call(
        methods::SPACE,
        SpaceParams {
            credentials: credentials(&server, PASSWORD),
            share: URL_CHARACTERS_SHARE.into(),
        },
    );
    let space: Option<SpaceResult> = result(&response);
    assert!(space.is_some(), "{response}");
}

#[test]
fn write_calls_keep_names_with_url_characters() {
    let server = Server::start();
    std::fs::write(server.path("a%20b.txt"), b"percent twenty").unwrap();
    std::fs::write(server.path("a b.txt"), b"space").unwrap();
    let credentials = credentials(&server, PASSWORD);

    let response = call(
        methods::CREATE_DIRECTORY,
        CreateDirectoryParams {
            credentials: credentials.clone(),
            share: SHARE.into(),
            path: "/new%41".into(),
        },
    );
    assert!(response.get("error").is_none(), "{response}");
    assert!(server.path("new%41").is_dir());
    assert!(!server.path("newA").exists());

    let response = call(
        methods::CREATE_DIRECTORY,
        CreateDirectoryParams {
            credentials: credentials.clone(),
            share: SHARE.into(),
            path: "/what?".into(),
        },
    );
    assert!(response.get("error").is_some(), "{response}");
    assert!(!server.path("what").exists());

    let response = call(
        methods::COPY,
        CopyParams {
            credentials: credentials.clone(),
            share: SHARE.into(),
            from_path: "/a%20b.txt".into(),
            to_path: "/copy%41.txt".into(),
        },
    );
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(
        std::fs::read(server.path("copy%41.txt")).unwrap(),
        b"percent twenty"
    );
    assert!(!server.path("copyA.txt").exists());

    let response = call(
        methods::RENAME,
        RenameParams {
            credentials: credentials.clone(),
            share: SHARE.into(),
            from_path: "/a%20b.txt".into(),
            to_path: "/r%20s.txt".into(),
        },
    );
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(
        std::fs::read(server.path("r%20s.txt")).unwrap(),
        b"percent twenty"
    );
    assert!(!server.path("a%20b.txt").exists());
    assert!(!server.path("r s.txt").exists());
    assert_eq!(std::fs::read(server.path("a b.txt")).unwrap(), b"space");

    // The local paths hold `%20` too: they must reach the disk as they are.
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("upload%20source.txt");
    std::fs::write(&source, b"uploaded").unwrap();
    let response = call(
        methods::UPLOAD_FILE,
        UploadFileParams {
            credentials: credentials.clone(),
            share: SHARE.into(),
            source_path: source.display().to_string(),
            dest_path: "/up%41.txt".into(),
        },
    );
    let uploaded: UploadFileResult = result(&response);
    assert_eq!(uploaded.size, 8);
    assert_eq!(
        std::fs::read(server.path("up%41.txt")).unwrap(),
        b"uploaded"
    );
    assert!(!server.path("upA.txt").exists());

    // A file and a folder, each beside what its name would reach if libsmbclient decoded it. Each decoy is the other kind, so a `stat` of the decoy would pick the wrong call.
    std::fs::create_dir(server.path("copyA.txt")).unwrap();
    std::fs::write(server.path("newA"), b"decoy").unwrap();
    for path in ["/copy%41.txt", "/new%41"] {
        let response = call(
            methods::DELETE,
            DeleteParams {
                credentials: credentials.clone(),
                share: SHARE.into(),
                path: path.into(),
            },
        );
        assert!(response.get("error").is_none(), "{response}");
    }
    assert!(!server.path("copy%41.txt").exists());
    assert!(!server.path("new%41").exists());
    assert!(server.path("copyA.txt").is_dir());
    assert_eq!(std::fs::read(server.path("newA")).unwrap(), b"decoy");

    let downloaded = temp.path().join("download%20dest.txt");
    let response = call(
        methods::DOWNLOAD_FILE,
        DownloadFileParams {
            credentials: credentials.clone(),
            share: SHARE.into(),
            path: "/r%20s.txt".into(),
            dest_path: downloaded.display().to_string(),
        },
    );
    assert_eq!(result::<DownloadFileResult>(&response).size, 14);
    assert_eq!(std::fs::read(&downloaded).unwrap(), b"percent twenty");

    let partial_path = temp.path().join("partial%20dest.txt");
    let response = call(
        methods::DOWNLOAD_PARTIAL,
        DownloadPartialParams {
            credentials,
            share: SHARE.into(),
            path: "/r%20s.txt".into(),
            dest_path: partial_path.display().to_string(),
            max_bytes: 7,
        },
    );
    let partial: DownloadPartialResult = result(&response);
    assert_eq!(partial.bytes_written, 7);
    assert_eq!(partial.total_size, 14);
    assert_eq!(std::fs::read(&partial_path).unwrap(), b"percent");
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
