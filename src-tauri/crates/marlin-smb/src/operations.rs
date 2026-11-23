//! SMB operations using pavao/libsmbclient.
//!
//! All operations receive credentials as parameters (no keychain access).

use marlin_smb_client::protocol::{
    error_codes, CopyParams, CreateDirectoryParams, DeleteParams, DirectoryEntry,
    DownloadFileParams, DownloadFileResult, DownloadPartialParams, DownloadPartialResult,
    FileMetadataResult, GetFileMetadataParams, ListSharesParams, ListSharesResult,
    ReadDirectoryParams, ReadDirectoryResult, ReadParams, ReadResult, RenameParams, ShareEntry,
    SmbCredentials, SpaceParams, SpaceResult, UploadFileParams, UploadFileResult, READ_MAX,
};
use pavao::{SmbClient, SmbCredentials as PavaoCredentials, SmbMode, SmbOpenOptions, SmbOptions};
use std::io::{Read, Seek, SeekFrom, Write};

/// Sidecar version string.
pub const SIDECAR_VERSION: &str = env!("CARGO_PKG_VERSION");

/// One connection to a share, for one call. pavao connects at the first call, not when it opens. The helper runs one request at a time, and pavao holds one lock around every libsmbclient call, whose global state is not thread-safe.
struct Share<'a> {
    client: SmbClient,
    credentials: &'a SmbCredentials,
}

