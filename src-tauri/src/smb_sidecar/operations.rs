//! SMB operations using pavao/libsmbclient.
//!
//! This module is only compiled when the `smb-sidecar` feature is enabled.
//! All operations receive credentials as parameters (no keychain access).

use crate::smb_sidecar::protocol::{
    error_codes, CopyParams, CreateDirectoryParams, DeleteParams, DirectoryEntry,
    DownloadFileParams, DownloadFileResult, FileMetadataResult, GetFileMetadataParams,
    ListSharesParams, ListSharesResult, ReadDirectoryParams, ReadDirectoryResult, RenameParams,
    ShareEntry, SmbCredentials, TestConnectionParams, TestConnectionResult, UploadFileParams,
    UploadFileResult,
};
use once_cell::sync::Lazy;
use pavao::{SmbClient, SmbCredentials as PavaoCredentials, SmbMode, SmbOpenOptions, SmbOptions};
use std::io::Write;
use std::sync::Mutex;

/// Global mutex to serialize ALL SMB operations.
/// libsmbclient has global state and is NOT thread-safe, even across separate connections.
static SMB_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

/// Sidecar version string.
pub const SIDECAR_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Build pavao credentials from our protocol credentials.
fn build_credentials(creds: &SmbCredentials, share: &str) -> PavaoCredentials {
    let smb_url = format!("smb://{}", creds.hostname);
    let share_path = if share.starts_with('/') {
        share.to_string()
    } else {
        format!("/{}", share)
    };

    let mut credentials = PavaoCredentials::default()
        .server(&smb_url)
        .share(&share_path)
        .username(&creds.username)
        .password(&creds.password);

    if let Some(domain) = &creds.domain {
        credentials = credentials.workgroup(domain);
    }

    credentials
}

/// Map pavao errors to our error codes.
fn map_smb_error(e: &pavao::SmbError) -> (i32, String) {
    let msg = e.to_string();
    let code = if msg.contains("LOGON_FAILURE") || msg.contains("authentication") {
        error_codes::SMB_AUTH_FAILED
    } else if msg.contains("NOT_FOUND") || msg.contains("No such file") {
        error_codes::SMB_PATH_NOT_FOUND
    } else if msg.contains("ACCESS_DENIED") || msg.contains("Permission denied") {
        error_codes::SMB_PERMISSION_DENIED
    } else {
        error_codes::SMB_ERROR
    };
    (code, msg)
}

/// Check if a file/directory name should be considered hidden.
fn is_hidden_file(name: &str) -> bool {
    name.starts_with('.')
}

