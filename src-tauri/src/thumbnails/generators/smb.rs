//! SMB thumbnail generation support
//!
//! This module handles downloading SMB files to a local temp directory before
//! generating thumbnails. Since the thumbnail generator runs in a blocking
//! thread pool, we use blocking SMB operations here via the sidecar.

use super::{ThumbnailGenerationResult, ThumbnailRequest};

/// Check if the path is an SMB URL
pub fn is_smb_path(path: &str) -> bool {
    path.starts_with("smb://")
}

/// Download an SMB file synchronously and return the local temp path.
/// This is designed to be called from a blocking context (thread pool).
pub fn download_smb_file_sync(smb_path: &str) -> Result<std::path::PathBuf, String> {
    use crate::locations::smb::{client, parse_smb_url, get_server_credentials, SidecarStatus};
    use sha2::{Digest, Sha256};

    log::debug!("download_smb_file_sync: path={}", smb_path);

    // Check sidecar availability
    if !client::is_available() {
        let status = client::initialize();
        if status != SidecarStatus::Available {
            return Err(status.error_message().unwrap_or_else(|| {
                "SMB support is not available".to_string()
            }));
        }
    }

    // Parse the SMB URL
    let (hostname, share, file_path) = parse_smb_url(smb_path)?;

    // Get credentials for this server
    let creds = get_server_credentials(&hostname)?;

    // Create a unique temp file path
    let temp_dir = std::env::temp_dir().join("marlin-smb-thumbnails");
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("Failed to create temp directory: {}", e))?;

    // Generate a safe filename from the path
    let original_name = std::path::Path::new(&file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");

    let mut hasher = Sha256::new();
    hasher.update(hostname.as_bytes());
    hasher.update(b"/");
    hasher.update(share.as_bytes());
    hasher.update(file_path.as_bytes());
    let hash = hasher.finalize();
    let hash_prefix = format!("{:x}", hash)[..12].to_string();

    let safe_name = original_name
        .replace(['/', '\\', '\0', ':', '*', '?', '"', '<', '>', '|'], "_")
        .trim_start_matches('.')
        .to_string();

    let temp_path = temp_dir.join(format!("{}_{}", hash_prefix, safe_name));
    let temp_path_str = temp_path.to_string_lossy().to_string();

    // Build sidecar request params
    let params = serde_json::json!({
        "credentials": {
            "hostname": hostname,
            "username": creds.username,
            "password": creds.password,
            "domain": creds.domain
        },
        "share": share,
        "path": file_path,
        "dest_path": temp_path_str
    });

    // Call sidecar with extended timeout for file downloads
    let _result: serde_json::Value = client::call_method_with_timeout(
        "download_file",
        params,
        client::DOWNLOAD_TIMEOUT_MS,
    )?;

    log::debug!(
        "Downloaded SMB file to {}",
        temp_path.display()
    );

    Ok(temp_path)
}

/// Get file identity for an SMB file including size and mtime.
/// Returns FileIdentity with nanosecond mtime precision.
pub fn get_smb_file_identity(smb_path: &str) -> Result<super::super::FileIdentity, String> {
    use crate::locations::smb::{client, parse_smb_url, get_server_credentials, SidecarStatus};
    use chrono::{DateTime, Utc};

    // Check sidecar availability
    if !client::is_available() {
        let status = client::initialize();
        if status != SidecarStatus::Available {
            return Err(status.error_message().unwrap_or_else(|| {
                "SMB support is not available".to_string()
            }));
        }
    }

    // Parse the SMB URL
    let (hostname, share, file_path) = parse_smb_url(smb_path)?;

    // Get credentials for this server
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
        "path": file_path
    });

    // Call sidecar
    let result: serde_json::Value = client::call_method("get_file_metadata", params)?;

    // Parse the modified time from the response
    let modified_str = result
        .get("modified")
        .and_then(|m| m.as_str())
        .ok_or("Missing modified time in response")?;

    // Parse ISO 8601 timestamp
    let modified: DateTime<Utc> = modified_str
        .parse()
        .map_err(|e| format!("Failed to parse modified time: {}", e))?;

    // Handle negative timestamps (dates before 1970) gracefully
    let mtime_secs = modified.timestamp().max(0) as u64;
    // Convert seconds to nanoseconds for consistency with local files
    let mtime_ns = (mtime_secs as u128) * 1_000_000_000;

    // Try to get file size from the response
    let size = result
        .get("size")
        .and_then(|s| s.as_u64())
        .unwrap_or(0);

    Ok(super::super::FileIdentity {
        size,
        mtime_ns,
        file_id: None, // SMB doesn't provide inode-like identifiers
    })
}

/// Generate a thumbnail for an SMB file.
/// Downloads the file to a temp location first, then generates the thumbnail.
pub fn generate_smb_thumbnail(request: &ThumbnailRequest) -> Result<ThumbnailGenerationResult, String> {
    // Download the file to a local temp path
    let temp_path = download_smb_file_sync(&request.path)?;

    // Create a modified request with the local path
    let local_request = ThumbnailRequest {
        id: request.id.clone(),
        path: temp_path.to_string_lossy().to_string(),
        size: request.size,
        quality: request.quality,
        priority: request.priority,
        format: request.format,
        accent: request.accent.clone(),
    };

    // Generate the thumbnail using the local file
    super::ThumbnailGenerator::generate_local(&local_request)
}
