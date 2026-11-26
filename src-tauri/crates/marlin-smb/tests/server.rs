use marlin_smb_client::protocol::{
    error_codes, methods, CopyParams, CreateDirectoryParams, DeleteParams, DownloadFileParams,
    DownloadFileResult, DownloadPartialParams, DownloadPartialResult, FileMetadataResult,
    FileStamp, GetFileMetadataParams, ListSharesParams, ListSharesResult, Notice, NoticeLine,
    ReadDirectoryParams, ReadDirectoryResult, ReadParams, ReadResult, RenameParams, SaveFileParams,
    SmbCredentials, SpaceParams, SpaceResult, UploadFileParams, UploadFileResult, READ_MAX,
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

/// Starts `marlin-smb`, sends one request (id 1), and reads until the answer. `act` runs for each notice. The caller reaps the process. After the answer, `exchange` closes stdin, so the helper leaves its request loop and exits.
fn exchange(
    method: &str,
    params: impl Serialize,
    mut act: impl FnMut(&Notice),
) -> (Child, Vec<Notice>, serde_json::Value) {
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

    let mut heard = Vec::new();
    let mut reader = std::io::BufReader::new(stdout);
    let response = loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).expect("read response");
        assert!(read > 0, "marlin-smb closed stdout");
        let value: serde_json::Value = serde_json::from_str(&line).expect("parse response");
        if value.get("id").is_some() {
            break value;
        }
        let notice = serde_json::from_value::<NoticeLine>(value)
            .expect("a notice")
            .notice;
        assert_eq!(notice.id(), 1, "notice for another request: {notice:?}");
        act(&notice);
        heard.push(notice);
    };
    drop(stdin);
    (child, heard, response)
}

fn call(method: &str, params: impl Serialize) -> serde_json::Value {
    call_reporting(method, params).0
}

fn call_reporting(method: &str, params: impl Serialize) -> (serde_json::Value, Vec<Notice>) {
    let (mut child, heard, response) = exchange(method, params, |_| {});
    let status = child.wait().expect("wait for marlin-smb");
    assert!(status.success(), "marlin-smb exited {status}");
    (response, heard)
}