/// Read directory contents.
pub fn read_directory(params: ReadDirectoryParams) -> Result<ReadDirectoryResult, (i32, String)> {
    let _guard = SMB_MUTEX
        .lock()
        .map_err(|e| (error_codes::INTERNAL_ERROR, format!("SMB mutex poisoned: {}", e)))?;

    let credentials = build_credentials(&params.credentials, &params.share);

    let client = SmbClient::new(credentials, SmbOptions::default())
        .map_err(|e| {
            let (code, msg) = map_smb_error(&e);
            (code, format!("Failed to connect to SMB server: {}", msg))
        })?;

    // Use list_dirplus to get file metadata inline with listing
    let entries = client
        .list_dirplus(&params.path)
        .map_err(|e| {
            let (code, msg) = map_smb_error(&e);
            (code, format!("Failed to list directory: {}", msg))
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
pub fn get_file_metadata(params: GetFileMetadataParams) -> Result<FileMetadataResult, (i32, String)> {
    let _guard = SMB_MUTEX
        .lock()
        .map_err(|e| (error_codes::INTERNAL_ERROR, format!("SMB mutex poisoned: {}", e)))?;

    let credentials = build_credentials(&params.credentials, &params.share);

    let client = SmbClient::new(credentials, SmbOptions::default())
        .map_err(|e| {
            let (code, msg) = map_smb_error(&e);
            (code, format!("Failed to connect to SMB server: {}", msg))
        })?;

    let stat = client
        .stat(&params.path)
        .map_err(|e| {
            let (code, msg) = map_smb_error(&e);
            (code, format!("Failed to get file metadata: {}", msg))
        })?;

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
    let _guard = SMB_MUTEX
        .lock()
        .map_err(|e| (error_codes::INTERNAL_ERROR, format!("SMB mutex poisoned: {}", e)))?;

    let credentials = build_credentials(&params.credentials, &params.share);

    let client = SmbClient::new(credentials, SmbOptions::default())
        .map_err(|e| {
            let (code, msg) = map_smb_error(&e);
            (code, format!("Failed to connect to SMB server: {}", msg))
        })?;

    client
        .mkdir(&params.path, SmbMode::from(0o755))
        .map_err(|e| {
            let (code, msg) = map_smb_error(&e);
            (code, format!("Failed to create directory: {}", msg))
        })
}

/// Delete a file or directory.
pub fn delete(params: DeleteParams) -> Result<(), (i32, String)> {
    let _guard = SMB_MUTEX
        .lock()
        .map_err(|e| (error_codes::INTERNAL_ERROR, format!("SMB mutex poisoned: {}", e)))?;

    let credentials = build_credentials(&params.credentials, &params.share);

    let client = SmbClient::new(credentials, SmbOptions::default())
        .map_err(|e| {
            let (code, msg) = map_smb_error(&e);
            (code, format!("Failed to connect to SMB server: {}", msg))
        })?;

    let stat = client
        .stat(&params.path)
        .map_err(|e| {
            let (code, msg) = map_smb_error(&e);
            (code, format!("Failed to stat path: {}", msg))
        })?;

    if stat.mode.is_dir() {
        client
            .rmdir(&params.path)
            .map_err(|e| {
                let (code, msg) = map_smb_error(&e);
                (code, format!("Failed to delete directory: {}", msg))
            })
    } else {
        client
            .unlink(&params.path)
            .map_err(|e| {
                let (code, msg) = map_smb_error(&e);
                (code, format!("Failed to delete file: {}", msg))
            })
    }
}

/// Rename a file or directory.
pub fn rename(params: RenameParams) -> Result<(), (i32, String)> {
    let _guard = SMB_MUTEX
        .lock()
        .map_err(|e| (error_codes::INTERNAL_ERROR, format!("SMB mutex poisoned: {}", e)))?;

    let credentials = build_credentials(&params.credentials, &params.share);

    let client = SmbClient::new(credentials, SmbOptions::default())
        .map_err(|e| {
            let (code, msg) = map_smb_error(&e);
            (code, format!("Failed to connect to SMB server: {}", msg))
        })?;

    client
        .rename(&params.from_path, &params.to_path)
        .map_err(|e| {
            let (code, msg) = map_smb_error(&e);
            (code, format!("Failed to rename: {}", msg))
        })
}

/// Copy a file.
pub fn copy(params: CopyParams) -> Result<(), (i32, String)> {
    let _guard = SMB_MUTEX
        .lock()
        .map_err(|e| (error_codes::INTERNAL_ERROR, format!("SMB mutex poisoned: {}", e)))?;

    let credentials = build_credentials(&params.credentials, &params.share);

    let client = SmbClient::new(credentials, SmbOptions::default())
        .map_err(|e| {
            let (code, msg) = map_smb_error(&e);
            (code, format!("Failed to connect to SMB server: {}", msg))
        })?;

    let mut src = client
        .open_with(&params.from_path, SmbOpenOptions::default().read(true))
        .map_err(|e| {
            let (code, msg) = map_smb_error(&e);
            (code, format!("Failed to open source: {}", msg))
        })?;

    let mut dst = client
        .open_with(
            &params.to_path,
            SmbOpenOptions::default().write(true).create(true).truncate(true),
        )
        .map_err(|e| {
            let (code, msg) = map_smb_error(&e);
            (code, format!("Failed to open destination: {}", msg))
        })?;

    std::io::copy(&mut src, &mut dst)
        .map_err(|e| (error_codes::SMB_ERROR, format!("Failed to copy: {}", e)))?;

    Ok(())
}

/// List available shares on a server.
pub fn list_shares(params: ListSharesParams) -> Result<ListSharesResult, (i32, String)> {
    use std::process::Command;
    use uuid::Uuid;

    // Use smbclient -L to enumerate shares (pavao doesn't support this well)
    let (smbclient_program, augmented_path) = resolve_smbclient_command();
    let mut cmd = Command::new(smbclient_program);
    if let Some(path_env) = augmented_path {
        cmd.env("PATH", path_env);
    }
    cmd.arg("-L")
        .arg(format!("//{}", params.credentials.hostname))
        .arg("-g"); // Machine-readable output

    // Avoid putting credentials on the process command line.
    // smbclient supports reading auth data from an authfile via -A.
    let auth_file_path =
        std::env::temp_dir().join(format!("marlin-smb-auth-{}.conf", Uuid::new_v4()));
    let auth_file_contents = {
        let mut s = format!(
            "username = {}\npassword = {}\n",
            params.credentials.username, params.credentials.password
        );
        if let Some(domain) = &params.credentials.domain {
            s.push_str(&format!("domain = {}\n", domain));
        }
        s
    };

    std::fs::write(&auth_file_path, auth_file_contents)
        .map_err(|e| (error_codes::INTERNAL_ERROR, format!("Failed to create smbclient auth file: {}", e)))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(&auth_file_path) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(&auth_file_path, perms);
        }
    }

    cmd.arg("-A").arg(&auth_file_path);

    let output = cmd.output();
    let _ = std::fs::remove_file(&auth_file_path);

    let output = output.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            (error_codes::SMB_LIBRARY_MISSING, "smbclient not found. Install Samba (macOS: `brew install samba`, Linux: `sudo apt-get install smbclient`).".to_string())
        } else {
            (error_codes::SMB_ERROR, format!("Failed to run smbclient: {}", e))
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("NT_STATUS_LOGON_FAILURE") {
            return Err((
                error_codes::SMB_AUTH_FAILED,
                "Authentication failed. Try using your full email as username (e.g., user@domain.com)".to_string(),
            ));
        }
        return Err((error_codes::SMB_ERROR, format!("Failed to list shares: {}", stderr.trim())));
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

/// Test connection to an SMB server.
pub fn test_connection(params: TestConnectionParams) -> Result<TestConnectionResult, (i32, String)> {
    let _guard = SMB_MUTEX
        .lock()
        .map_err(|e| (error_codes::INTERNAL_ERROR, format!("SMB mutex poisoned: {}", e)))?;

    let smb_url = format!("smb://{}", params.credentials.hostname);

    let mut credentials = PavaoCredentials::default()
        .server(&smb_url)
        .share("/")
        .username(&params.credentials.username)
        .password(&params.credentials.password);

    if let Some(domain) = &params.credentials.domain {
        credentials = credentials.workgroup(domain);
    }

    match SmbClient::new(credentials, SmbOptions::default()) {
        Ok(_) => Ok(TestConnectionResult { success: true }),
        Err(e) => {
            let (code, msg) = map_smb_error(&e);
            Err((code, format!("Connection failed: {}", msg)))
        }
    }
}

/// Download a file to a local path.
pub fn download_file(params: DownloadFileParams) -> Result<DownloadFileResult, (i32, String)> {
    let _guard = SMB_MUTEX
        .lock()
        .map_err(|e| (error_codes::INTERNAL_ERROR, format!("SMB mutex poisoned: {}", e)))?;

    let credentials = build_credentials(&params.credentials, &params.share);

    let client = SmbClient::new(credentials, SmbOptions::default())
        .map_err(|e| {
            let (code, msg) = map_smb_error(&e);
            (code, format!("Failed to connect to SMB server: {}", msg))
        })?;

    // Open the remote file for reading
    let mut smb_file = client
        .open_with(&params.path, SmbOpenOptions::default().read(true))
        .map_err(|e| {
            let (code, msg) = map_smb_error(&e);
            (code, format!("Failed to open SMB file: {}", msg))
        })?;

    // Create parent directories if needed
    if let Some(parent) = std::path::Path::new(&params.dest_path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| (error_codes::INTERNAL_ERROR, format!("Failed to create temp directory: {}", e)))?;
    }

    // Write to local file
    let mut local_file = std::fs::File::create(&params.dest_path)
        .map_err(|e| (error_codes::INTERNAL_ERROR, format!("Failed to create temp file: {}", e)))?;

    let size = std::io::copy(&mut smb_file, &mut local_file)
        .map_err(|e| (error_codes::SMB_ERROR, format!("Failed to copy SMB file to temp: {}", e)))?;

    local_file
        .flush()
        .map_err(|e| (error_codes::INTERNAL_ERROR, format!("Failed to flush temp file: {}", e)))?;

    Ok(DownloadFileResult {
        path: params.dest_path,
        size,
    })
}

/// Upload a local file to SMB.
pub fn upload_file(params: UploadFileParams) -> Result<UploadFileResult, (i32, String)> {
    let _guard = SMB_MUTEX
        .lock()
        .map_err(|e| (error_codes::INTERNAL_ERROR, format!("SMB mutex poisoned: {}", e)))?;

    let credentials = build_credentials(&params.credentials, &params.share);

    let client = SmbClient::new(credentials, SmbOptions::default())
        .map_err(|e| {
            let (code, msg) = map_smb_error(&e);
            (code, format!("Failed to connect to SMB server: {}", msg))
        })?;

    let mut local_file = std::fs::File::open(&params.source_path)
        .map_err(|e| (error_codes::INTERNAL_ERROR, format!("Failed to open source file: {}", e)))?;

    let mut smb_file = client
        .open_with(
            &params.dest_path,
            SmbOpenOptions::default()
                .write(true)
                .create(true)
                .exclusive(true),
        )
        .map_err(|e| {
            let (code, msg) = map_smb_error(&e);
            (code, format!("Failed to open SMB destination: {}", msg))
        })?;

    let size = std::io::copy(&mut local_file, &mut smb_file)
        .map_err(|e| (error_codes::SMB_ERROR, format!("Failed to upload file: {}", e)))?;

    smb_file
        .flush()
        .map_err(|e| (error_codes::SMB_ERROR, format!("Failed to flush SMB file: {}", e)))?;

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

    fn join_paths_lossy(paths: &[PathBuf], fallback: OsString) -> OsString {
        env::join_paths(paths).unwrap_or(fallback)
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
        let augmented_path = join_paths_lossy(&search_paths, current_path);

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
