//! SMB operations. Most go through pavao/libsmbclient; `list_shares` and `rename` run the `smbclient` program.
//!
//! All operations receive credentials as parameters (no keychain access).

use marlin_smb_client::protocol::{
    error_codes, CopyParams, CreateDirectoryParams, DeleteParams, DirectoryEntry,
    DownloadFileParams, DownloadFileResult, DownloadPartialParams, DownloadPartialResult,
    FileMetadataResult, GetFileMetadataParams, ListSharesParams, ListSharesResult,
    ReadDirectoryParams, ReadDirectoryResult, ReadParams, ReadResult, RenameParams, ShareEntry,
    SmbCredentials, SpaceParams, SpaceResult, UploadFileParams, UploadFileResult, READ_MAX,
    SMBCLIENT_MISSING,
};
use pavao::{
    SmbClient, SmbCredentials as PavaoCredentials, SmbDirentInfo, SmbFile, SmbMode, SmbOpenOptions,
    SmbOptions, SmbResult, SmbStat, SmbStatVfs,
};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::{Mutex, PoisonError};

/// Sidecar version string.
pub const SIDECAR_VERSION: &str = env!("CARGO_PKG_VERSION");

/// One connection to a share, for one call. pavao connects at the first call, not when it opens. The helper runs one request at a time, and pavao holds one lock around every libsmbclient call, whose global state is not thread-safe.
///
/// Its calls take each name as the server holds it, and escape it for libsmbclient's URL ([`escape_for_url`]). Every libsmbclient call goes through one of them.
struct Share<'a> {
    client: SmbClient,
    credentials: &'a SmbCredentials,
}

impl<'a> Share<'a> {
    fn open(credentials: &'a SmbCredentials, share: &str) -> Result<Self, (i32, String)> {
        wait_for_local_network(&credentials.hostname)?;
        // pavao adds the `/` before the share when it is missing.
        let mut server = PavaoCredentials::default()
            .server(format!("smb://{}", credentials.hostname))
            .share(escape_for_url(share))
            .username(&credentials.username)
            .password(&credentials.password);
        if let Some(domain) = &credentials.domain {
            server = server.workgroup(domain);
        }

        let client = SmbClient::new(server, SmbOptions::default()).map_err(|e| {
            (
                error_code(&e),
                format!("Failed to connect to SMB server: {e}"),
            )
        })?;
        Ok(Self {
            client,
            credentials,
        })
    }

    /// The code and text for a failed call. `action` opens the text.
    fn failure(&self, action: &str, error: impl Into<pavao::SmbError>) -> (i32, String) {
        let error = error.into();
        let code = match error_code(&error) {
            // libsmbclient answers EACCES when the server refuses the sign-in, when it refuses one share or file, and, on a Mac, when a LAN host is turned off (the socket's EHOSTDOWN). `smbclient -L` signs in without opening a share and names the first two apart from the third, so only its refused sign-in or failed connection changes the code.
            error_codes::SMB_PERMISSION_DENIED => match list_shares(ListSharesParams {
                credentials: self.credentials.clone(),
            }) {
                Err((
                    code @ (error_codes::SMB_AUTH_FAILED | error_codes::SMB_CONNECTION_LOST),
                    _,
                )) => code,
                _ => error_codes::SMB_PERMISSION_DENIED,
            },
            code => code,
        };
        (code, format!("{action}: {error}"))
    }

    fn list_dirplus(&self, path: &str) -> SmbResult<Vec<SmbDirentInfo>> {
        self.client.list_dirplus(escape_for_url(path))
    }

    fn stat(&self, path: &str) -> SmbResult<SmbStat> {
        self.client.stat(escape_for_url(path))
    }

    fn statvfs(&self, path: &str) -> SmbResult<SmbStatVfs> {
        self.client.statvfs(escape_for_url(path))
    }

    fn mkdir(&self, path: &str, mode: SmbMode) -> SmbResult<()> {
        self.client.mkdir(escape_for_url(path), mode)
    }

    fn rmdir(&self, path: &str) -> SmbResult<()> {
        self.client.rmdir(escape_for_url(path))
    }

    fn unlink(&self, path: &str) -> SmbResult<()> {
        self.client.unlink(escape_for_url(path))
    }

    fn open_with(&self, path: &str, options: SmbOpenOptions) -> SmbResult<SmbFile<'_>> {
        self.client.open_with(escape_for_url(path), options)
    }
}

/// pavao hands libsmbclient each share and path inside an `smb://` URL. libsmbclient ends the path at the last `?`, drops `"` from a share name, and decodes every `%XX`, so `a%20b.txt` would reach `a b.txt`. Each of those characters goes in escaped, and the server gets the name as given.
fn escape_for_url(name: &str) -> String {
    name.replace('%', "%25")
        .replace('?', "%3F")
        .replace('"', "%22")
}

/// The code for a failed libsmbclient call, from its errno.
fn error_code(error: &pavao::SmbError) -> i32 {
    use std::io::ErrorKind;
    let pavao::SmbError::Io(error) = error else {
        return error_codes::SMB_ERROR;
    };
    match error.kind() {
        ErrorKind::NotFound => error_codes::SMB_PATH_NOT_FOUND,
        ErrorKind::IsADirectory => error_codes::SMB_IS_A_FOLDER,
        ErrorKind::NotADirectory => error_codes::SMB_NOT_A_FOLDER,
        ErrorKind::PermissionDenied => error_codes::SMB_PERMISSION_DENIED,
        ErrorKind::ConnectionRefused
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::NotConnected
        | ErrorKind::BrokenPipe
        | ErrorKind::TimedOut
        | ErrorKind::HostUnreachable
        | ErrorKind::NetworkUnreachable
        | ErrorKind::NetworkDown => error_codes::SMB_CONNECTION_LOST,
        _ => error_codes::SMB_ERROR,
    }
}