/// Like `call`, and also returns the helper's peak memory in bytes.
fn measured_call(method: &str, params: impl Serialize) -> (serde_json::Value, u64) {
    let (child, _heard, response) = exchange(method, params, |_| {});
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
            replace: false,
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
fn list_shares_signs_in_through_a_pipe() {
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

    // smbclient would strip the space and send another password, so it is refused before it reaches the server.
    let response = call(
        methods::LIST_SHARES,
        ListSharesParams {
            credentials: credentials(&server, " starts-with-a-space"),
        },
    );
    assert_eq!(code(&response), i64::from(error_codes::SMB_AUTH_FAILED));
    assert!(
        message(&response).contains("starts with a space"),
        "{response}"
    );
}

fn message(response: &serde_json::Value) -> String {
    response["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("error has no message: {response}"))
        .to_string()
}

fn renaming(server: &Server, from: &str, to: &str, replace: bool) -> RenameParams {
    RenameParams {
        credentials: credentials(server, PASSWORD),
        share: SHARE.into(),
        from_path: from.into(),
        to_path: to.into(),
        replace,
    }
}

/// The request the app's `smb://` rename sends: it has no `replace` field.
#[test]
fn the_apps_rename_never_overwrites_a_taken_name() {
    let server = Server::start();
    std::fs::write(server.path("a.txt"), b"from").unwrap();
    std::fs::write(server.path("b.txt"), b"to").unwrap();

    let response = call(
        methods::RENAME,
        serde_json::json!({
            "credentials": credentials(&server, PASSWORD),
            "share": SHARE,
            "from_path": "/a.txt",
            "to_path": "/b.txt",
        }),
    );
    assert_eq!(code(&response), i64::from(error_codes::SMB_ALREADY_EXISTS));
    assert_eq!(message(&response), "Destination path already exists");
    assert_eq!(std::fs::read(server.path("a.txt")).unwrap(), b"from");
    assert_eq!(std::fs::read(server.path("b.txt")).unwrap(), b"to");
}

#[test]
fn rename_moves_and_replaces_only_when_asked() {
    let server = Server::start();
    std::fs::write(server.path("a.txt"), b"from").unwrap();
    std::fs::write(server.path("b.txt"), b"to").unwrap();
    let response = call(methods::RENAME, renaming(&server, "/a.txt", "/b.txt", true));
    assert!(response.get("error").is_none(), "{response}");
    assert!(!server.path("a.txt").exists());
    assert_eq!(std::fs::read(server.path("b.txt")).unwrap(), b"from");

    std::fs::create_dir(server.path("src-dir")).unwrap();
    std::fs::write(server.path("src-dir/inside.txt"), b"inside").unwrap();
    std::fs::create_dir(server.path("dst-dir")).unwrap();
    std::fs::write(server.path("dst-dir/keep.txt"), b"keep").unwrap();
    let response = call(
        methods::RENAME,
        renaming(&server, "/src-dir", "/dst-dir", true),
    );
    assert_eq!(
        code(&response),
        i64::from(error_codes::SMB_NOT_EMPTY),
        "{response}"
    );
    assert_eq!(
        std::fs::read(server.path("src-dir/inside.txt")).unwrap(),
        b"inside"
    );
    assert_eq!(
        std::fs::read(server.path("dst-dir/keep.txt")).unwrap(),
        b"keep"
    );

    let response = call(
        methods::RENAME,
        renaming(&server, "/missing.txt", "/b.txt", false),
    );
    assert_eq!(
        code(&response),
        i64::from(error_codes::SMB_PATH_NOT_FOUND),
        "{response}"
    );
    assert_eq!(std::fs::read(server.path("b.txt")).unwrap(), b"from");

    std::fs::create_dir(server.path("dest")).unwrap();
    std::fs::write(server.path("move-me.txt"), b"move").unwrap();
    let response = call(
        methods::RENAME,
        renaming(&server, "/move-me.txt", "/dest/move-me.txt", false),
    );
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(
        std::fs::read(server.path("dest/move-me.txt")).unwrap(),
        b"move"
    );
    assert!(!server.path("move-me.txt").exists());

    std::fs::write(server.path("other.txt"), b"other").unwrap();
    std::fs::write(server.path("dest/there.txt"), b"there").unwrap();
    let response = call(
        methods::RENAME,
        renaming(&server, "/other.txt", "/dest/there.txt", true),
    );
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(
        std::fs::read(server.path("dest/there.txt")).unwrap(),
        b"other"
    );
    assert!(!server.path("other.txt").exists());
}

#[test]
fn rename_reads_the_status_not_the_names() {
    let server = Server::start();
    for name in [
        "/NT_STATUS_CONNECTION_RESET.txt",
        "/session setup failed.txt",
    ] {
        let response = call(methods::RENAME, renaming(&server, name, "/b.txt", false));
        assert_eq!(
            code(&response),
            i64::from(error_codes::SMB_PATH_NOT_FOUND),
            "{response}"
        );
    }
}

#[test]
fn rename_keeps_names_with_semicolons_spaces_and_percent() {
    let server = Server::start();
    std::fs::write(server.path("semi;colon a.txt"), b"semi").unwrap();
    let response = call(
        methods::RENAME,
        renaming(&server, "/semi;colon a.txt", "/new;name %41.txt", false),
    );
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(
        std::fs::read(server.path("new;name %41.txt")).unwrap(),
        b"semi"
    );
    assert!(!server.path("semi;colon a.txt").exists());
    assert!(!server.path("new;name A.txt").exists());
    assert!(!server.path("new").exists());

    std::fs::write(server.path("keep.txt"), b"keep").unwrap();
    let response = call(
        methods::RENAME,
        renaming(&server, "/keep.txt", "/quo\"te.txt", false),
    );
    assert_eq!(
        code(&response),
        i64::from(error_codes::INVALID_PARAMS),
        "{response}"
    );
    assert_eq!(std::fs::read(server.path("keep.txt")).unwrap(), b"keep");
    assert!(!server.path("quo\"te.txt").exists());
}

#[test]
fn rename_in_a_deep_folder() {
    let server = Server::start();
    let nested = format!(
        "{}/{}/{}",
        "a".repeat(200),
        "b".repeat(200),
        "c".repeat(200)
    );
    std::fs::create_dir_all(server.path(&nested)).unwrap();
    let old_name = "f".repeat(200);
    let new_name = "h".repeat(200);
    std::fs::write(server.path(&format!("{nested}/{old_name}")), b"deep").unwrap();
    assert!(format!("rename \"{old_name}\" \"{new_name}\"").len() < 1000);

    let from = format!("/{nested}/{old_name}");
    let to = format!("/{nested}/{new_name}");
    let response = call(methods::RENAME, renaming(&server, &from, &to, false));
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(
        std::fs::read(server.path(&format!("{nested}/{new_name}"))).unwrap(),
        b"deep"
    );
    assert!(!server.path(&format!("{nested}/{old_name}")).exists());

    let top = format!("/{}", "g".repeat(200));
    let from = format!("/{nested}/{new_name}");
    let command = format!("rename \"{from}\" \"{top}\"");
    assert!(
        command.len() >= 1000,
        "the full paths are {} bytes",
        command.len()
    );
    let response = call(methods::RENAME, renaming(&server, &from, &top, false));
    assert_eq!(
        code(&response),
        i64::from(error_codes::INVALID_PARAMS),
        "{response}"
    );
    assert_eq!(
        std::fs::read(server.path(&format!("{nested}/{new_name}"))).unwrap(),
        b"deep"
    );
    assert!(!server.path(&"g".repeat(200)).exists());
}

/// A rename that changes only the letter case names the same file on a server that ignores case, and is not a taken name.
#[test]
fn rename_changes_only_the_case() {
    let server = Server::start();
    std::fs::write(server.path("report.txt"), b"report").unwrap();

    let response = call(
        methods::RENAME,
        renaming(&server, "/report.txt", "/Report.txt", false),
    );
    assert!(response.get("error").is_none(), "{response}");
    let names: Vec<String> = std::fs::read_dir(server.path(""))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["Report.txt"]);
    assert_eq!(std::fs::read(server.path("Report.txt")).unwrap(), b"report");
}

const TEMPORARY: &str = "/.marlin-test-1";
const MODIFIED: &str = "2001-02-03T04:05:06.789Z";
const MODIFIED_SECONDS: i64 = 981173106;

fn saving(
    server: &Server,
    source: &std::path::Path,
    path: &str,
    replaces: Option<FileStamp>,
) -> SaveFileParams {
    SaveFileParams {
        credentials: credentials(server, PASSWORD),
        share: SHARE.into(),
        source_path: source.display().to_string(),
        path: path.into(),
        temporary_path: TEMPORARY.into(),
        modified: MODIFIED.into(),
        replaces,
    }
}

/// A local file holding `bytes`. It goes when the folder does.
fn local_file(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("source");
    std::fs::write(&path, bytes).unwrap();
    (folder, path)
}

fn pattern(size: usize) -> Vec<u8> {
    (0..size).map(|index| (index % 251) as u8).collect()
}

fn file_seconds(path: &std::path::Path) -> i64 {
    std::fs::metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// The stamp `save_file` takes for an answer's file: its size and whole modified second.
fn stamp_of(meta: &FileMetadataResult) -> FileStamp {
    FileStamp {
        size: meta.size,
        modified: chrono::DateTime::parse_from_rfc3339(&meta.modified)
            .unwrap_or_else(|error| panic!("{error}: {}", meta.modified))
            .timestamp(),
    }
}

/// The stamp of the file at `path`, as a caller gets it from `get_file_metadata`.
fn stamp_at(server: &Server, path: &str) -> FileStamp {
    let listed = call(
        methods::GET_FILE_METADATA,
        GetFileMetadataParams {
            credentials: credentials(server, PASSWORD),
            share: SHARE.into(),
            path: path.into(),
        },
    );
    stamp_of(&result(&listed))
}

fn progress_ends_at(heard: &[Notice], size: u64) {
    assert!(
        matches!(heard.first(), Some(Notice::Staged { .. })),
        "{heard:?}"
    );
    let mut last = 0;
    for notice in &heard[1..] {
        let Notice::Progress { bytes, .. } = notice else {
            panic!("notice after staged: {heard:?}");
        };
        assert!(*bytes > last, "{heard:?}");
        last = *bytes;
    }
    assert_eq!(last, size, "{heard:?}");
}

/// Whether the helper reads a file's access from the test server, which is not on port 445. libsmbclient before 4.23 reads access only on port 445, so with it the helper refuses a replace before anything is made.
fn server_reads_access() -> bool {
    // SAFETY: libsmbclient returns a static string, such as `4.17.12-Debian`.
    let version = unsafe { std::ffi::CStr::from_ptr(pavao_sys::smbc_version()) }.to_string_lossy();
    let mut numbers = version
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0));
    (numbers.next().unwrap_or(0), numbers.next().unwrap_or(0)) >= (4, 23)
}

