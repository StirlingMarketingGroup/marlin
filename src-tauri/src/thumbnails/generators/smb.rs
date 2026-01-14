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
    use std::io::{Read, Write};

    log::debug!("download_smb_file_sync: path={}", smb_path);

    // Parse the SMB URL
    let (hostname, share, file_path) = parse_smb_url(smb_path)?;

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

    // Read the entire file content using std::io::Read trait
    let mut content = Vec::new();
    smb_file
        .read_to_end(&mut content)
        .map_err(|e| format!("Failed to read SMB file: {}", e))?;

    // Write to temp file
    let mut local_file = std::fs::File::create(&temp_path)
        .map_err(|e| format!("Failed to create temp file: {}", e))?;

    local_file
        .write_all(&content)
        .map_err(|e| format!("Failed to write temp file: {}", e))?;

    log::debug!(
        "Downloaded {} bytes from SMB to {}",
        content.len(),
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
pub fn get_smb_file_mtime(smb_path: &str) -> u64 {
    use pavao::{SmbClient, SmbCredentials, SmbOptions};
    use std::time::UNIX_EPOCH;

    // Parse the SMB URL
    let (hostname, share, file_path) = match parse_smb_url(smb_path) {
        Ok(result) => result,
        Err(_) => return 0,
    };

    // Get credentials for this server
    let creds = match crate::locations::smb::get_server_credentials(&hostname) {
        Ok(c) => c,
        Err(_) => return 0,
    };

    // Acquire global SMB mutex
    let _guard = match crate::locations::smb::SMB_MUTEX.lock() {
        Ok(g) => g,
        Err(_) => return 0,
    };

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

    let client = match SmbClient::new(credentials, SmbOptions::default()) {
        Ok(c) => c,
        Err(_) => return 0,
    };

    match client.stat(&file_path) {
        Ok(stat) => {
            let system_time: std::time::SystemTime = stat.modified.into();
            system_time
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        }
        Err(_) => 0,
    }
}

/// Stub implementation when SMB feature is disabled
#[cfg(not(feature = "smb"))]
pub fn get_smb_file_mtime(_smb_path: &str) -> u64 {
    0
}

/// Parse an SMB URL into (hostname, share, path) components.
#[cfg(feature = "smb")]
fn parse_smb_url(url: &str) -> Result<(String, String, String), String> {
    let without_scheme = url
        .strip_prefix("smb://")
        .ok_or_else(|| format!("Invalid SMB URL (must start with smb://): {}", url))?;

    // Split into authority and path
    let (authority, path) = match without_scheme.find('/') {
        Some(idx) => (&without_scheme[..idx], &without_scheme[idx..]),
        None => (without_scheme, "/"),
    };

    // Authority might have credentials (user:pass@server) - strip them
    let hostname = if let Some(at_pos) = authority.rfind('@') {
        authority[at_pos + 1..].to_string()
    } else {
        authority.to_string()
    };

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
        return Err(format!("SMB URL must include share name: {}", url));
    }

    Ok((hostname, share, file_path))
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