/// Check if a file/directory name should be considered hidden.
fn is_hidden_file(name: &str) -> bool {
    name.starts_with('.')
}

/// Read directory contents.
pub fn read_directory(params: ReadDirectoryParams) -> Result<ReadDirectoryResult, (i32, String)> {
    let share = Share::open(&params.credentials, &params.share)?;

    // pavao drops opendir's errno (it answers BadFileDescriptor), so a stat of the same path finds out why the listing failed. A folder that is there and still cannot be listed was refused: the stat proves the sign-in works. A path that is there and is not a folder answers SMB_NOT_A_FOLDER.
    let entries = share
        .list_dirplus(&params.path)
        .map_err(|_| match share.stat(&params.path) {
            Err(cause) => share.failure("Failed to list directory", cause),
            Ok(stat) if stat.mode.is_dir() => (
                error_codes::SMB_PERMISSION_DENIED,
                "Failed to list directory: permission denied".to_string(),
            ),
            Ok(_) => (
                error_codes::SMB_NOT_A_FOLDER,
                "Failed to list directory: not a folder".to_string(),
            ),
        })?;

    let mut result_entries = Vec::new();

    for entry in entries {
        let name = entry.name();

        // Skip . and ..
        if name == "." || name == ".." {
            continue;
        }

        let entry_type = entry.get_type();
        let is_directory = matches!(entry_type, pavao::SmbDirentType::Dir);

        // Convert SystemTime to ISO 8601 string
        let modified: chrono::DateTime<chrono::Utc> = entry.mtime.into();

        result_entries.push(DirectoryEntry {
            name: name.to_string(),
            is_directory,
            is_hidden: is_hidden_file(name),
            size: entry.size,
            modified: modified.to_rfc3339(),
            extension: if is_directory {
                None
            } else {
                std::path::Path::new(name)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase())
            },
        });
    }

    Ok(ReadDirectoryResult {
        entries: result_entries,
    })
}

/// Get metadata for a single file or directory.
pub fn get_file_metadata(
    params: GetFileMetadataParams,
) -> Result<FileMetadataResult, (i32, String)> {
    let share = Share::open(&params.credentials, &params.share)?;

    let stat = share
        .stat(&params.path)
        .map_err(|e| share.failure("Failed to get file metadata", e))?;

    let name = std::path::Path::new(&params.path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&params.path)
        .to_string();

    let is_directory = stat.mode.is_dir();
    let modified: chrono::DateTime<chrono::Utc> = stat.modified.into();

    Ok(FileMetadataResult {
        name: name.clone(),
        is_directory,
        is_hidden: is_hidden_file(&name),
        size: stat.size,
        modified: modified.to_rfc3339(),
        extension: if is_directory {
            None
        } else {
            std::path::Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase())
        },
    })
}

/// Create a directory.
pub fn create_directory(params: CreateDirectoryParams) -> Result<(), (i32, String)> {
    let share = Share::open(&params.credentials, &params.share)?;

    share
        .mkdir(&params.path, SmbMode::from(0o755))
        .map_err(|e| share.failure("Failed to create directory", e))
}

/// Delete a file or directory.
pub fn delete(params: DeleteParams) -> Result<(), (i32, String)> {
    let share = Share::open(&params.credentials, &params.share)?;

    let stat = share
        .stat(&params.path)
        .map_err(|e| share.failure("Failed to stat path", e))?;

    if stat.mode.is_dir() {
        share
            .rmdir(&params.path)
            .map_err(|e| share.failure("Failed to delete directory", e))
    } else {
        share
            .unlink(&params.path)
            .map_err(|e| share.failure("Failed to delete file", e))
    }
}