fn assert_no_marlin_names(server: &Server) {
    for entry in std::fs::read_dir(server.path("")).unwrap() {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        assert!(!name.starts_with(".marlin-"), "{name}");
    }
}

#[test]
fn save_file_makes_a_new_file() {
    let server = Server::start();
    let bytes = pattern(5 * 1024 * 1024 / 2);
    let (_folder, source) = local_file(&bytes);

    // A name with `;` and `%41` reaches its own file.
    let (response, heard) = call_reporting(
        methods::SAVE_FILE,
        saving(&server, &source, "/a;b %41.bin", None),
    );
    let saved: FileMetadataResult = result(&response);
    assert_eq!(saved.size, bytes.len() as u64);
    assert_eq!(stamp_of(&saved).modified, MODIFIED_SECONDS);
    assert_eq!(std::fs::read(server.path("a;b %41.bin")).unwrap(), bytes);
    assert_eq!(file_seconds(&server.path("a;b %41.bin")), MODIFIED_SECONDS);
    assert!(!server.path("a;b A.bin").exists());
    progress_ends_at(&heard, bytes.len() as u64);
    assert_no_marlin_names(&server);
}

/// A taken name, or a taken temporary name, refuses the save before anything is made, with no notice: the caller must not remove a name it did not make.
#[test]
fn save_file_refuses_a_taken_name() {
    let server = Server::start();
    std::fs::write(server.path("taken.txt"), b"old").unwrap();
    std::fs::write(server.path(".marlin-test-1"), b"temp").unwrap();
    let (_folder, source) = local_file(b"new");

    let params = SaveFileParams {
        temporary_path: "/.marlin-test-2".into(),
        ..saving(&server, &source, "/taken.txt", None)
    };
    let (response, heard) = call_reporting(methods::SAVE_FILE, params);
    assert_eq!(code(&response), i64::from(error_codes::SMB_ALREADY_EXISTS));
    assert_eq!(message(&response), "Destination path already exists");
    assert!(heard.is_empty(), "{heard:?}");

    let (response, heard) = call_reporting(
        methods::SAVE_FILE,
        saving(&server, &source, "/free.txt", None),
    );
    assert_eq!(code(&response), i64::from(error_codes::SMB_ALREADY_EXISTS));
    assert!(heard.is_empty(), "{heard:?}");

    assert_eq!(std::fs::read(server.path("taken.txt")).unwrap(), b"old");
    assert_eq!(
        std::fs::read(server.path(".marlin-test-1")).unwrap(),
        b"temp"
    );
    assert!(!server.path(".marlin-test-2").exists());
    assert!(!server.path("free.txt").exists());
}