impl<'a> Share<'a> {
    fn open(credentials: &'a SmbCredentials, share: &str) -> Result<Self, (i32, String)> {
        // pavao adds the `/` before the share when it is missing.
        let mut server = PavaoCredentials::default()
            .server(format!("smb://{}", credentials.hostname))
            .share(share)
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

    // pavao drops opendir's errno (it answers BadFileDescriptor), so a stat of the same path finds out why the listing failed. A folder that is there and still cannot be listed was refused: the stat proves the sign-in works.
    let entries = share.client.list_dirplus(&params.path).map_err(|e| {
        match share.client.stat(&params.path) {
            Err(cause) => share.failure("Failed to list directory", cause),
            Ok(stat) if stat.mode.is_dir() => (
                error_codes::SMB_PERMISSION_DENIED,
                "Failed to list directory: permission denied".to_string(),
            ),
            Ok(_) => share.failure("Failed to list directory", e),
        }
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
        .client
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
        .client
        .mkdir(&params.path, SmbMode::from(0o755))
        .map_err(|e| share.failure("Failed to create directory", e))
}

/// Delete a file or directory.
pub fn delete(params: DeleteParams) -> Result<(), (i32, String)> {
    let share = Share::open(&params.credentials, &params.share)?;

    let stat = share
        .client
        .stat(&params.path)
        .map_err(|e| share.failure("Failed to stat path", e))?;

    if stat.mode.is_dir() {
        share
            .client
            .rmdir(&params.path)
            .map_err(|e| share.failure("Failed to delete directory", e))
    } else {
        share
            .client
            .unlink(&params.path)
            .map_err(|e| share.failure("Failed to delete file", e))
    }
}

/// Rename a file or directory.
pub fn rename(params: RenameParams) -> Result<(), (i32, String)> {
    let share = Share::open(&params.credentials, &params.share)?;

    share
        .client
        .rename(&params.from_path, &params.to_path)
        .map_err(|e| share.failure("Failed to rename", e))
}

/// Copy a file.
pub fn copy(params: CopyParams) -> Result<(), (i32, String)> {
    let share = Share::open(&params.credentials, &params.share)?;

    let mut src = share
        .client
        .open_with(&params.from_path, SmbOpenOptions::default().read(true))
        .map_err(|e| share.failure("Failed to open source", e))?;

    let mut dst = share
        .client
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
        .client
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
        .client
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

/// The other methods put the port in the URL, which libsmbclient reads. `smbclient` takes it only as `-p`. An IPv6 address stays whole: it has colons of its own, and neither tool takes one with a port (#642).
fn split_port(hostname: &str) -> (&str, Option<&str>) {
    match hostname.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') && port.parse::<u16>().is_ok() => {
            (host, Some(port))
        }
        _ => (hostname, None),
    }
}

/// The code for a failed `smbclient -L`, from the lines it printed.
fn smbclient_code(reported: &str) -> i32 {
    let unreachable = [
        "NT_STATUS_CONNECTION_REFUSED",
        "NT_STATUS_IO_TIMEOUT",
        "NT_STATUS_HOST_UNREACHABLE",
        "NT_STATUS_NETWORK_UNREACHABLE",
        "NT_STATUS_CONNECTION_RESET",
        "NT_STATUS_CONNECTION_DISCONNECTED",
    ];
    // A `do_connect` line means smbclient never reached the server, whatever its status. On a Mac, a LAN host that is turned off answers NT_STATUS_ACCESS_DENIED there: the socket's EHOSTDOWN.
    if reported.contains("do_connect:")
        || unreachable.iter().any(|status| reported.contains(status))
    {
        error_codes::SMB_CONNECTION_LOST
    } else if reported.contains("session setup failed") {
        // A wrong password, or a locked, disabled, or expired account.
        error_codes::SMB_AUTH_FAILED
    } else if reported.contains("NT_STATUS_ACCESS_DENIED") {
        // Signed in, but the server does not let this user list its shares.
        error_codes::SMB_PERMISSION_DENIED
    } else {
        error_codes::SMB_ERROR
    }
}

/// List available shares on a server.
pub fn list_shares(params: ListSharesParams) -> Result<ListSharesResult, (i32, String)> {
    use std::process::{Command, Stdio};

    // Use smbclient -L to enumerate shares (pavao doesn't support this well)
    let (smbclient_program, augmented_path) = resolve_smbclient_command();
    let mut cmd = Command::new(smbclient_program);
    if let Some(path_env) = augmented_path {
        cmd.env("PATH", path_env);
    }
    let (host, port) = split_port(&params.credentials.hostname);
    cmd.arg("-L").arg(format!("//{host}")).arg("-g"); // Machine-readable output
    if let Some(port) = port {
        cmd.arg("-p").arg(port);
    }
    cmd.arg("-A")
        .arg("/dev/stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // smbclient reads its auth text from stdin (-A /dev/stdin), so the credentials reach
    // neither the command line nor a file.
    let auth_text = {
        let mut s = format!(
            "username = {}\npassword = {}\n",
            params.credentials.username, params.credentials.password
        );
        if let Some(domain) = &params.credentials.domain {
            s.push_str(&format!("domain = {}\n", domain));
        }
        s
    };

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err((error_codes::SMB_LIBRARY_MISSING, "smbclient not found. Install Samba (macOS: `brew install samba`, Linux: `sudo apt-get install smbclient`).".to_string()));
        }
        Err(e) => {
            return Err((
                error_codes::SMB_ERROR,
                format!("Failed to run smbclient: {}", e),
            ));
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        // A smbclient that exited before it read this fails below, with its own stderr.
        let _ = stdin.write_all(auth_text.as_bytes());
    }

    let output = child.wait_with_output().map_err(|e| {
        (
            error_codes::SMB_ERROR,
            format!("Failed to run smbclient: {}", e),
        )
    })?;

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

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse the -g (grep-friendly) output format: type|name|comment
    let shares: Vec<ShareEntry> = stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() < 2 {
                return None;
            }

            let share_type = parts[0];
            let name = parts[1];
            let comment = parts.get(2).map(|s| s.to_string());

            // Only include Disk shares, skip IPC$, ADMIN$, etc.
            if share_type != "Disk" {
                return None;
            }

            // Skip hidden shares (ending in $)
            if name.ends_with('$') {
                return None;
            }

            Some(ShareEntry {
                name: name.to_string(),
                comment,
            })
        })
        .collect();

    Ok(ListSharesResult { shares })
}

/// Download a file to a local path.
pub fn download_file(params: DownloadFileParams) -> Result<DownloadFileResult, (i32, String)> {
    let share = Share::open(&params.credentials, &params.share)?;

    // Open the remote file for reading
    let mut smb_file = share
        .client
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
        .client
        .stat(&params.path)
        .map_err(|e| share.failure("Failed to stat SMB file", e))?;
    let total_size = stat.size;

    // Open the remote file for reading
    let smb_file = share
        .client
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
        .client
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

        return (OsString::from("smbclient"), Some(augmented_path));
    }

    #[cfg(not(target_os = "macos"))]
    {
        (OsString::from("smbclient"), None)
    }
}

#[cfg(test)]
mod tests {
    use super::{error_code, smbclient_code, space_from, split_port};
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
}
