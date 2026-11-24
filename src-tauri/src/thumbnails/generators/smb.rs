//! SMB thumbnail generation support
//!
//! This module handles downloading SMB files to a local temp directory before
//! generating thumbnails. Since the thumbnail generator runs in a blocking
//! thread pool, we use blocking SMB operations here via the sidecar.
//!
//! For video files, only the first ~10 MB is downloaded (partial download),
//! which is enough for FFmpeg to extract a frame. This prevents blocking
//! the SMB mutex for minutes/hours on large video files (100+ GiB).

use super::{is_video_extension, ThumbnailGenerationResult, ThumbnailRequest};
use marlin_smb_client::protocol::methods;

/// Maximum bytes to download for video thumbnail extraction.
/// 10 MB is enough for FFmpeg to find a keyframe in most container formats
/// (MP4/MOV with moov atom at start, MKV/WebM with headers first).
const VIDEO_PARTIAL_DOWNLOAD_BYTES: u64 = 10 * 1024 * 1024;

/// Maximum file size for full downloads of non-video files.
/// Files larger than this are skipped entirely (no thumbnail).
const MAX_FULL_DOWNLOAD_SIZE: u64 = 500 * 1024 * 1024;

/// Check if the path is an SMB URL
pub fn is_smb_path(path: &str) -> bool {
    path.starts_with("smb://")
}

/// Parse the SMB URL and load the saved credentials for its host.
/// Returns (share, file_path, credentials json).
fn prepare_smb_request(smb_path: &str) -> Result<(String, String, serde_json::Value), String> {
    use crate::locations::smb::{get_server_credentials, parse_smb_url};

    let (hostname, share, file_path) = parse_smb_url(smb_path)?;
    let creds = get_server_credentials(&hostname)?;

    let creds_json = serde_json::json!({
        "hostname": hostname,
        "username": creds.username,
        "password": creds.password,
        "domain": creds.domain
    });

    Ok((share, file_path, creds_json))
}

/// The temp path for a copy of an SMB file, in a new folder for each download, so two jobs for one file never share a copy.
fn smb_temp_path(file_path: &str) -> Result<std::path::PathBuf, String> {
    crate::locations::temp_copy_path(
        "marlin-smb-thumbnails",
        &uuid::Uuid::new_v4().simple().to_string(),
        file_path.rsplit('/').next().unwrap_or_default(),
    )
}

/// Clone a ThumbnailRequest with a different local file path.
fn request_with_local_path(
    request: &ThumbnailRequest,
    local_path: &std::path::Path,
) -> ThumbnailRequest {
    let mut r = request.clone();
    r.path = local_path.to_string_lossy().to_string();
    r
}

/// Download an SMB file synchronously and return the local temp path.
/// This is designed to be called from a blocking context (thread pool).
pub fn download_smb_file_sync(smb_path: &str) -> Result<std::path::PathBuf, String> {
    use crate::locations::smb::client;

    log::debug!("download_smb_file_sync: path={}", smb_path);

    let (share, file_path, creds_json) = prepare_smb_request(smb_path)?;
    let temp_path = smb_temp_path(&file_path)?;
    let temp_path_str = temp_path.to_string_lossy().to_string();

    let params = serde_json::json!({
        "credentials": creds_json,
        "share": share,
        "path": file_path,
        "dest_path": temp_path_str
    });

    // A failed download can leave part of the file behind.
    if let Err(error) = client::call_method::<_, serde_json::Value>(methods::DOWNLOAD_FILE, params)
    {
        let _ = crate::locations::remove_temp_copy(&temp_path);
        return Err(error);
    }

    log::debug!("Downloaded SMB file to {}", temp_path.display());

    Ok(temp_path)
}

/// Download up to `max_bytes` of an SMB file and return (temp_path, bytes_written, total_size).
fn download_smb_file_partial_sync(
    smb_path: &str,
    max_bytes: u64,
) -> Result<(std::path::PathBuf, u64, u64), String> {
    use crate::locations::smb::client;

    log::debug!(
        "download_smb_file_partial_sync: path={} max_bytes={}",
        smb_path,
        max_bytes
    );

    let (share, file_path, creds_json) = prepare_smb_request(smb_path)?;
    let temp_path = smb_temp_path(&file_path)?;
    let temp_path_str = temp_path.to_string_lossy().to_string();

    let params = serde_json::json!({
        "credentials": creds_json,
        "share": share,
        "path": file_path,
        "dest_path": temp_path_str,
        "max_bytes": max_bytes
    });

    // A failed download can leave part of the file behind.
    let result: serde_json::Value = match client::call_method(methods::DOWNLOAD_PARTIAL, params) {
        Ok(result) => result,
        Err(error) => {
            let _ = crate::locations::remove_temp_copy(&temp_path);
            return Err(error);
        }
    };

    let bytes_written = result
        .get("bytes_written")
        .and_then(|v| v.as_u64())
        .ok_or("Missing bytes_written in download_partial response")?;
    let total_size = result
        .get("total_size")
        .and_then(|v| v.as_u64())
        .ok_or("Missing total_size in download_partial response")?;

    log::debug!(
        "Partial download: {} bytes of {} total to {}",
        bytes_written,
        total_size,
        temp_path.display()
    );

    Ok((temp_path, bytes_written, total_size))
}

