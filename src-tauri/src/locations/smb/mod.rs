mod auth;
#[cfg(test)]
mod smb_test;

use async_trait::async_trait;
use crate::fs_utils::FileItem;
#[cfg(feature = "smb")]
use crate::fs_utils::is_hidden_file;
use crate::locations::{
    Location, LocationCapabilities, LocationProvider, ProviderDirectoryEntries,
};
#[cfg(feature = "smb")]
use crate::locations::LocationSummary;
#[cfg(feature = "smb")]
use std::sync::Mutex;
#[cfg(feature = "smb")]
use once_cell::sync::Lazy;

pub use auth::{
    add_smb_server, get_smb_servers, remove_smb_server, test_smb_connection,
    SmbServerInfo,
};
#[cfg(feature = "smb")]
pub use auth::get_server_credentials;

// Global mutex to serialize ALL SMB operations
// libsmbclient has global state and is NOT thread-safe, even across separate connections
#[cfg(feature = "smb")]
pub static SMB_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[derive(Default)]
pub struct SmbProvider;

#[async_trait]
impl LocationProvider for SmbProvider {
    fn scheme(&self) -> &'static str {
        "smb"
    }

    fn capabilities(&self, _location: &Location) -> LocationCapabilities {
        LocationCapabilities::new("smb", "SMB Share", true, true)
    }

    #[cfg(feature = "smb")]
    async fn read_directory(
        &self,
        location: &Location,
    ) -> Result<ProviderDirectoryEntries, String> {
        let authority = location
            .authority()
            .ok_or_else(|| "SMB path requires server: smb://server/share/path".to_string())?
            .to_string();

        let path = location.path().to_string();

        // If path is just "/" or empty, enumerate shares
        if path == "/" || path.is_empty() {
            return self.list_shares(&authority).await;
        }

        let (hostname, share, dir_path) = parse_smb_path(&authority, &path)?;

        // Get credentials
        let creds = get_server_credentials(&hostname)?;

        // Clone values for the blocking closure
        let hostname_clone = hostname.clone();
        let share_clone = share.clone();
        let dir_path_clone = dir_path.clone();

        // Run SMB operations on a blocking thread with mutex (libsmbclient is not thread-safe)
        let items = tokio::task::spawn_blocking(move || {
            use chrono::{DateTime, Utc};
            use pavao::{SmbClient, SmbCredentials, SmbDirentType, SmbOptions};

            // Acquire global SMB mutex - libsmbclient has global state
            let _guard = SMB_MUTEX.lock().map_err(|e| format!("SMB mutex poisoned: {}", e))?;

            // Build connection
            let smb_url = format!("smb://{}", hostname_clone);
            let share_path = if share_clone.starts_with('/') {
                share_clone.clone()
            } else {
                format!("/{}", share_clone)
            };

            let mut credentials = SmbCredentials::default()
                .server(&smb_url)
                .share(&share_path)
                .username(&creds.username)
                .password(&creds.password);

            if let Some(domain) = &creds.domain {
                credentials = credentials.workgroup(domain);
            }

            let client = SmbClient::new(credentials, SmbOptions::default())
                .map_err(|e| format!("Failed to connect to SMB server: {}", e))?;

            // Use list_dirplus to get file metadata (size, mtime) inline with listing
            // This uses SMB2's enhanced directory listing - no separate stat() calls needed!
            // Critical for performance with large directories (90k+ files)
            let entries = client
                .list_dirplus(&dir_path_clone)
                .map_err(|e| format!("Failed to list directory: {}", e))?;

            let mut items: Vec<FileItem> = Vec::new();

            for entry in entries {
                let name = entry.name();

                // Skip . and ..
                if name == "." || name == ".." {
                    continue;
                }

                let full_path = if dir_path_clone == "/" {
                    format!("smb://{}/{}/{}", hostname_clone, share_clone, name)
                } else {
                    format!("smb://{}/{}{}/{}", hostname_clone, share_clone, dir_path_clone, name)
                };

                // Use entry type from directory listing - no stat() calls needed!
                let entry_type = entry.get_type();
                let is_directory = matches!(entry_type, SmbDirentType::Dir);
                // Note: list_dirplus doesn't distinguish symlinks via attrs, default to false
                let is_symlink = false;

                // Convert SystemTime to DateTime<Utc>
                let modified: DateTime<Utc> = entry.mtime.into();

                items.push(FileItem {
                    name: name.to_string(),
                    path: full_path,
                    is_directory,
                    is_hidden: is_hidden_file(name),
                    size: entry.size, // Size from list_dirplus - no stat() needed!
                    modified,         // Modified time from list_dirplus - no stat() needed!
                    extension: if is_directory {
                        None
                    } else {
                        std::path::Path::new(name)
                            .extension()
                            .and_then(|e| e.to_str())
                            .map(|e| e.to_lowercase())
                    },
                    is_symlink,
                    is_git_repo: false,
                    child_count: None,
                    image_width: None,
                    image_height: None,
                    remote_id: None,
                    thumbnail_url: None,
                    download_url: None,
                });
            }

            // Sort: directories first, then by name
            items.sort_by(|a, b| match (a.is_directory, b.is_directory) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            });

            Ok::<Vec<FileItem>, String>(items)
        })
        .await
        .map_err(|e| format!("SMB task failed: {}", e))??;

        let display_path = format!("smb://{}/{}{}", hostname, share, dir_path);

        Ok(ProviderDirectoryEntries {
            location: LocationSummary::new(
                "smb",
                Some(hostname.clone()),
                format!("/{}{}", share, dir_path),
                display_path,
            ),
            entries: items,
        })
    }

    #[cfg(not(feature = "smb"))]
    async fn read_directory(
        &self,
        _location: &Location,
    ) -> Result<ProviderDirectoryEntries, String> {
        Err("SMB support not compiled. Build with --features smb".to_string())
    }

    #[cfg(feature = "smb")]
    async fn get_file_metadata(&self, location: &Location) -> Result<FileItem, String> {
        use chrono::{DateTime, Utc};
        use pavao::{SmbClient, SmbCredentials, SmbOptions};

        let authority = location
            .authority()
            .ok_or_else(|| "SMB path requires server".to_string())?;

        let (hostname, share, path) = parse_smb_path(authority, location.path())?;
        let creds = get_server_credentials(&hostname)?;

        let smb_url = format!("smb://{}", hostname);
        let share_path = if share.starts_with('/') {
            share.clone()
        } else {
            format!("/{}", share)
        };

        let mut credentials = SmbCredentials::default()
            .server(&smb_url)
            .share(&share_path)
            .username(&creds.username)
            .password(&creds.password);

        if let Some(domain) = &creds.domain {
            credentials = credentials.workgroup(domain);
        }

        let client = SmbClient::new(credentials, SmbOptions::default())
            .map_err(|e| format!("Failed to connect: {}", e))?;

        let stat = client
            .stat(&path)
            .map_err(|e| format!("Failed to get file metadata: {}", e))?;

        let name = std::path::Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&path)
            .to_string();

        let is_directory = stat.mode.is_dir();
        let modified: DateTime<Utc> = stat.modified.into();

        Ok(FileItem {
            name: name.clone(),
            path: location.raw().to_string(),
            is_directory,
            is_hidden: is_hidden_file(&name),
            size: stat.size,
            modified,
            extension: if is_directory {
                None
            } else {
                std::path::Path::new(&name)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase())
            },
            is_symlink: false,
            is_git_repo: false,
            child_count: None,
            image_width: None,
            image_height: None,
            remote_id: None,
            thumbnail_url: None,
            download_url: None,
        })
    }

    #[cfg(not(feature = "smb"))]
    async fn get_file_metadata(&self, _location: &Location) -> Result<FileItem, String> {
        Err("SMB support not compiled. Build with --features smb".to_string())
    }

    #[cfg(feature = "smb")]
    async fn create_directory(&self, location: &Location) -> Result<(), String> {
        use pavao::{SmbClient, SmbCredentials, SmbMode, SmbOptions};

        let authority = location
            .authority()
            .ok_or_else(|| "SMB path requires server".to_string())?;

        let (hostname, share, path) = parse_smb_path(authority, location.path())?;
        let creds = get_server_credentials(&hostname)?;

        let smb_url = format!("smb://{}", hostname);
        let share_path = if share.starts_with('/') {
            share.clone()
        } else {
            format!("/{}", share)
        };

        let mut credentials = SmbCredentials::default()
            .server(&smb_url)
            .share(&share_path)
            .username(&creds.username)
            .password(&creds.password);

        if let Some(domain) = &creds.domain {
            credentials = credentials.workgroup(domain);
        }

        let client = SmbClient::new(credentials, SmbOptions::default())
            .map_err(|e| format!("Failed to connect: {}", e))?;

        client
            .mkdir(&path, SmbMode::from(0o755))
            .map_err(|e| format!("Failed to create directory: {}", e))
    }

    #[cfg(not(feature = "smb"))]
    async fn create_directory(&self, _location: &Location) -> Result<(), String> {
        Err("SMB support not compiled. Build with --features smb".to_string())
    }

    #[cfg(feature = "smb")]
    async fn delete(&self, location: &Location) -> Result<(), String> {
        use pavao::{SmbClient, SmbCredentials, SmbOptions};

        let authority = location
            .authority()
            .ok_or_else(|| "SMB path requires server".to_string())?;

        let (hostname, share, path) = parse_smb_path(authority, location.path())?;
        let creds = get_server_credentials(&hostname)?;

        let smb_url = format!("smb://{}", hostname);
        let share_path = if share.starts_with('/') {
            share.clone()
        } else {
            format!("/{}", share)
        };

        let mut credentials = SmbCredentials::default()
            .server(&smb_url)
            .share(&share_path)
            .username(&creds.username)
            .password(&creds.password);

        if let Some(domain) = &creds.domain {
            credentials = credentials.workgroup(domain);
        }

        let client = SmbClient::new(credentials, SmbOptions::default())
            .map_err(|e| format!("Failed to connect: {}", e))?;

        // Check if it's a directory or file
        let stat = client
            .stat(&path)
            .map_err(|e| format!("Failed to stat path: {}", e))?;

        if stat.mode.is_dir() {
            client
                .rmdir(&path)
                .map_err(|e| format!("Failed to delete directory: {}", e))
        } else {
            client
                .unlink(&path)
                .map_err(|e| format!("Failed to delete file: {}", e))
        }
    }

    #[cfg(not(feature = "smb"))]
    async fn delete(&self, _location: &Location) -> Result<(), String> {
        Err("SMB support not compiled. Build with --features smb".to_string())
    }

    #[cfg(feature = "smb")]
    async fn rename(&self, from: &Location, to: &Location) -> Result<(), String> {
        use pavao::{SmbClient, SmbCredentials, SmbOptions};

        let from_authority = from
            .authority()
            .ok_or_else(|| "SMB path requires server".to_string())?;
        let to_authority = to
            .authority()
            .ok_or_else(|| "SMB path requires server".to_string())?;

        if from_authority != to_authority {
            return Err("Cannot rename across different servers".to_string());
        }

        let (hostname, share, from_path) = parse_smb_path(from_authority, from.path())?;
        let (_, to_share, to_path) = parse_smb_path(to_authority, to.path())?;

        if share != to_share {
            return Err("Cannot rename across different shares".to_string());
        }

        let creds = get_server_credentials(&hostname)?;

        let smb_url = format!("smb://{}", hostname);
        let share_path = if share.starts_with('/') {
            share.clone()
        } else {
            format!("/{}", share)
        };

        let mut credentials = SmbCredentials::default()
            .server(&smb_url)
            .share(&share_path)
            .username(&creds.username)
            .password(&creds.password);

        if let Some(domain) = &creds.domain {
            credentials = credentials.workgroup(domain);
        }

        let client = SmbClient::new(credentials, SmbOptions::default())
            .map_err(|e| format!("Failed to connect: {}", e))?;

        client
            .rename(&from_path, &to_path)
            .map_err(|e| format!("Failed to rename: {}", e))
    }

    #[cfg(not(feature = "smb"))]
    async fn rename(&self, _from: &Location, _to: &Location) -> Result<(), String> {
        Err("SMB support not compiled. Build with --features smb".to_string())
    }

    async fn copy(&self, _from: &Location, _to: &Location) -> Result<(), String> {
        // SMB doesn't have native copy - would need to read+write
        Err("Copy not yet implemented for SMB".to_string())
    }
}