/// Rename a file or directory.
///
/// libsmbclient's rename deletes a file that already has the new name, then renames, so a taken name is destroyed and a replace leaves the name empty between the two steps. `smbclient`'s `rename` refuses a taken name, and `rename -f` replaces it in one step.
///
/// The command goes on stdin: `-c` splits on `;` even inside quotes, so a name with `;` would run the rest as another command. smbclient reads each stdin line into a 1024-byte buffer and runs what does not fit as another command, so a line of 1000 bytes or more is refused. A rename inside one folder starts smbclient in that folder (`-D`, which ends smbclient when it cannot go there) and names only the two names, so a deep folder still fits. Only a rename across folders, which names both full paths, can reach the limit.
///
/// Each error text is a reason on its own: the app shows it after its own "Failed to rename:".
pub fn rename(params: RenameParams) -> Result<(), (i32, String)> {
    // Inside a command, a name cannot hold `"`, and a control character could end the line. An SMB name holds neither.
    if [&params.from_path, &params.to_path]
        .iter()
        .any(|path| path.chars().any(|c| c == '"' || c < ' '))
    {
        return Err((
            error_codes::INVALID_PARAMS,
            "A name cannot hold \" or a control character".to_string(),
        ));
    }
    let (from_folder, from_name) = params
        .from_path
        .rsplit_once('/')
        .unwrap_or(("", &params.from_path));
    let (to_folder, to_name) = params
        .to_path
        .rsplit_once('/')
        .unwrap_or(("", &params.to_path));
    let (host, _) = split_port(&params.credentials.hostname);
    let mut args = vec![format!("//{host}/{}", params.share), "-q".to_string()];
    if from_folder != to_folder {
        return run_rename(
            &params.credentials,
            &args,
            &params.from_path,
            &params.to_path,
            params.replace,
        );
    }
    let folder = if from_folder.is_empty() {
        "/"
    } else {
        from_folder
    };
    args.extend(["-D".to_string(), folder.to_string()]);

    // A server whose disk ignores case finds the item itself under a name that changes only the case, and refuses the rename: Samba answers that the name is taken, or with `-f` that access is denied. So when the folder holds nothing else under the new name, the item goes through a temporary name, as the app's local rename does.
    if from_name != to_name && from_name.to_lowercase() == to_name.to_lowercase() {
        let share = Share::open(&params.credentials, &params.share)?;
        let listed = share
            .list_dirplus(folder)
            .map_err(|e| share.failure("Failed to list the folder", e))?;
        let holders = listed
            .iter()
            .filter(|entry| entry.name().to_lowercase() == to_name.to_lowercase())
            .count();
        if holders == 1 {
            let aside = format!(".marlin-case-{}", std::process::id());
            run_rename(&params.credentials, &args, from_name, &aside, false)?;
            return run_rename(&params.credentials, &args, &aside, to_name, false).map_err(
                |(code, reason)| {
                    // Back to the old name, so the item never stays under the temporary one. When that fails too, the reason says where the item is, because a dot name is hidden.
                    match run_rename(&params.credentials, &args, &aside, from_name, false) {
                        Ok(()) => (code, reason),
                        Err(_) => (code, format!("{reason}. The item is now named {aside}")),
                    }
                },
            );
        }
    }
    run_rename(
        &params.credentials,
        &args,
        from_name,
        to_name,
        params.replace,
    )
}

/// One smbclient `rename` from `from` to `to`, relative to the folder `args` starts in.
fn run_rename(
    credentials: &SmbCredentials,
    args: &[String],
    from: &str,
    to: &str,
    replace: bool,
) -> Result<(), (i32, String)> {
    let command = format!(
        "rename \"{from}\" \"{to}\"{}\n",
        if replace { " -f" } else { "" }
    );
    if command.len() >= 1000 {
        return Err((
            error_codes::INVALID_PARAMS,
            "The path is too long for smbclient".to_string(),
        ));
    }

    let output = run_smbclient(credentials, args, &command)?;
    let reported = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    command_result(&reported, output.status.success()).map_err(|(code, status)| {
        let reason = match (code, status) {
            (_, Some("NT_STATUS_SHARING_VIOLATION")) => "The item is in use",
            (error_codes::SMB_ALREADY_EXISTS, _) => "Destination path already exists",
            (error_codes::SMB_PATH_NOT_FOUND, _) => "The item or its folder is not there",
            (error_codes::SMB_NOT_EMPTY, _) => "The folder to replace is not empty",
            (error_codes::SMB_PERMISSION_DENIED, _) => "Permission denied",
            (error_codes::SMB_CONNECTION_LOST, _) => "Failed to connect to SMB server",
            (error_codes::SMB_AUTH_FAILED, _) => "The server refused the sign-in",
            _ if reported.trim().is_empty() => {
                return (code, format!("smbclient {}", output.status))
            }
            _ => reported.trim(),
        };
        (code, reason.to_string())
    })
}

/// Copy a file.
pub fn copy(params: CopyParams) -> Result<(), (i32, String)> {
    let share = Share::open(&params.credentials, &params.share)?;

    let mut src = share
        .open_with(&params.from_path, SmbOpenOptions::default().read(true))
        .map_err(|e| share.failure("Failed to open source", e))?;

    let mut dst = share
        .open_with(
            &params.to_path,
            SmbOpenOptions::default()
                .write(true)
                .create(true)
                .truncate(true),
        )
        .map_err(|e| share.failure("Failed to open destination", e))?;

    std::io::copy(&mut src, &mut dst).map_err(|e| share.failure("Failed to copy", e))?;

    Ok(())
}

/// Up to `length` bytes of a file from `offset`. Fewer only at the end of the file, and none at or past it.
pub fn read(params: ReadParams) -> Result<ReadResult, (i32, String)> {
    if params.length > READ_MAX {
        return Err((
            error_codes::INVALID_PARAMS,
            format!(
                "A read takes at most {READ_MAX} bytes, not {}",
                params.length
            ),
        ));
    }

    let share = Share::open(&params.credentials, &params.share)?;

    let mut file = share
        .open_with(&params.path, SmbOpenOptions::default().read(true))
        .map_err(|e| share.failure("Failed to open SMB file", e))?;
    file.seek(SeekFrom::Start(params.offset))
        .map_err(|e| share.failure("Failed to seek SMB file", e))?;
    // The buffer never grows past `length`. The reply holds the range a few times over (bytes, base64, the JSON line), never the whole file.
    let mut data = Vec::with_capacity(params.length as usize);
    file.take(u64::from(params.length))
        .read_to_end(&mut data)
        .map_err(|e| share.failure("Failed to read SMB file", e))?;
    Ok(ReadResult { data })
}

/// A share's total and free bytes, or `None` when the server reports no size.
pub fn space(params: SpaceParams) -> Result<Option<SpaceResult>, (i32, String)> {
    let share = Share::open(&params.credentials, &params.share)?;

    let stat = share
        .statvfs("/")
        .map_err(|e| share.failure("Failed to read share size", e))?;
    Ok(space_from(&stat))
}

