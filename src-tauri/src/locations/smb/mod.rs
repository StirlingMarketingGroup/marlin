mod auth;
pub mod client;

use async_trait::async_trait;
use crate::fs_utils::FileItem;
use crate::locations::{
    Location, LocationCapabilities, LocationProvider, LocationSummary, ProviderDirectoryEntries,
};
use chrono::{DateTime, Utc};

pub use auth::{
    add_smb_server, get_smb_servers, remove_smb_server, test_smb_connection,
    SmbServerInfo,
};
pub use auth::get_server_credentials;
pub use client::SidecarStatus;

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

    async fn read_directory(
        &self,
        location: &Location,
    ) -> Result<ProviderDirectoryEntries, String> {
        // Check sidecar availability
        if !client::is_available() {
            let status = client::initialize();
            if status != SidecarStatus::Available {
                return Err(status.error_message().unwrap_or_else(|| {
                    "SMB support is not available".to_string()
                }));
            }
        }

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

        // Build sidecar request params
        let params = serde_json::json!({
            "credentials": {
                "hostname": hostname,
                "username": creds.username,
                "password": creds.password,
                "domain": creds.domain
            },
            "share": share,
            "path": dir_path
        });

        // Call sidecar
        let result: serde_json::Value = tokio::task::spawn_blocking(move || {
            client::call_method::<serde_json::Value, serde_json::Value>("read_directory", params)
        })
        .await
        .map_err(|e| format!("SMB task failed: {}", e))??;

        // Parse result
        let entries = result
            .get("entries")
            .and_then(|e| e.as_array())
            .ok_or("Invalid response from sidecar")?;

        let mut items: Vec<FileItem> = Vec::new();

        for entry in entries {
            let name = entry.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let is_directory = entry.get("is_directory").and_then(|d| d.as_bool()).unwrap_or(false);
            let is_hidden = entry.get("is_hidden").and_then(|h| h.as_bool()).unwrap_or(false);
            let size = entry.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
            let modified_str = entry.get("modified").and_then(|m| m.as_str()).unwrap_or("");
            let extension = entry.get("extension").and_then(|e| e.as_str()).map(String::from);

            // Parse modified time
            let modified: DateTime<Utc> = modified_str
                .parse()
                .unwrap_or_else(|_| Utc::now());

            let full_path = if dir_path == "/" {
                format!("smb://{}/{}/{}", hostname, share, name)
            } else {
                format!("smb://{}/{}{}/{}", hostname, share, dir_path, name)
            };

            items.push(FileItem {
                name: name.to_string(),
                path: full_path,
                is_directory,
                is_hidden,
                size,
                modified,
                extension,
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

    async fn get_file_metadata(&self, location: &Location) -> Result<FileItem, String> {
        // Check sidecar availability
        if !client::is_available() {
            let status = client::initialize();
            if status != SidecarStatus::Available {
                return Err(status.error_message().unwrap_or_else(|| {
                    "SMB support is not available".to_string()
                }));
            }
        }

        let authority = location
            .authority()
            .ok_or_else(|| "SMB path requires server".to_string())?;

        // Server root (share listing)
        let location_path = location.path();
        if location_path == "/" || location_path.is_empty() {
            return Ok(FileItem {
                name: authority.to_string(),
                path: location.raw().to_string(),
                is_directory: true,
                is_hidden: false,
                is_symlink: false,
                is_git_repo: false,
                size: 0,
                modified: Utc::now(),
                extension: None,
                child_count: None,
                image_width: None,
                image_height: None,
                remote_id: None,
                thumbnail_url: None,
                download_url: None,
            });
        }

        let (hostname, share, path) = parse_smb_path(authority, location.path())?;
        let creds = get_server_credentials(&hostname)?;
        let location_raw = location.raw().to_string();

        let params = serde_json::json!({
            "credentials": {
                "hostname": hostname,
                "username": creds.username,
                "password": creds.password,
                "domain": creds.domain
            },
            "share": share,
            "path": path
        });

        let result: serde_json::Value = tokio::task::spawn_blocking(move || {
            client::call_method::<serde_json::Value, serde_json::Value>("get_file_metadata", params)
        })
        .await
        .map_err(|e| format!("SMB task failed: {}", e))??;

        let name = result.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
        let is_directory = result.get("is_directory").and_then(|d| d.as_bool()).unwrap_or(false);
        let is_hidden = result.get("is_hidden").and_then(|h| h.as_bool()).unwrap_or(false);
        let size = result.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
        let modified_str = result.get("modified").and_then(|m| m.as_str()).unwrap_or("");
        let extension = result.get("extension").and_then(|e| e.as_str()).map(String::from);

        let modified: DateTime<Utc> = modified_str
            .parse()
            .unwrap_or_else(|_| Utc::now());

        Ok(FileItem {
            name,
            path: location_raw,
            is_directory,
            is_hidden,
            size,
            modified,
            extension,
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

    async fn create_directory(&self, location: &Location) -> Result<(), String> {
        if !client::is_available() {
            let status = client::initialize();
            if status != SidecarStatus::Available {
                return Err(status.error_message().unwrap_or_else(|| {
                    "SMB support is not available".to_string()
                }));
            }
        }

        let authority = location
            .authority()
            .ok_or_else(|| "SMB path requires server".to_string())?;

        let (hostname, share, path) = parse_smb_path(authority, location.path())?;
        let creds = get_server_credentials(&hostname)?;

        let params = serde_json::json!({
            "credentials": {
                "hostname": hostname,
                "username": creds.username,
                "password": creds.password,
                "domain": creds.domain
            },
            "share": share,
            "path": path
        });

        tokio::task::spawn_blocking(move || {
            client::call_method::<serde_json::Value, serde_json::Value>("create_directory", params)
        })
        .await
        .map_err(|e| format!("SMB task failed: {}", e))??;

        Ok(())
    }

    async fn delete(&self, location: &Location) -> Result<(), String> {
        if !client::is_available() {
            let status = client::initialize();
            if status != SidecarStatus::Available {
                return Err(status.error_message().unwrap_or_else(|| {
                    "SMB support is not available".to_string()
                }));
            }
        }

        let authority = location
            .authority()
            .ok_or_else(|| "SMB path requires server".to_string())?;

        let (hostname, share, path) = parse_smb_path(authority, location.path())?;
        let creds = get_server_credentials(&hostname)?;

        let params = serde_json::json!({
            "credentials": {
                "hostname": hostname,
                "username": creds.username,
                "password": creds.password,
                "domain": creds.domain
            },
            "share": share,
            "path": path
        });

        tokio::task::spawn_blocking(move || {
            client::call_method::<serde_json::Value, serde_json::Value>("delete", params)
        })
        .await
        .map_err(|e| format!("SMB task failed: {}", e))??;

        Ok(())
    }

    async fn rename(&self, from: &Location, to: &Location) -> Result<(), String> {
        if !client::is_available() {
            let status = client::initialize();
            if status != SidecarStatus::Available {
                return Err(status.error_message().unwrap_or_else(|| {
                    "SMB support is not available".to_string()
                }));
            }
        }

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

        let params = serde_json::json!({
            "credentials": {
                "hostname": hostname,
                "username": creds.username,
                "password": creds.password,
                "domain": creds.domain
            },
            "share": share,
            "from_path": from_path,
            "to_path": to_path
        });

        tokio::task::spawn_blocking(move || {
            client::call_method::<serde_json::Value, serde_json::Value>("rename", params)
        })
        .await
        .map_err(|e| format!("SMB task failed: {}", e))??;

        Ok(())
    }

    async fn copy(&self, from: &Location, to: &Location) -> Result<(), String> {
        if !client::is_available() {
            let status = client::initialize();
            if status != SidecarStatus::Available {
                return Err(status.error_message().unwrap_or_else(|| {
                    "SMB support is not available".to_string()
                }));
            }
        }

        let from_authority = from
            .authority()
            .ok_or_else(|| "SMB path requires server".to_string())?;
        let to_authority = to
            .authority()
            .ok_or_else(|| "SMB path requires server".to_string())?;

        if from_authority != to_authority {
            return Err("Copying across different SMB servers is not supported".to_string());
        }

        let (hostname, share, from_path) = parse_smb_path(from_authority, from.path())?;
        let (_, to_share, to_path) = parse_smb_path(to_authority, to.path())?;

        if share != to_share {
            return Err("Copying across different SMB shares is not supported".to_string());
        }

        let creds = get_server_credentials(&hostname)?;

        let params = serde_json::json!({
            "credentials": {
                "hostname": hostname,
                "username": creds.username,
                "password": creds.password,
                "domain": creds.domain
            },
            "share": share,
            "from_path": from_path,
            "to_path": to_path
        });

        tokio::task::spawn_blocking(move || {
            client::call_method::<serde_json::Value, serde_json::Value>("copy", params)
        })
        .await
        .map_err(|e| format!("SMB task failed: {}", e))??;

        Ok(())
    }
}

impl SmbProvider {
    async fn list_shares(&self, hostname: &str) -> Result<ProviderDirectoryEntries, String> {
        let creds = get_server_credentials(hostname)?;
        let hostname_clone = hostname.to_string();

        let params = serde_json::json!({
            "credentials": {
                "hostname": hostname_clone,
                "username": creds.username,
                "password": creds.password,
                "domain": creds.domain
            }
        });

        let result: serde_json::Value = tokio::task::spawn_blocking(move || {
            client::call_method::<serde_json::Value, serde_json::Value>("list_shares", params)
        })
        .await
        .map_err(|e| format!("SMB task failed: {}", e))??;

        let shares = result
            .get("shares")
            .and_then(|s| s.as_array())
            .ok_or("Invalid response from sidecar")?;

        let items: Vec<FileItem> = shares
            .iter()
            .filter_map(|share| {
                let name = share.get("name")?.as_str()?;
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
}

/// Parse an SMB path into (hostname, share, path)
pub(crate) fn parse_smb_path(authority: &str, path: &str) -> Result<(String, String, String), String> {
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

/// Parse a full SMB URL into (hostname, share, path)
pub fn parse_smb_url(url: &str) -> Result<(String, String, String), String> {
    // NOTE: Do not use `url::Url::parse` here.
    // Our `smb://` paths are *not* guaranteed to be RFC-3986 compliant URLs:
    // - SMB hostnames can contain underscores and other characters `url` rejects.
    // - Paths can contain characters that `url` would treat as query/fragment delimiters.
    //
    // The rest of the app treats these as "raw" locations (see `locations::Location`), so we
    // parse them leniently to match that behavior.
    let remainder = url
        .strip_prefix("smb://")
        .ok_or_else(|| format!("Invalid SMB URL scheme (expected smb://): {}", url))?;

    // Support (but do not require) smb://user:pass@server/share/path by stripping credentials.
    let (authority_part, path_part) = remainder
        .split_once('/')
        .ok_or_else(|| "SMB URL must include share name: smb://server/share/path".to_string())?;

    let hostname = authority_part
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority_part);

    if hostname.is_empty() {
        return Err("SMB URL requires server hostname".to_string());
    }

    // Normalize any accidental duplicate slashes at the start of the path portion.
    let path = format!("/{}", path_part.trim_start_matches('/'));

    parse_smb_path(hostname, &path)
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

/// Upload a local file to an SMB share via the sidecar, handling name collisions.
/// Returns the final filename used on the SMB share.
pub fn upload_file_to_smb(
    local_path: &std::path::Path,
    hostname: &str,
    share: &str,
    dest_dir: &str,
    preferred_name: &str,
) -> Result<String, String> {
    use client::SidecarStatus;
    use std::path::Path;

    // Ensure sidecar is running
    if !client::is_available() {
        let status = client::initialize();
        if status != SidecarStatus::Available {
            return Err(status.error_message().unwrap_or_else(|| {
                "SMB support is not available".to_string()
            }));
        }
    }

    let creds = get_server_credentials(hostname)?;

    // Split name into stem + extension for collision avoidance
    let p = Path::new(preferred_name);
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(preferred_name)
        .to_string();
    let ext = p.extension().and_then(|e| e.to_str()).map(|s| s.to_string());

    for i in 1..1000usize {
        let candidate = if i == 1 {
            preferred_name.to_string()
        } else if let Some(ref e) = ext {
            format!("{stem} ({i}).{e}")
        } else {
            format!("{stem} ({i})")
        };

        let dest_rel = if dest_dir == "/" {
            format!("/{}", candidate)
        } else {
            format!("{}/{}", dest_dir.trim_end_matches('/'), candidate)
        };

        let params = serde_json::json!({
            "credentials": {
                "hostname": hostname,
                "username": creds.username,
                "password": creds.password,
                "domain": creds.domain
            },
            "share": share,
            "source_path": local_path.to_string_lossy(),
            "dest_path": dest_rel
        });

        match client::call_method_with_timeout::<_, serde_json::Value>(
            "upload_file",
            params,
            client::DOWNLOAD_TIMEOUT_MS,
        ) {
            Ok(_) => return Ok(candidate),
            Err(e) => {
                // If the file already exists, try the next name
                let lower = e.to_lowercase();
                if lower.contains("exist") || lower.contains("eexist") {
                    continue;
                }
                return Err(e);
            }
        }
    }

    Err("Unable to allocate unique destination name on SMB share".to_string())
}

/// Download an SMB file to a local path via the sidecar.
pub fn download_file_from_smb(
    hostname: &str,
    share: &str,
    file_path: &str,
    dest_path: &std::path::Path,
) -> Result<(), String> {
    use client::SidecarStatus;

    if !client::is_available() {
        let status = client::initialize();
        if status != SidecarStatus::Available {
            return Err(status.error_message().unwrap_or_else(|| {
                "SMB support is not available".to_string()
            }));
        }
    }

    let creds = get_server_credentials(hostname)?;

    let params = serde_json::json!({
        "credentials": {
            "hostname": hostname,
            "username": creds.username,
            "password": creds.password,
            "domain": creds.domain
        },
        "share": share,
        "path": file_path,
        "dest_path": dest_path.to_string_lossy()
    });

    let _result: serde_json::Value = client::call_method_with_timeout(
        "download_file",
        params,
        client::DOWNLOAD_TIMEOUT_MS,
    )?;

    Ok(())
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
    fn test_parse_smb_url_lenient_hostname() {
        let (host, share, path) = parse_smb_url("smb://nas_1/myshare/folder/file name.jpg").unwrap();
        assert_eq!(host, "nas_1");
        assert_eq!(share, "myshare");
        assert_eq!(path, "/folder/file name.jpg");
    }

    #[test]
    fn test_parse_smb_url_strips_credentials() {
        let (host, share, path) =
            parse_smb_url("smb://user:password@server.local/share/folder/file.txt").unwrap();
        assert_eq!(host, "server.local");
        assert_eq!(share, "share");
        assert_eq!(path, "/folder/file.txt");
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
