//! SFTP thumbnail generation support
//!
//! Downloads SFTP files to a local temp directory before generating thumbnails.
//! All network operations are async to avoid blocking tokio or rayon threads.


/// Check if the path is an SFTP URL
pub fn is_sftp_path(path: &str) -> bool {
    path.starts_with("sftp://")
}

/// Download an SFTP file asynchronously and return the local temp path.
pub async fn download_sftp_file_async(sftp_path: &str) -> Result<std::path::PathBuf, String> {
    crate::locations::sftp::download_sftp_file_to_temp(sftp_path).await
}

/// Get file identity for an SFTP file including size and mtime. Fully async.
pub async fn get_sftp_file_identity_async(
    sftp_path: &str,
) -> Result<super::super::FileIdentity, String> {
    let (_, hostname, port, remote_path) = crate::locations::sftp::parse_sftp_url(sftp_path)?;

    let sftp = crate::locations::sftp::pool::get_sftp_session(&hostname, port).await?;
    let _permit = crate::locations::sftp::pool::acquire_permit(&hostname, port).await?;
    let attrs = sftp
        .metadata(&remote_path)
        .await
        .map_err(|e| format!("Failed to get SFTP metadata: {}", e))?;

    let mtime = attrs.mtime.unwrap_or(0);
    let mtime_ns = (mtime as u128) * 1_000_000_000;
    let size = attrs.len();

    Ok(super::super::FileIdentity {
        size,
        mtime_ns,
        file_id: None,
    })
}