/// libsmbclient counts `blocks` and `bavail` in allocation units of `frsize` sectors of `bsize` bytes (Samba 4.23 answers 512 and 2). An `frsize` of 0 counts as 1. A server that reports no size leaves the fields at 0.
fn space_from(stat: &pavao::SmbStatVfs) -> Option<SpaceResult> {
    let unit = stat.bsize.saturating_mul(stat.frsize.max(1));
    let total = stat.blocks.saturating_mul(unit);
    (total > 0).then(|| SpaceResult {
        total,
        free: stat.bavail.saturating_mul(unit),
    })
}

/// libsmbclient and smbclient fail at once while macOS waits for the user's Local Network answer. So a call to a server waits for that answer here, before any SMB call, and no call runs twice. A mount makes many calls, so a server this process has reached is not tried again.
fn wait_for_local_network(hostname: &str) -> Result<(), (i32, String)> {
    static REACHED: Mutex<Vec<String>> = Mutex::new(Vec::new());
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    let mut reached = REACHED.lock().unwrap_or_else(PoisonError::into_inner);
    if reached.iter().any(|server| server == hostname) {
        return Ok(());
    }
    let (host, port) = split_port(hostname);
    let port = port.and_then(|port| port.parse().ok()).unwrap_or(445);
    // A probe slower than `AT_ONCE` cannot be the block, so it gets no more time.
    match marlin_local_network::connect(host, port, marlin_local_network::AT_ONCE) {
        Ok(_) => {
            reached.push(hostname.to_string());
            Ok(())
        }
        Err(error) if error.waited => Err((
            error_codes::SMB_CONNECTION_LOST,
            format!("Failed to connect to SMB server: {error}"),
        )),
        // libsmbclient or smbclient reports any other failure. The server stays unreached, so a later call waits if the user moved onto its network.
        Err(_) => Ok(()),
    }
}

/// The other methods put the port in the URL, which libsmbclient reads. `smbclient` takes it only as `-p`. An IPv6 address stays whole: it has colons of its own, and neither tool takes one with a port (#642).
fn split_port(hostname: &str) -> (&str, Option<&str>) {
    match hostname.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') && port.parse::<u16>().is_ok() => {
            (host, Some(port))
        }
        _ => (hostname, None),
    }
}

/// Statuses for a connection that never came up or broke.
const UNREACHABLE: &[&str] = &[
    "NT_STATUS_CONNECTION_REFUSED",
    "NT_STATUS_IO_TIMEOUT",
    "NT_STATUS_HOST_UNREACHABLE",
    "NT_STATUS_NETWORK_UNREACHABLE",
    "NT_STATUS_CONNECTION_RESET",
    "NT_STATUS_CONNECTION_DISCONNECTED",
];

/// The code for `smbclient` output that names no path: a failed `-L`, or a connect line of a run that was given commands.
fn smbclient_code(reported: &str) -> i32 {
    // A `do_connect` line means smbclient never reached the server, whatever its status. On a Mac, a LAN host that is turned off answers NT_STATUS_ACCESS_DENIED there: the socket's EHOSTDOWN.
    if reported.contains("do_connect:")
        || UNREACHABLE.iter().any(|status| reported.contains(status))
    {
        error_codes::SMB_CONNECTION_LOST
    } else if reported.contains("session setup failed") {
        // A wrong password, or a locked, disabled, or expired account.
        error_codes::SMB_AUTH_FAILED
    } else if reported.contains("NT_STATUS_ACCESS_DENIED") {
        // Signed in, but the server refuses the listing or the share.
        error_codes::SMB_PERMISSION_DENIED
    } else {
        error_codes::SMB_ERROR
    }
}

/// The code for one NT status token.
fn status_code(status: &str) -> i32 {
    match status {
        "NT_STATUS_OBJECT_NAME_COLLISION" => error_codes::SMB_ALREADY_EXISTS,
        "NT_STATUS_OBJECT_NAME_NOT_FOUND"
        | "NT_STATUS_OBJECT_PATH_NOT_FOUND"
        | "NT_STATUS_NO_SUCH_FILE" => error_codes::SMB_PATH_NOT_FOUND,
        "NT_STATUS_DIRECTORY_NOT_EMPTY" => error_codes::SMB_NOT_EMPTY,
        "NT_STATUS_FILE_IS_A_DIRECTORY" => error_codes::SMB_IS_A_FOLDER,
        "NT_STATUS_NOT_A_DIRECTORY" => error_codes::SMB_NOT_A_FOLDER,
        "NT_STATUS_ACCESS_DENIED" => error_codes::SMB_PERMISSION_DENIED,
        _ if UNREACHABLE.contains(&status) => error_codes::SMB_CONNECTION_LOST,
        _ => error_codes::SMB_ERROR,
    }
}