#[test]
fn save_file_replaces_a_file_in_one_step() {
    let server = Server::start();
    let (_first_folder, first) = local_file(b"first");
    let made = call(
        methods::SAVE_FILE,
        saving(&server, &first, "/replaced.bin", None),
    );
    let stamp = stamp_of(&result(&made));

    let bytes = pattern(3 * 1024 * 1024);
    let (_folder, source) = local_file(&bytes);
    let params = SaveFileParams {
        modified: "2010-01-02T03:04:05Z".into(),
        ..saving(&server, &source, "/replaced.bin", Some(stamp))
    };
    let (response, heard) = call_reporting(methods::SAVE_FILE, params);
    if !server_reads_access() {
        assert_eq!(
            code(&response),
            i64::from(error_codes::SMB_PERMISSION_DENIED),
            "{response}"
        );
        assert!(heard.is_empty(), "{heard:?}");
        assert_eq!(
            std::fs::read(server.path("replaced.bin")).unwrap(),
            b"first"
        );
        assert_no_marlin_names(&server);
        return;
    }
    let saved: FileMetadataResult = result(&response);
    assert_eq!(saved.size, bytes.len() as u64);
    assert_eq!(stamp_of(&saved).modified, 1262401445);
    assert_eq!(std::fs::read(server.path("replaced.bin")).unwrap(), bytes);
    assert_eq!(file_seconds(&server.path("replaced.bin")), 1262401445);
    progress_ends_at(&heard, bytes.len() as u64);
    assert_no_marlin_names(&server);
}