/// Get file identity for an SMB file including size and mtime.
/// Returns FileIdentity with nanosecond mtime precision.
pub fn get_smb_file_identity(smb_path: &str) -> Result<super::super::FileIdentity, String> {
    use crate::locations::smb::client;
    use chrono::{DateTime, Utc};

    let (share, file_path, creds_json) = prepare_smb_request(smb_path)?;

    let params = serde_json::json!({
        "credentials": creds_json,
        "share": share,
        "path": file_path
    });

    let result: serde_json::Value = client::call_method(methods::GET_FILE_METADATA, params)?;

    let modified_str = result
        .get("modified")
        .and_then(|m| m.as_str())
        .ok_or("Missing modified time in response")?;

    let modified: DateTime<Utc> = modified_str
        .parse()
        .map_err(|e| format!("Failed to parse modified time: {}", e))?;

    let mtime_secs = modified.timestamp().max(0) as u64;
    let mtime_ns = (mtime_secs as u128) * 1_000_000_000;

    let size = result
        .get("size")
        .and_then(|s| s.as_u64())
        .ok_or("Missing size in file metadata response")?;

    Ok(super::super::FileIdentity {
        size,
        mtime_ns,
        file_id: None,
    })
}

/// Check if an SMB path points to a video file based on its extension.
fn is_smb_video_path(smb_path: &str) -> bool {
    std::path::Path::new(smb_path)
        .extension()
        .and_then(|s| s.to_str())
        .map_or(false, is_video_extension)
}

/// Generate a thumbnail for an SMB file.
///
/// For video files: partial download (10 MB) -> FFmpeg. Falls back to full
/// download only if the file is < 500 MB.
///
/// For non-video files: checks size first, skips files > 500 MB, otherwise
/// does a full download and generates the thumbnail normally.
pub fn generate_smb_thumbnail(
    request: &ThumbnailRequest,
) -> Result<ThumbnailGenerationResult, String> {
    if is_smb_video_path(&request.path) {
        generate_smb_video_thumbnail(request)
    } else {
        generate_smb_nonvideo_thumbnail(request)
    }
}

/// Video thumbnail: partial download -> FFmpeg, with bounded fallback.
fn generate_smb_video_thumbnail(
    request: &ThumbnailRequest,
) -> Result<ThumbnailGenerationResult, String> {
    // Step 1: Partial download
    let (partial_path, _bytes_written, total_size) =
        download_smb_file_partial_sync(&request.path, VIDEO_PARTIAL_DOWNLOAD_BYTES)?;

    // Step 2: Try FFmpeg on the partial file
    let local_request = request_with_local_path(request, &partial_path);
    let result = super::ThumbnailGenerator::generate_local(&local_request);
    if let Err(e) = crate::locations::remove_temp_copy(&partial_path) {
        log::warn!(
            "Failed to remove temp file {}: {}",
            partial_path.display(),
            e
        );
    }

    match result {
        Ok(thumb) => Ok(thumb),
        Err(partial_err) => {
            log::debug!(
                "Partial download FFmpeg failed ({}), total_size={}: {}",
                request.path,
                total_size,
                partial_err
            );

            // Step 3: Fallback — only if the full file is reasonably sized
            if total_size > MAX_FULL_DOWNLOAD_SIZE {
                return Err(format!(
                    "Video too large for thumbnail ({:.0} MB, max {:.0} MB). \
                     Partial download also failed: {}",
                    total_size as f64 / (1024.0 * 1024.0),
                    MAX_FULL_DOWNLOAD_SIZE as f64 / (1024.0 * 1024.0),
                    partial_err
                ));
            }

            log::debug!(
                "Retrying with full download for {} ({:.1} MB)",
                request.path,
                total_size as f64 / (1024.0 * 1024.0)
            );

            let full_path = download_smb_file_sync(&request.path)?;
            let full_request = request_with_local_path(request, &full_path);
            let full_result = super::ThumbnailGenerator::generate_local(&full_request);
            if let Err(e) = crate::locations::remove_temp_copy(&full_path) {
                log::warn!("Failed to remove temp file {}: {}", full_path.display(), e);
            }
            full_result
        }
    }
}

/// Non-video thumbnail: check size, skip if too large, full download otherwise.
fn generate_smb_nonvideo_thumbnail(
    request: &ThumbnailRequest,
) -> Result<ThumbnailGenerationResult, String> {
    // Check file size before downloading
    let identity = get_smb_file_identity(&request.path)?;
    if identity.size > MAX_FULL_DOWNLOAD_SIZE {
        return Err(format!(
            "File too large for thumbnail ({:.0} MB, max {:.0} MB)",
            identity.size as f64 / (1024.0 * 1024.0),
            MAX_FULL_DOWNLOAD_SIZE as f64 / (1024.0 * 1024.0),
        ));
    }

    let temp_path = download_smb_file_sync(&request.path)?;
    let local_request = request_with_local_path(request, &temp_path);
    let result = super::ThumbnailGenerator::generate_local(&local_request);
    if let Err(e) = crate::locations::remove_temp_copy(&temp_path) {
        log::warn!("Failed to remove temp file {}: {}", temp_path.display(), e);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::smb_temp_path;

    #[test]
    fn smb_temp_path_keeps_the_file_name_in_its_own_folder() {
        let first = smb_temp_path("/a/.env").unwrap();
        let second = smb_temp_path("/a/.env").unwrap();
        let other = smb_temp_path("/b/.env").unwrap();

        assert_ne!(first.parent(), second.parent());
        assert_ne!(first.parent(), other.parent());

        for path in [&first, &second, &other] {
            assert_eq!(
                path.file_name().and_then(|name| name.to_str()),
                Some(".env")
            );
            crate::locations::remove_temp_copy(path).unwrap();
            assert!(!path.parent().unwrap().exists());
        }
    }
}
