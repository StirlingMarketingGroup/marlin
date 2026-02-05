pub mod auth;
pub mod pool;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use crate::fs_utils::FileItem;
use crate::locations::{
    Location, LocationCapabilities, LocationProvider, LocationSummary, ProviderDirectoryEntries,
};

pub use auth::{
    add_sftp_server, get_sftp_servers, remove_sftp_server, SftpServerInfo,
};

#[derive(Default)]
pub struct SftpProvider;

#[async_trait]
impl LocationProvider for SftpProvider {
    fn scheme(&self) -> &'static str {
        "sftp"
    }

    fn capabilities(&self, _location: &Location) -> LocationCapabilities {
        LocationCapabilities::new("sftp", "SFTP Server", true, true)
    }

    async fn read_directory(
        &self,
        location: &Location,
    ) -> Result<ProviderDirectoryEntries, String> {
        let authority = location
            .authority()
            .ok_or_else(|| "SFTP path requires server: sftp://user@host:port/path".to_string())?;

        let (username, hostname, port) = parse_sftp_authority(authority)?;
        let path = location.path().to_string();
        let remote_path = if path.is_empty() || path == "/" { "/" } else { &path };

        let sftp = pool::get_sftp_session(&hostname, port).await?;

        let entries = sftp
            .read_dir(remote_path)
            .await
            .map_err(|e| format!("Failed to read directory: {}", e))?;

        let mut items: Vec<FileItem> = Vec::new();

        for entry in entries {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }

            let is_directory = entry.file_type().is_dir();
            let is_hidden = name.starts_with('.');
            let is_symlink = entry.file_type().is_symlink();
            let size = entry.metadata().len();
            let mtime = entry.metadata().mtime.unwrap_or(0);
            let modified = Utc.timestamp_opt(mtime as i64, 0).single().unwrap_or_else(Utc::now);

            let extension = if !is_directory {
                std::path::Path::new(&name)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(String::from)
            } else {
                None
            };

            let full_path = if remote_path == "/" {
                format!("sftp://{}@{}:{}/{}", username, hostname, port, name)
            } else {
                format!(
                    "sftp://{}@{}:{}{}/{}",
                    username, hostname, port, remote_path, name
                )
            };

            items.push(FileItem {
                name,
                path: full_path,
                is_directory,
                is_hidden,
                size,
                modified,
                extension,
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

        items.sort_by(|a, b| match (a.is_directory, b.is_directory) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });

        let display_path = if port == 22 {
            format!("sftp://{}@{}{}", username, hostname, remote_path)
        } else {
            format!("sftp://{}@{}:{}{}", username, hostname, port, remote_path)
        };

        Ok(ProviderDirectoryEntries {
            location: LocationSummary::new(
                "sftp",
                Some(format!("{}@{}:{}", username, hostname, port)),
                remote_path,
                display_path,
            ),
            entries: items,
        })
    }

    async fn get_file_metadata(&self, location: &Location) -> Result<FileItem, String> {
        let authority = location
            .authority()
            .ok_or_else(|| "SFTP path requires server".to_string())?;

        let (username, hostname, port) = parse_sftp_authority(authority)?;
        let remote_path = location.path().to_string();

        if remote_path == "/" || remote_path.is_empty() {
            return Ok(FileItem {
                name: if port == 22 {
                    format!("{}@{}", username, hostname)
                } else {
                    format!("{}@{}:{}", username, hostname, port)
                },
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

        let sftp = pool::get_sftp_session(&hostname, port).await?;

        let attrs = sftp
            .metadata(&remote_path)
            .await
            .map_err(|e| format!("Failed to get metadata: {}", e))?;

        let name = std::path::Path::new(&remote_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let is_directory = attrs.is_dir();
        let size = attrs.len();
        let mtime = attrs.mtime.unwrap_or(0);
        let modified = Utc.timestamp_opt(mtime as i64, 0).single().unwrap_or_else(Utc::now);

        let extension = if !is_directory {
            std::path::Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .map(String::from)
        } else {
            None
        };

        Ok(FileItem {
            name,
            path: location.raw().to_string(),
            is_directory,
            is_hidden: remote_path
                .rsplit('/')
                .next()
                .map_or(false, |n| n.starts_with('.')),
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
        let authority = location
            .authority()
            .ok_or_else(|| "SFTP path requires server".to_string())?;
        let (_username, hostname, port) = parse_sftp_authority(authority)?;
        let remote_path = location.path().to_string();

        let sftp = pool::get_sftp_session(&hostname, port).await?;
        sftp.create_dir(&remote_path)
            .await
            .map_err(|e| format!("Failed to create directory: {}", e))?;

        Ok(())
    }

    async fn delete(&self, location: &Location) -> Result<(), String> {
        let authority = location
            .authority()
            .ok_or_else(|| "SFTP path requires server".to_string())?;
        let (_username, hostname, port) = parse_sftp_authority(authority)?;
        let remote_path = location.path().to_string();

        let sftp = pool::get_sftp_session(&hostname, port).await?;

        let attrs = sftp
            .metadata(&remote_path)
            .await
            .map_err(|e| format!("Failed to stat path for deletion: {}", e))?;

        if attrs.is_dir() {
            recursive_delete(&sftp, &remote_path).await?;
        } else {
            sftp.remove_file(&remote_path)
                .await
                .map_err(|e| format!("Failed to remove file: {}", e))?;
        }

        Ok(())
    }

    async fn rename(&self, from: &Location, to: &Location) -> Result<(), String> {
        let from_authority = from
            .authority()
            .ok_or_else(|| "SFTP path requires server".to_string())?;
        let to_authority = to
            .authority()
            .ok_or_else(|| "SFTP path requires server".to_string())?;

        let (_from_user, from_host, from_port) = parse_sftp_authority(from_authority)?;
        let (_to_user, to_host, to_port) = parse_sftp_authority(to_authority)?;

        if from_host != to_host || from_port != to_port {
            return Err("Cannot rename across different servers".to_string());
        }

        let sftp = pool::get_sftp_session(&from_host, from_port).await?;
        sftp.rename(from.path(), to.path())
            .await
            .map_err(|e| format!("Failed to rename: {}", e))?;

        Ok(())
    }

    async fn copy(&self, from: &Location, to: &Location) -> Result<(), String> {
        let from_authority = from
            .authority()
            .ok_or_else(|| "SFTP path requires server".to_string())?;
        let to_authority = to
            .authority()
            .ok_or_else(|| "SFTP path requires server".to_string())?;

        let (_from_user, from_host, from_port) = parse_sftp_authority(from_authority)?;
        let (_to_user, to_host, to_port) = parse_sftp_authority(to_authority)?;

        if from_host != to_host || from_port != to_port {
            return Err("Copying across different SFTP servers is not supported".to_string());
        }

        // SFTP has no server-side copy; download into memory and re-upload
        let sftp = pool::get_sftp_session(&from_host, from_port).await?;

        let data = sftp
            .read(from.path())
            .await
            .map_err(|e| format!("Failed to read source file: {}", e))?;

        sftp.write(to.path(), &data)
            .await
            .map_err(|e| format!("Failed to write destination file: {}", e))?;

        Ok(())
    }
}

/// Recursively delete a directory and all its contents.
async fn recursive_delete(sftp: &russh_sftp::client::SftpSession, path: &str) -> Result<(), String> {
    let entries = sftp
        .read_dir(path)
        .await
        .map_err(|e| format!("Failed to list directory for deletion: {}", e))?;

    for entry in entries {
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }

        let child_path = if path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", path.trim_end_matches('/'), name)
        };

        if entry.file_type().is_dir() {
            Box::pin(recursive_delete(sftp, &child_path)).await?;
        } else {
            sftp.remove_file(&child_path)
                .await
                .map_err(|e| format!("Failed to delete file {}: {}", child_path, e))?;
        }
    }

    sftp.remove_dir(path)
        .await
        .map_err(|e| format!("Failed to remove directory {}: {}", path, e))?;

    Ok(())
}

/// Parse authority part: "user@host:port" -> (user, host, port)
pub fn parse_sftp_authority(authority: &str) -> Result<(String, String, u16), String> {
    let (user_part, host_part) = authority
        .rsplit_once('@')
        .ok_or_else(|| format!("SFTP authority must include username: user@host (got '{}')", authority))?;

    let username = user_part.to_string();
    if username.is_empty() {
        return Err("SFTP username cannot be empty".to_string());
    }

    let (hostname, port) = if let Some((h, p)) = host_part.rsplit_once(':') {
        let port: u16 = p
            .parse()
            .map_err(|_| format!("Invalid port number: {}", p))?;
        (h.to_string(), port)
    } else {
        (host_part.to_string(), 22)
    };

    if hostname.is_empty() {
        return Err("SFTP hostname cannot be empty".to_string());
    }

    Ok((username, hostname, port))
}

/// Parse a full SFTP URL: "sftp://user@host:port/path" -> (user, host, port, path)
pub fn parse_sftp_url(url: &str) -> Result<(String, String, u16, String), String> {
    let remainder = url
        .strip_prefix("sftp://")
        .ok_or_else(|| format!("Invalid SFTP URL scheme (expected sftp://): {}", url))?;

    let (authority_part, path_part) = if let Some(slash_idx) = remainder.find('/') {
        let (auth, path) = remainder.split_at(slash_idx);
        (auth, path.to_string())
    } else {
        (remainder, "/".to_string())
    };

    let (username, hostname, port) = parse_sftp_authority(authority_part)?;

    let path = if path_part.is_empty() {
        "/".to_string()
    } else {
        path_part
    };

    Ok((username, hostname, port, path))
}

/// Download a file from SFTP to a local path.
/// Acquires a concurrency permit to avoid overwhelming the server.
pub async fn download_file_from_sftp(
    hostname: &str,
    port: u16,
    remote_path: &str,
    dest: &std::path::Path,
) -> Result<(), String> {
    let sftp = pool::get_sftp_session(hostname, port).await?;
    let _permit = pool::acquire_permit(hostname, port).await?;

    let data = sftp
        .read(remote_path)
        .await
        .map_err(|e| format!("Failed to download file: {}", e))?;

    tokio::fs::write(dest, &data)
        .await
        .map_err(|e| format!("Failed to write downloaded file: {}", e))?;

    Ok(())
}

/// Upload a local file to SFTP, handling name collisions.
/// Returns the final filename used on the server.
pub async fn upload_file_to_sftp(
    local_path: &std::path::Path,
    hostname: &str,
    port: u16,
    dest_dir: &str,
    preferred_name: &str,
) -> Result<String, String> {
    let sftp = pool::get_sftp_session(hostname, port).await?;

    let data = tokio::fs::read(local_path)
        .await
        .map_err(|e| format!("Failed to read local file: {}", e))?;

    let p = std::path::Path::new(preferred_name);
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(preferred_name)
        .to_string();
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_string());

    for i in 1..1000usize {
        let candidate = if i == 1 {
            preferred_name.to_string()
        } else if let Some(ref e) = ext {
            format!("{stem} ({i}).{e}")
        } else {
            format!("{stem} ({i})")
        };

        let dest_path = if dest_dir == "/" {
            format!("/{}", candidate)
        } else {
            format!("{}/{}", dest_dir.trim_end_matches('/'), candidate)
        };

        // Check if file already exists
        match sftp.metadata(&dest_path).await {
            Ok(_) => continue, // File exists, try next name
            Err(_) => {
                // File doesn't exist, upload here
                sftp.write(&dest_path, &data)
                    .await
                    .map_err(|e| format!("Failed to upload file: {}", e))?;
                return Ok(candidate);
            }
        }
    }

    Err("Unable to allocate unique destination name on SFTP server".to_string())
}

/// Download an SFTP file to a temp location.
/// Returns the temporary file path.
pub async fn download_sftp_file_to_temp(sftp_url: &str) -> Result<std::path::PathBuf, String> {
    use sha2::{Digest, Sha256};

    let (username, hostname, port, remote_path) = parse_sftp_url(sftp_url)?;

    let temp_dir = std::env::temp_dir().join("marlin-sftp-downloads");
    tokio::fs::create_dir_all(&temp_dir)
        .await
        .map_err(|e| format!("Failed to create temp directory: {}", e))?;

    let original_name = std::path::Path::new(&remote_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");

    let mut hasher = Sha256::new();
    hasher.update(username.as_bytes());
    hasher.update(b"@");
    hasher.update(hostname.as_bytes());
    hasher.update(b":");
    hasher.update(port.to_string().as_bytes());
    hasher.update(remote_path.as_bytes());
    let hash = hasher.finalize();
    let hash_prefix = format!("{:x}", hash)[..12].to_string();

    let safe_name = original_name
        .replace(['/', '\\', '\0', ':', '*', '?', '"', '<', '>', '|'], "_")
        .trim_start_matches('.')
        .to_string();

    let temp_path = temp_dir.join(format!("{}_{}", hash_prefix, safe_name));

    download_file_from_sftp(&hostname, port, &remote_path, &temp_path).await?;

    Ok(temp_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sftp_authority_basic() {
        let (user, host, port) = parse_sftp_authority("demo@test.rebex.net").unwrap();
        assert_eq!(user, "demo");
        assert_eq!(host, "test.rebex.net");
        assert_eq!(port, 22);
    }

    #[test]
    fn test_parse_sftp_authority_with_port() {
        let (user, host, port) = parse_sftp_authority("admin@192.168.1.1:2222").unwrap();
        assert_eq!(user, "admin");
        assert_eq!(host, "192.168.1.1");
        assert_eq!(port, 2222);
    }

    #[test]
    fn test_parse_sftp_authority_missing_user() {
        assert!(parse_sftp_authority("host.com").is_err());
    }

    #[test]
    fn test_parse_sftp_url_basic() {
        let (user, host, port, path) =
            parse_sftp_url("sftp://demo@test.rebex.net/pub/example/readme.txt").unwrap();
        assert_eq!(user, "demo");
        assert_eq!(host, "test.rebex.net");
        assert_eq!(port, 22);
        assert_eq!(path, "/pub/example/readme.txt");
    }

    #[test]
    fn test_parse_sftp_url_with_port() {
        let (user, host, port, path) =
            parse_sftp_url("sftp://admin@myserver:2222/home/admin/file.txt").unwrap();
        assert_eq!(user, "admin");
        assert_eq!(host, "myserver");
        assert_eq!(port, 2222);
        assert_eq!(path, "/home/admin/file.txt");
    }

    #[test]
    fn test_parse_sftp_url_root() {
        let (user, host, port, path) = parse_sftp_url("sftp://demo@host.com").unwrap();
        assert_eq!(user, "demo");
        assert_eq!(host, "host.com");
        assert_eq!(port, 22);
        assert_eq!(path, "/");
    }

    #[test]
    fn test_parse_sftp_url_no_scheme() {
        assert!(parse_sftp_url("user@host/path").is_err());
    }
}