/// How one smbclient run that was given commands failed: its code, and the status a command's own line names. Only a failed command's line names a path, so its status is read by its place in that line: a name may hold `NT_STATUS_CONNECTION_RESET` or `session setup failed`. The connect lines name no path and read as a failed listing's do. Any other line that names a status is a generic error, and only exit 0 with no status anywhere is success.
fn command_result(output: &str, exited_ok: bool) -> Result<(), (i32, Option<&str>)> {
    for line in output.lines() {
        // `NT_STATUS_X renaming files \a -> \b`
        if let Some((status, rest)) = line.split_once(' ') {
            if status.starts_with("NT_STATUS_") && rest.starts_with("renaming files ") {
                return Err((status_code(status), Some(status)));
            }
        }
        // `rename \a: NT_STATUS_X`, and `cd \folder: NT_STATUS_X` when `-D` could not enter the folder: smbclient then runs no command.
        if let Some(rest) = line
            .strip_prefix("rename ")
            .or_else(|| line.strip_prefix("cd "))
        {
            let status = rest
                .rsplit_once(": ")
                .map_or("", |(_, status)| status.trim());
            return Err((status_code(status), Some(status)));
        }
        if [
            "do_connect:",
            "session setup failed",
            "tree connect failed",
            "protocol negotiation failed",
        ]
        .iter()
        .any(|start| line.starts_with(start))
        {
            return Err((smbclient_code(line), None));
        }
        if line.contains("NT_STATUS_") {
            return Err((error_codes::SMB_ERROR, None));
        }
    }
    if exited_ok {
        Ok(())
    } else {
        Err((error_codes::SMB_ERROR, None))
    }
}