impl SmbProvider {
    #[cfg(feature = "smb")]
    async fn list_shares(&self, hostname: &str) -> Result<ProviderDirectoryEntries, String> {
        use chrono::Utc;
        use std::process::Command;

        let creds = get_server_credentials(hostname)?;

        // Use smbclient -L to enumerate shares (pavao doesn't support this well)
        let mut cmd = Command::new("smbclient");
        cmd.arg("-L")
            .arg(format!("//{}", hostname))
            .arg("-U")
            .arg(format!("{}%{}", creds.username, creds.password))
            .arg("-g"); // Machine-readable output

        if let Some(domain) = &creds.domain {
            cmd.arg("-W").arg(domain);
        }

        let output = cmd
            .output()
            .map_err(|e| format!("Failed to run smbclient: {}. Make sure samba is installed (brew install samba)", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Check for common errors
            if stderr.contains("NT_STATUS_LOGON_FAILURE") {
                return Err(format!("Authentication failed. Try using your full email as username (e.g., user@domain.com)"));
            }
            return Err(format!("Failed to list shares: {}", stderr.trim()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse the -g (grep-friendly) output format: type|name|comment
        let items: Vec<FileItem> = stdout
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split('|').collect();
                if parts.len() < 2 {
                    return None;
                }

                let share_type = parts[0];
                let name = parts[1];

                // Only include Disk shares, skip IPC$, ADMIN$, etc.
                if share_type != "Disk" {
                    return None;
                }

                // Skip hidden shares (ending in $)
                if name.ends_with('$') {
                    return None;
                }

                Some(FileItem {
                    name: name.to_string(),
                    path: format!("smb://{}/{}", hostname, name),
                    is_directory: true,
                    is_hidden: false,
                    size: 0,
                    modified: Utc::now(),
                    extension: None,
                    is_symlink: false,
                    is_git_repo: false,
                    child_count: None,
                    image_width: None,
                    image_height: None,
                    remote_id: None,
                    thumbnail_url: None,
                    download_url: None,
                })
            })
            .collect();

        Ok(ProviderDirectoryEntries {
            location: LocationSummary::new(
                "smb",
                Some(hostname.to_string()),
                "/",
                format!("smb://{}", hostname),
            ),
            entries: items,
        })
    }

    #[cfg(not(feature = "smb"))]
    #[allow(dead_code)]
    async fn list_shares(&self, _hostname: &str) -> Result<ProviderDirectoryEntries, String> {
        Err("SMB support not compiled. Build with --features smb".to_string())
    }
}

/// Parse an SMB path into (hostname, share, path)
#[cfg(feature = "smb")]
fn parse_smb_path(authority: &str, path: &str) -> Result<(String, String, String), String> {
    let hostname = authority.to_string();

    // Path format: /share/rest/of/path
    let path = if path.starts_with('/') {
        &path[1..]
    } else {
        path
    };

    let mut parts = path.splitn(2, '/');
    let share = parts.next().unwrap_or("").to_string();
    let file_path = parts
        .next()
        .map(|p| format!("/{}", p))
        .unwrap_or_else(|| "/".to_string());

    if share.is_empty() {
        return Err("SMB path must include share name: smb://server/share/path".to_string());
    }

    Ok((hostname, share, file_path))
}

/// Extract credentials from SMB URL if present
/// Format: smb://user:pass@server/share or smb://domain;user:pass@server/share
#[allow(dead_code)]
pub fn extract_url_credentials(url: &str) -> Option<(String, String, String, Option<String>)> {
    // Parse smb://[domain;]user:pass@server/...
    let without_scheme = url.strip_prefix("smb://")?;

    let at_pos = without_scheme.find('@')?;
    let auth_part = &without_scheme[..at_pos];
    let server_part = &without_scheme[at_pos + 1..];

    // Extract server (before first /)
    let server = server_part.split('/').next()?.to_string();

    // Check for domain;user:pass format
    let (domain, user_pass) = if auth_part.contains(';') {
        let mut parts = auth_part.splitn(2, ';');
        (Some(parts.next()?.to_string()), parts.next()?)
    } else {
        (None, auth_part)
    };

    // Split user:pass
    let colon_pos = user_pass.find(':')?;
    let username = user_pass[..colon_pos].to_string();
    let password = user_pass[colon_pos + 1..].to_string();

    // URL decode the parts
    let username = urlencoding::decode(&username).ok()?.to_string();
    let password = urlencoding::decode(&password).ok()?.to_string();
    let domain = domain.and_then(|d| urlencoding::decode(&d).ok().map(|s| s.to_string()));

    Some((server, username, password, domain))
}

/// Strip credentials from URL for display/storage
#[allow(dead_code)]
pub fn strip_url_credentials(url: &str) -> String {
    if let Some(without_scheme) = url.strip_prefix("smb://") {
        if let Some(at_pos) = without_scheme.find('@') {
            return format!("smb://{}", &without_scheme[at_pos + 1..]);
        }
    }
    url.to_string()
}

#[cfg(all(test, feature = "smb"))]
mod tests {
    use super::*;

    #[test]
    fn test_parse_smb_path() {
        let (host, share, path) = parse_smb_path("server.local", "/myshare/folder/file.txt").unwrap();
        assert_eq!(host, "server.local");
        assert_eq!(share, "myshare");
        assert_eq!(path, "/folder/file.txt");
    }

    #[test]
    fn test_parse_smb_path_root() {
        let (host, share, path) = parse_smb_path("server.local", "/myshare").unwrap();
        assert_eq!(host, "server.local");
        assert_eq!(share, "myshare");
        assert_eq!(path, "/");
    }

    #[test]
    fn test_extract_url_credentials() {
        let result = extract_url_credentials("smb://user:password@server.local/share");
        assert!(result.is_some());
        let (server, user, pass, domain) = result.unwrap();
        assert_eq!(server, "server.local");
        assert_eq!(user, "user");
        assert_eq!(pass, "password");
        assert!(domain.is_none());
    }

    #[test]
    fn test_extract_url_credentials_with_domain() {
        let result = extract_url_credentials("smb://CORP;user:password@server.local/share");
        assert!(result.is_some());
        let (server, user, pass, domain) = result.unwrap();
        assert_eq!(server, "server.local");
        assert_eq!(user, "user");
        assert_eq!(pass, "password");
        assert_eq!(domain, Some("CORP".to_string()));
    }

    #[test]
    fn test_strip_url_credentials() {
        let url = "smb://user:password@server.local/share/path";
        let stripped = strip_url_credentials(url);
        assert_eq!(stripped, "smb://server.local/share/path");
    }
}
