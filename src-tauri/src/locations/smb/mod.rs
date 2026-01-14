mod auth;

use async_trait::async_trait;
use crate::fs_utils::FileItem;
use crate::locations::{
    Location, LocationCapabilities, LocationProvider, ProviderDirectoryEntries,
};
#[cfg(feature = "smb")]
use crate::locations::LocationSummary;

pub use auth::{
    add_smb_server, get_smb_servers, remove_smb_server, test_smb_connection,
    SmbServerInfo,
};
#[cfg(feature = "smb")]
pub use auth::get_server_credentials;

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
        use chrono::{DateTime, Utc};
        use pavao::{SmbClient, SmbCredentials, SmbOptions};

        let authority = location
            .authority()
            .ok_or_else(|| "SMB path requires server: smb://server/share/path".to_string())?;

        let path = location.path();

        // If path is just "/" or empty, enumerate shares
        if path == "/" || path.is_empty() {
            return self.list_shares(authority).await;
        }

        let (hostname, share, dir_path) = parse_smb_path(authority, path)?;

        // Get credentials
        let creds = get_server_credentials(&hostname)?;

        // Build connection
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
            .map_err(|e| format!("Failed to connect to SMB server: {}", e))?;

        // List directory
        let entries = client
            .list_dir(&dir_path)
            .map_err(|e| format!("Failed to list directory: {}", e))?;

        let mut items: Vec<FileItem> = Vec::new();

        for entry in entries {
            let name = entry.name();

            // Skip . and ..
            if name == "." || name == ".." {
                continue;
            }

            let full_path = if dir_path == "/" {
                format!("smb://{}/{}/{}", hostname, share, name)
            } else {
                format!("smb://{}/{}{}/{}", hostname, share, dir_path, name)
            };

            // Get file stats if available
            let entry_path = if dir_path == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", dir_path, name)
            };

            let (is_directory, size, modified) = match client.stat(&entry_path) {
                Ok(stat) => {
                    let is_dir = stat.mode.is_dir();
                    let file_size = stat.size;
                    let mtime: DateTime<Utc> = stat.modified.into();
                    (is_dir, file_size, mtime)
                }
                Err(_) => {
                    // Fallback: try to detect directory by listing it
                    let is_dir = client.list_dir(&entry_path).is_ok();
                    (is_dir, 0, Utc::now())
                }
            };

            items.push(FileItem {
                name: name.to_string(),
                path: full_path,
                is_directory,
                is_hidden: name.starts_with('.'),
                size,
                modified,
                extension: if is_directory {
                    None
                } else {
                    std::path::Path::new(name)
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
            });
        }

        // Sort: directories first, then by name
        items.sort_by(|a, b| match (a.is_directory, b.is_directory) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });

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
            is_hidden: name.starts_with('.'),
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
        use pavao::{SmbClient, SmbCredentials, SmbOptions};

        let creds = get_server_credentials(hostname)?;

        let smb_url = format!("smb://{}", hostname);

        let mut credentials = SmbCredentials::default()
            .server(&smb_url)
            .share("/")
            .username(&creds.username)
            .password(&creds.password);

        if let Some(domain) = &creds.domain {
            credentials = credentials.workgroup(domain);
        }

        let client = SmbClient::new(credentials, SmbOptions::default())
            .map_err(|e| format!("Failed to connect: {}", e))?;

        // List shares (directories at the root)
        let entries = client
            .list_dir("/")
            .map_err(|e| format!("Failed to enumerate shares: {}", e))?;

        let items: Vec<FileItem> = entries
            .into_iter()
            .filter_map(|entry| {
                let name = entry.name();
                // Skip hidden shares (ending in $) and special entries
                if name == "." || name == ".." || name.ends_with('$') {
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

#[cfg(test)]
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