/// Runs the `smbclient` program for this server with `args`, and `commands` on its stdin.
///
/// The sign-in goes through a pipe on descriptor 3 (`-A /dev/fd/3`), so it is never an argument, an environment variable, or a file. smbclient strips the leading spaces and tabs of each value it reads there, and a line break ends one, so such a value is refused before it could reach the server changed: each refused sign-in can count toward a lockout.
fn run_smbclient(
    credentials: &SmbCredentials,
    args: &[String],
    commands: &str,
) -> Result<std::process::Output, (i32, String)> {
    use std::os::unix::io::AsRawFd;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    // smbclient also reads a user name's `%` as the start of a password.
    let domain = credentials.domain.as_deref().unwrap_or_default();
    if [credentials.username.as_str(), &credentials.password, domain]
        .iter()
        .any(|value| value.starts_with([' ', '\t']) || value.contains(['\n', '\r']))
        || credentials.username.contains('%')
    {
        return Err((
            error_codes::SMB_AUTH_FAILED,
            "The saved sign-in cannot be passed to smbclient: a value starts with a space or a tab or holds a line break, or the user name holds %".to_string(),
        ));
    }
    let mut sign_in = format!(
        "username = {}\npassword = {}\n",
        credentials.username, credentials.password
    );
    if credentials.domain.is_some() {
        sign_in.push_str(&format!("domain = {domain}\n"));
    }

    wait_for_local_network(&credentials.hostname)?;

    let (program, path_env) = resolve_smbclient_command();
    let mut cmd = Command::new(program);
    if let Some(path_env) = path_env {
        cmd.env("PATH", path_env);
    }
    if let (_, Some(port)) = split_port(&credentials.hostname) {
        cmd.arg("-p").arg(port);
    }
    cmd.args(args)
        .args(["-A", "/dev/fd/3"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let (reader, mut writer) = std::io::pipe().map_err(|e| {
        (
            error_codes::SMB_ERROR,
            format!("Failed to run smbclient: {e}"),
        )
    })?;
    let read_fd = reader.as_raw_fd();
    // SAFETY: runs in the forked child before exec, and calls only `dup2` and `fcntl` on a descriptor open in this process. `dup2` onto 3 clears close-on-exec. `dup2` of a descriptor onto itself does not, so that case clears the flag with `fcntl`.
    unsafe {
        cmd.pre_exec(move || {
            if read_fd == 3 {
                let flags = libc::fcntl(read_fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(read_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            } else if libc::dup2(read_fd, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err((
                error_codes::SMB_LIBRARY_MISSING,
                SMBCLIENT_MISSING.to_string(),
            ));
        }
        Err(e) => {
            return Err((
                error_codes::SMB_ERROR,
                format!("Failed to run smbclient: {e}"),
            ));
        }
    };
    drop(reader);
    // A smbclient that exited before it read these fails below, with its own output.
    let _ = writer.write_all(sign_in.as_bytes());
    drop(writer);
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(commands.as_bytes());
    }

    child.wait_with_output().map_err(|e| {
        (
            error_codes::SMB_ERROR,
            format!("Failed to run smbclient: {e}"),
        )
    })
}

/// List available shares on a server.
pub fn list_shares(params: ListSharesParams) -> Result<ListSharesResult, (i32, String)> {
    // Use smbclient -L to enumerate shares (pavao doesn't support this well). `-g` prints them machine-readable.
    let (host, _) = split_port(&params.credentials.hostname);
    let output = run_smbclient(
        &params.credentials,
        &["-L".to_string(), format!("//{host}"), "-g".to_string()],
        "",
    )?;

    if !output.status.success() {
        // With `-g`, smbclient can print a failure on either stream: a refused sign-in on stdout (`session setup failed: NT_STATUS_LOGON_FAILURE`), a refused connection on stderr.
        let reported = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let text = if reported.contains("NT_STATUS_LOGON_FAILURE") {
            "Authentication failed. Try using your full email as username (e.g., user@domain.com)"
                .to_string()
        } else {
            format!("Failed to list shares: {}", reported.trim())
        };
        return Err((smbclient_code(&reported), text));
    }

    Ok(ListSharesResult {
        shares: shares_from(&String::from_utf8_lossy(&output.stdout))?,
    })
}

/// The disk shares in `smbclient -L -g` output, one `type|name|comment` line each for a share, beside `Server|` and `Workgroup|` lines from browsing. Every SMB server lists `IPC$`, so output with no share line is a listing that failed, even when `smbclient` exited 0. Samba's test server does that when its RPC service does not answer: it prints only `SMB1 disabled -- no workgroup available`.
fn shares_from(stdout: &str) -> Result<Vec<ShareEntry>, (i32, String)> {
    let mut listed_any = false;
    let mut shares = Vec::new();
    for line in stdout.lines() {
        // The comment is the rest of the line, `|` and all.
        let mut parts = line.splitn(3, '|');
        let (Some(kind), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        listed_any |= matches!(kind, "Disk" | "IPC" | "Printer");
        // Only disk shares, and none whose name ends in `$`: that leaves out `IPC$`, `ADMIN$`, `C$`, and printers.
        if kind == "Disk" && !name.ends_with('$') {
            shares.push(ShareEntry {
                name: name.to_string(),
                comment: parts.next().map(|comment| comment.to_string()),
            });
        }
    }
    if !listed_any {
        return Err((
            error_codes::SMB_ERROR,
            "Failed to list shares: the server listed none".to_string(),
        ));
    }
    Ok(shares)
}

/// Download a file to a local path.
pub fn download_file(params: DownloadFileParams) -> Result<DownloadFileResult, (i32, String)> {
    let share = Share::open(&params.credentials, &params.share)?;

    // Open the remote file for reading
    let mut smb_file = share
        .open_with(&params.path, SmbOpenOptions::default().read(true))
        .map_err(|e| share.failure("Failed to open SMB file", e))?;

    // Create parent directories if needed
    if let Some(parent) = std::path::Path::new(&params.dest_path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            (
                error_codes::INTERNAL_ERROR,
                format!("Failed to create temp directory: {}", e),
            )
        })?;
    }

    // Write to local file
    let mut local_file = std::fs::File::create(&params.dest_path).map_err(|e| {
        (
            error_codes::INTERNAL_ERROR,
            format!("Failed to create temp file: {}", e),
        )
    })?;

    let size = std::io::copy(&mut smb_file, &mut local_file).map_err(|e| {
        (
            error_codes::SMB_ERROR,
            format!("Failed to copy SMB file to temp: {}", e),
        )
    })?;

    local_file.flush().map_err(|e| {
        (
            error_codes::INTERNAL_ERROR,
            format!("Failed to flush temp file: {}", e),
        )
    })?;

    Ok(DownloadFileResult {
        path: params.dest_path,
        size,
    })
}

/// Download up to `max_bytes` of a file to a local path.
/// Returns the number of bytes written and the total file size on the server.
pub fn download_partial(
    params: DownloadPartialParams,
) -> Result<DownloadPartialResult, (i32, String)> {
    let share = Share::open(&params.credentials, &params.share)?;

    // Get total file size first
    let stat = share
        .stat(&params.path)
        .map_err(|e| share.failure("Failed to stat SMB file", e))?;
    let total_size = stat.size;

    // Open the remote file for reading
    let smb_file = share
        .open_with(&params.path, SmbOpenOptions::default().read(true))
        .map_err(|e| share.failure("Failed to open SMB file", e))?;

    // Create parent directories if needed
    if let Some(parent) = std::path::Path::new(&params.dest_path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            (
                error_codes::INTERNAL_ERROR,
                format!("Failed to create temp directory: {}", e),
            )
        })?;
    }

    // Write to local file, limiting to max_bytes
    let mut local_file = std::fs::File::create(&params.dest_path).map_err(|e| {
        (
            error_codes::INTERNAL_ERROR,
            format!("Failed to create temp file: {}", e),
        )
    })?;

    let mut limited = smb_file.take(params.max_bytes);
    let bytes_written = std::io::copy(&mut limited, &mut local_file).map_err(|e| {
        (
            error_codes::SMB_ERROR,
            format!("Failed to copy SMB file to temp: {}", e),
        )
    })?;

    local_file.flush().map_err(|e| {
        (
            error_codes::INTERNAL_ERROR,
            format!("Failed to flush temp file: {}", e),
        )
    })?;

    Ok(DownloadPartialResult {
        path: params.dest_path,
        bytes_written,
        total_size,
    })
}

/// Upload a local file to SMB.
pub fn upload_file(params: UploadFileParams) -> Result<UploadFileResult, (i32, String)> {
    let share = Share::open(&params.credentials, &params.share)?;

    let mut local_file = std::fs::File::open(&params.source_path).map_err(|e| {
        (
            error_codes::INTERNAL_ERROR,
            format!("Failed to open source file: {}", e),
        )
    })?;

    let mut smb_file = share
        .open_with(
            &params.dest_path,
            SmbOpenOptions::default()
                .write(true)
                .create(true)
                .exclusive(true),
        )
        .map_err(|e| share.failure("Failed to open SMB destination", e))?;

    let size = std::io::copy(&mut local_file, &mut smb_file).map_err(|e| {
        (
            error_codes::SMB_ERROR,
            format!("Failed to upload file: {}", e),
        )
    })?;

    smb_file.flush().map_err(|e| {
        (
            error_codes::SMB_ERROR,
            format!("Failed to flush SMB file: {}", e),
        )
    })?;

    Ok(UploadFileResult { size })
}

/// Resolve the smbclient command, searching common paths on macOS.
fn resolve_smbclient_command() -> (std::ffi::OsString, Option<std::ffi::OsString>) {
    use std::env;
    use std::ffi::{OsStr, OsString};
    use std::path::PathBuf;

    fn find_in_path(program: &str, path: &OsStr) -> Option<PathBuf> {
        env::split_paths(path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    }

    // Allow overriding the smbclient location when needed.
    if let Some(smbclient_path) = env::var_os("SMBCLIENT_PATH") {
        let smbclient_path = PathBuf::from(smbclient_path);
        if smbclient_path.is_file() {
            return (smbclient_path.into_os_string(), None);
        }
    }

    let current_path = env::var_os("PATH").unwrap_or_default();
    if let Some(found) = find_in_path("smbclient", &current_path) {
        return (found.into_os_string(), None);
    }

    #[cfg(target_os = "macos")]
    {
        // Packaged macOS apps (launched from Finder) often have a very minimal PATH
        // and won't include common Homebrew locations.
        let mut search_paths = vec![
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/opt/local/bin"),
        ];
        search_paths.extend(env::split_paths(&current_path));
        let augmented_path = env::join_paths(&search_paths).unwrap_or(current_path);

        if let Some(found) = find_in_path("smbclient", &augmented_path) {
            return (found.into_os_string(), Some(augmented_path));
        }

        (OsString::from("smbclient"), Some(augmented_path))
    }

    #[cfg(not(target_os = "macos"))]
    {
        (OsString::from("smbclient"), None)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        command_result, error_code, shares_from, smbclient_code, space_from, split_port,
        status_code,
    };
    use marlin_smb_client::protocol::error_codes;
    use std::io::{Error, ErrorKind};

    fn statvfs(bsize: u64, frsize: u64, blocks: u64, bavail: u64) -> pavao::SmbStatVfs {
        pavao::SmbStatVfs {
            bsize,
            frsize,
            blocks,
            bfree: 0,
            bavail,
            files: 0,
            ffree: 0,
            favail: 0,
            fsid: 0,
            flag: 0,
            namemax: 0,
        }
    }

    #[test]
    fn space_from_counts_allocation_units() {
        let samba = space_from(&statvfs(512, 2, 10, 4)).expect("size");
        assert_eq!(samba.total, 10240);
        assert_eq!(samba.free, 4096);

        let zero_frsize = space_from(&statvfs(4096, 0, 3, 1)).expect("size");
        assert_eq!(zero_frsize.total, 12288);
        assert_eq!(zero_frsize.free, 4096);

        assert!(space_from(&statvfs(0, 0, 0, 0)).is_none());
    }

    #[test]
    fn error_code_from_errno() {
        let code = |kind| error_code(&pavao::SmbError::Io(Error::from(kind)));

        assert_eq!(code(ErrorKind::NotFound), error_codes::SMB_PATH_NOT_FOUND);
        assert_eq!(code(ErrorKind::IsADirectory), error_codes::SMB_IS_A_FOLDER);
        assert_eq!(
            code(ErrorKind::NotADirectory),
            error_codes::SMB_NOT_A_FOLDER
        );
        assert_eq!(
            code(ErrorKind::PermissionDenied),
            error_codes::SMB_PERMISSION_DENIED
        );
        assert_eq!(
            code(ErrorKind::ConnectionRefused),
            error_codes::SMB_CONNECTION_LOST
        );
        assert_eq!(
            code(ErrorKind::ConnectionAborted),
            error_codes::SMB_CONNECTION_LOST
        );
        assert_eq!(code(ErrorKind::TimedOut), error_codes::SMB_CONNECTION_LOST);
        assert_eq!(code(ErrorKind::InvalidInput), error_codes::SMB_ERROR);
        assert_eq!(
            error_code(&pavao::SmbError::BadFileDescriptor),
            error_codes::SMB_ERROR
        );
    }

    #[test]
    fn split_port_keeps_an_address_whole() {
        assert_eq!(split_port("nas"), ("nas", None));
        assert_eq!(split_port("nas:4450"), ("nas", Some("4450")));
        assert_eq!(split_port("nas:abc"), ("nas:abc", None));
        assert_eq!(split_port("fe80::1"), ("fe80::1", None));
    }

    /// `smbclient -L -g` output, as smbclient 4.23.4 prints it.
    #[test]
    fn shares_from_the_listing() {
        let listed = shares_from(
            "Disk|data|\nDisk|denied|Team files | read only\nIPC|IPC$|IPC Service (Samba 4.23.4)\nDisk|C$|Default share\nSMB1 disabled -- no workgroup available\n",
        )
        .unwrap();
        let names: Vec<&str> = listed.iter().map(|share| share.name.as_str()).collect();
        assert_eq!(names, ["data", "denied"]);
        assert_eq!(listed[1].comment.as_deref(), Some("Team files | read only"));

        // A server whose only share is `IPC$` listed fine, and shows no folder.
        assert!(shares_from("IPC|IPC$|IPC Service\n").unwrap().is_empty());

        // No line at all is a listing that failed, though smbclient exited 0.
        for failed in [
            "SMB1 disabled -- no workgroup available\n",
            "Workgroup|WORKGROUP|NAS\n",
            "",
        ] {
            assert_eq!(shares_from(failed).unwrap_err().0, error_codes::SMB_ERROR);
        }
    }

    /// These are lines smbclient 4.23.4 prints.
    #[test]
    fn smbclient_code_from_what_it_printed() {
        assert_eq!(
            smbclient_code(
                "do_connect: Connection to 127.0.0.1 failed (Error NT_STATUS_CONNECTION_REFUSED)"
            ),
            error_codes::SMB_CONNECTION_LOST
        );
        assert_eq!(
            smbclient_code(
                "do_connect: Connection to 10.255.255.1 failed (Error NT_STATUS_IO_TIMEOUT)"
            ),
            error_codes::SMB_CONNECTION_LOST
        );
        // A LAN host that is turned off, on a Mac.
        assert_eq!(
            smbclient_code(
                "do_connect: Connection to 10.1.0.199 failed (Error NT_STATUS_ACCESS_DENIED)"
            ),
            error_codes::SMB_CONNECTION_LOST
        );
        assert_eq!(
            smbclient_code("session setup failed: NT_STATUS_LOGON_FAILURE"),
            error_codes::SMB_AUTH_FAILED
        );
        assert_eq!(
            smbclient_code("session setup failed: NT_STATUS_ACCOUNT_DISABLED"),
            error_codes::SMB_AUTH_FAILED
        );
    }

    #[test]
    fn status_code_for_each_status() {
        for (status, code) in [
            (
                "NT_STATUS_OBJECT_NAME_COLLISION",
                error_codes::SMB_ALREADY_EXISTS,
            ),
            (
                "NT_STATUS_OBJECT_NAME_NOT_FOUND",
                error_codes::SMB_PATH_NOT_FOUND,
            ),
            (
                "NT_STATUS_OBJECT_PATH_NOT_FOUND",
                error_codes::SMB_PATH_NOT_FOUND,
            ),
            ("NT_STATUS_NO_SUCH_FILE", error_codes::SMB_PATH_NOT_FOUND),
            ("NT_STATUS_DIRECTORY_NOT_EMPTY", error_codes::SMB_NOT_EMPTY),
            (
                "NT_STATUS_FILE_IS_A_DIRECTORY",
                error_codes::SMB_IS_A_FOLDER,
            ),
            ("NT_STATUS_NOT_A_DIRECTORY", error_codes::SMB_NOT_A_FOLDER),
            (
                "NT_STATUS_ACCESS_DENIED",
                error_codes::SMB_PERMISSION_DENIED,
            ),
            (
                "NT_STATUS_CONNECTION_RESET",
                error_codes::SMB_CONNECTION_LOST,
            ),
            ("NT_STATUS_IO_TIMEOUT", error_codes::SMB_CONNECTION_LOST),
            ("NT_STATUS_NOT_IMPLEMENTED", error_codes::SMB_ERROR),
        ] {
            assert_eq!(status_code(status), code, "{status}");
        }
    }

    /// Each line is a shape smbclient 4.23.4 prints. A status inside a path is not the status of the command.
    #[test]
    fn command_result_reads_the_line_not_the_names() {
        for (printed, expected) in [
            (
                "NT_STATUS_OBJECT_NAME_COLLISION renaming files \\a.txt -> \\b.txt \n",
                (
                    error_codes::SMB_ALREADY_EXISTS,
                    Some("NT_STATUS_OBJECT_NAME_COLLISION"),
                ),
            ),
            (
                "rename \\a.txt: NT_STATUS_NO_SUCH_FILE\n",
                (
                    error_codes::SMB_PATH_NOT_FOUND,
                    Some("NT_STATUS_NO_SUCH_FILE"),
                ),
            ),
            (
                "cd \\gone\\: NT_STATUS_OBJECT_NAME_NOT_FOUND\n",
                (
                    error_codes::SMB_PATH_NOT_FOUND,
                    Some("NT_STATUS_OBJECT_NAME_NOT_FOUND"),
                ),
            ),
            // A status or a phrase inside a name is not the command's status.
            (
                "NT_STATUS_OBJECT_NAME_NOT_FOUND renaming files \\NT_STATUS_CONNECTION_RESET.txt -> \\b.txt \n",
                (
                    error_codes::SMB_PATH_NOT_FOUND,
                    Some("NT_STATUS_OBJECT_NAME_NOT_FOUND"),
                ),
            ),
            (
                "NT_STATUS_OBJECT_NAME_NOT_FOUND renaming files \\session setup failed.txt -> \\b.txt \n",
                (
                    error_codes::SMB_PATH_NOT_FOUND,
                    Some("NT_STATUS_OBJECT_NAME_NOT_FOUND"),
                ),
            ),
            (
                "NT_STATUS_OBJECT_NAME_INVALID renaming files \\NT_STATUS_SHARING_VIOLATION.txt -> \\bad?.txt \n",
                (
                    error_codes::SMB_ERROR,
                    Some("NT_STATUS_OBJECT_NAME_INVALID"),
                ),
            ),
            (
                "do_connect: Connection to HOST failed (Error NT_STATUS_CONNECTION_REFUSED)\n",
                (error_codes::SMB_CONNECTION_LOST, None),
            ),
            (
                "session setup failed: NT_STATUS_LOGON_FAILURE\n",
                (error_codes::SMB_AUTH_FAILED, None),
            ),
            (
                "tree connect failed: NT_STATUS_ACCESS_DENIED\n",
                (error_codes::SMB_PERMISSION_DENIED, None),
            ),
            (
                "protocol negotiation failed: NT_STATUS_CONNECTION_RESET\n",
                (error_codes::SMB_CONNECTION_LOST, None),
            ),
            // Any other line that names a status fails, and says nothing more.
            (
                "NT_STATUS_INVALID_DEVICE_REQUEST getting shadow copy data for \\a.txt\n",
                (error_codes::SMB_ERROR, None),
            ),
            (
                "warning: saw NT_STATUS_CONNECTION_RESET in a note\n",
                (error_codes::SMB_ERROR, None),
            ),
            ("", (error_codes::SMB_ERROR, None)),
        ] {
            assert_eq!(command_result(printed, false), Err(expected), "{printed}");
        }
        assert_eq!(
            command_result("warning: saw NT_STATUS_UNSUCCESSFUL in a note\n", true),
            Err((error_codes::SMB_ERROR, None))
        );
        assert_eq!(command_result("", true), Ok(()));
    }
}