#[test]
fn save_file_keeps_the_old_files_access() {
    let server = Server::start();
    let (_folder, source) = local_file(b"new-bytes");

    // A new file over SMB gets mode 0744, so these differ from it.
    for mode in [0o600, 0o664] {
        let name = format!("mode-{mode:o}.txt");
        std::fs::write(server.path(&name), b"old").unwrap();
        std::fs::set_permissions(server.path(&name), Permissions::from_mode(mode)).unwrap();
        let stamp = stamp_at(&server, &format!("/{name}"));
        let (response, heard) = call_reporting(
            methods::SAVE_FILE,
            saving(&server, &source, &format!("/{name}"), Some(stamp)),
        );
        if server_reads_access() {
            assert!(response.get("error").is_none(), "{response}");
            assert_eq!(std::fs::read(server.path(&name)).unwrap(), b"new-bytes");
            let new_mode = std::fs::metadata(server.path(&name))
                .unwrap()
                .permissions()
                .mode();
            // The group bits of a file with a POSIX ACL show the ACL's mask, not the group's entry, so they are left out.
            assert_eq!(new_mode & 0o707, mode & 0o707, "{name}");
        } else {
            assert_eq!(
                code(&response),
                i64::from(error_codes::SMB_PERMISSION_DENIED),
                "{response}"
            );
            assert_eq!(std::fs::read(server.path(&name)).unwrap(), b"old");
            assert!(heard.is_empty(), "{heard:?}");
        }
        assert_no_marlin_names(&server);
    }
}

