//! SMB thumbnail generation support
//!
//! This module handles downloading SMB files to a local temp directory before
//! generating thumbnails. Since the thumbnail generator runs in a blocking
//! thread pool, we use blocking SMB operations here.

use super::{ThumbnailGenerationResult, ThumbnailRequest};

/// Check if the path is an SMB URL
pub fn is_smb_path(path: &str) -> bool {
    path.starts_with("smb://")
}

/// Download an SMB file synchronously and return the local temp path.
/// This is designed to be called from a blocking context (thread pool).
#[cfg(feature = "smb")]
pub fn download_smb_file_sync(smb_path: &str) -> Result<std::path::PathBuf, String> {
    use pavao::{SmbClient, SmbCredentials, SmbOpenOptions, SmbOptions};
    use sha2::{Digest, Sha256};
    use std::io::Write;

    log::debug!("download_smb_file_sync: path={}", smb_path);

    // Parse the SMB URL
    let (hostname, share, file_path) = crate::locations::smb::parse_smb_url(smb_path)?;

    // Get credentials for this server
    let creds = crate::locations::smb::get_server_credentials(&hostname)?;

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

    // Acquire global SMB mutex - libsmbclient has global state
    let _guard = crate::locations::smb::SMB_MUTEX
        .lock()
        .map_err(|e| format!("SMB mutex poisoned: {}", e))?;

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

    // Open the remote file for reading
    let mut smb_file = client
        .open_with(&file_path, SmbOpenOptions::default().read(true))
        .map_err(|e| format!("Failed to open SMB file: {}", e))?;

    // Write to temp file (streaming; avoids loading whole file into memory)
    let mut local_file = std::fs::File::create(&temp_path)
        .map_err(|e| format!("Failed to create temp file: {}", e))?;

    std::io::copy(&mut smb_file, &mut local_file)
        .map_err(|e| format!("Failed to copy SMB file to temp: {}", e))?;

    local_file
        .flush()
        .map_err(|e| format!("Failed to flush temp file: {}", e))?;

    log::debug!(
        "Downloaded SMB file to {}",
        temp_path.display()
    );

    Ok(temp_path)
}

/// Stub implementation when SMB feature is disabled
#[cfg(not(feature = "smb"))]
pub fn download_smb_file_sync(_smb_path: &str) -> Result<std::path::PathBuf, String> {
    Err("SMB support not compiled. Build with --features smb".to_string())
}

/// Get the modified time for an SMB file.
/// Returns 0 if the mtime cannot be determined.
#[cfg(feature = "smb")]
pub fn get_smb_file_mtime(smb_path: &str) -> Result<u64, String> {
    use pavao::{SmbClient, SmbCredentials, SmbOptions};
    use std::time::UNIX_EPOCH;

    // Parse the SMB URL
    let (hostname, share, file_path) = crate::locations::smb::parse_smb_url(smb_path)?;

    // Get credentials for this server
    let creds = crate::locations::smb::get_server_credentials(&hostname)?;

    // Acquire global SMB mutex
    let _guard = crate::locations::smb::SMB_MUTEX
        .lock()
        .map_err(|e| format!("SMB mutex poisoned: {}", e))?;

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

    let stat = client
        .stat(&file_path)
        .map_err(|e| format!("Failed to stat SMB file: {}", e))?;

    let system_time: std::time::SystemTime = stat.modified.into();
    Ok(system_time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0))
}

/// Stub implementation when SMB feature is disabled
#[cfg(not(feature = "smb"))]
pub fn get_smb_file_mtime(_smb_path: &str) -> Result<u64, String> {
    Err("SMB support not compiled. Build with --features smb".to_string())
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