#[test]
fn save_file_refuses_a_file_changed_during_the_upload() {
    let server = Server::start();
    let (_first_folder, first) = local_file(b"original");
    let made = call(
        methods::SAVE_FILE,
        saving(&server, &first, "/target.txt", None),
    );
    let stamp = stamp_of(&result(&made));
    // A sparse file, so this process never holds 64 MiB. On Linux a child's peak memory starts from this process's peak, and `read_in_the_middle_of_a_large_file` measures a child's.
    let (_folder, source) = local_file(b"");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&source)
        .unwrap()
        .set_len(64 * 1024 * 1024)
        .unwrap();

    let target = server.path("target.txt");
    let mut appended = false;
    let (mut child, _, response) = exchange(
        methods::SAVE_FILE,
        saving(&server, &source, "/target.txt", Some(stamp)),
        |notice| {
            if matches!(notice, Notice::Progress { bytes, .. } if *bytes > 0) && !appended {
                appended = true;
                let mut out = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&target)
                    .unwrap();
                out.write_all(b"x").unwrap();
            }
        },
    );
    let status = child.wait().expect("wait for marlin-smb");
    assert!(status.success(), "marlin-smb exited {status}");
    if !server_reads_access() {
        assert!(!appended, "the replace sent bytes");
        assert_eq!(
            code(&response),
            i64::from(error_codes::SMB_PERMISSION_DENIED),
            "{response}"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
        return;
    }
    assert!(appended, "no progress arrived");
    assert_eq!(
        code(&response),
        i64::from(error_codes::SMB_CHANGED),
        "{response}"
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"originalx");
    assert_no_marlin_names(&server);
}

/// smbclient would strip the space and send another password at the publish, so the save refuses before it sends anything.
#[test]
fn save_file_refuses_a_sign_in_smbclient_cannot_take() {
    let server = Server::start();
    let (_folder, source) = local_file(b"new");
    let params = SaveFileParams {
        credentials: credentials(&server, " starts-with-a-space"),
        ..saving(&server, &source, "/new.txt", None)
    };
    let (response, heard) = call_reporting(methods::SAVE_FILE, params);
    assert_eq!(code(&response), i64::from(error_codes::SMB_AUTH_FAILED));
    assert!(
        message(&response).contains("starts with a space"),
        "{response}"
    );
    assert!(heard.is_empty(), "{heard:?}");
    assert!(!server.path("new.txt").exists());
    assert_no_marlin_names(&server);
}

/// Each of these refuses a replace before any byte is sent, makes no temporary file, and leaves the file as it was.
#[test]
fn save_file_refuses_a_replace_it_cannot_make() {
    let server = Server::start();
    std::fs::write(server.path("stay.txt"), b"abcd").unwrap();
    // smbd runs as the test's user, so a file that user cannot write is refused. Root would write it anyway.
    std::fs::write(server.path("locked.txt"), b"locked").unwrap();
    std::fs::set_permissions(server.path("locked.txt"), Permissions::from_mode(0o444)).unwrap();
    std::fs::create_dir(server.path("folder")).unwrap();
    let (_folder, source) = local_file(b"new");
    let stale = FileStamp {
        size: 3,
        ..stamp_at(&server, "/stay.txt")
    };
    let missing_source = std::path::Path::new("/no/such/marlin-source");

    for (path, source, stamp, expected) in [
        (
            "/stay.txt",
            source.as_path(),
            stale.clone(),
            error_codes::SMB_CHANGED,
        ),
        (
            "/missing.txt",
            source.as_path(),
            stale.clone(),
            error_codes::SMB_PATH_NOT_FOUND,
        ),
        (
            "/folder",
            source.as_path(),
            stale.clone(),
            error_codes::SMB_IS_A_FOLDER,
        ),
        (
            "/locked.txt",
            source.as_path(),
            stamp_at(&server, "/locked.txt"),
            error_codes::SMB_PERMISSION_DENIED,
        ),
        (
            "/stay.txt",
            missing_source,
            stamp_at(&server, "/stay.txt"),
            error_codes::INTERNAL_ERROR,
        ),
    ] {
        let (response, heard) = call_reporting(
            methods::SAVE_FILE,
            saving(&server, source, path, Some(stamp)),
        );
        assert_eq!(code(&response), i64::from(expected), "{path}: {response}");
        assert!(heard.is_empty(), "{path}: {heard:?}");
        assert_no_marlin_names(&server);
    }
    assert_eq!(std::fs::read(server.path("stay.txt")).unwrap(), b"abcd");
    assert_eq!(std::fs::read(server.path("locked.txt")).unwrap(), b"locked");
    assert!(!server.path("missing.txt").exists());
    assert!(server.path("folder").is_dir());
}
