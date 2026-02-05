use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use uuid::Uuid;

pub mod cache;
pub mod generators;
pub mod worker;

/// Version prefix for cache keys - increment to invalidate all existing cache entries
const CACHE_KEY_VERSION: &str = "v2";

/// File identity information used for cache key generation.
/// Includes multiple attributes to detect file changes that mtime alone might miss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIdentity {
    /// File size in bytes
    pub size: u64,
    /// Modification time in nanoseconds since UNIX epoch
    pub mtime_ns: u128,
    /// Platform-specific file identifier (inode on Unix, file_index on Windows)
    pub file_id: Option<u64>,
}

/// Get file identity information for cache key generation.
/// Returns identity with nanosecond mtime precision and platform-specific file ID.
pub fn get_file_identity(path: &Path) -> FileIdentity {
    let metadata = path.metadata().ok();

    let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);

    // Nanosecond mtime precision
    let mtime_ns = metadata
        .as_ref()
        .and_then(|m| m.modified().ok())
        .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos())
        .unwrap_or(0);

    // Platform-specific file identity
    #[cfg(target_family = "unix")]
    let file_id = {
        use std::os::unix::fs::MetadataExt;
        metadata.as_ref().map(|m| m.ino())
    };

    #[cfg(target_family = "windows")]
    let file_id = {
        use std::os::windows::fs::MetadataExt;
        metadata.as_ref().and_then(|m| m.file_index())
    };

    #[cfg(not(any(target_family = "unix", target_family = "windows")))]
    let file_id: Option<u64> = None;

    FileIdentity {
        size,
        mtime_ns,
        file_id,
    }
}

use cache::ThumbnailCache;
use worker::ThumbnailWorker;

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct AccentColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ThumbnailRequest {
    pub id: String,
    pub path: String,
    pub size: u32,
    pub quality: ThumbnailQuality,
    pub priority: ThumbnailPriority,
    pub format: ThumbnailFormat,
    pub accent: Option<AccentColor>,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ThumbnailQuality {
    Low,    // Fast generation, lower quality
    Medium, // Balanced
    High,   // Best quality, slower
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum ThumbnailPriority {
    Low = 0,    // Background generation
    Medium = 1, // Near visible viewport
    High = 2,   // Visible items
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ThumbnailFormat {
    WebP,
    PNG,
    JPEG,
}

/// Result of thumbnail generation including metadata
#[derive(Debug, Clone)]
pub struct ThumbnailGenerationResult {
    pub data_url: String,
    pub has_transparency: bool,
    /// Original image width in pixels (if available)
    pub image_width: Option<u32>,
    /// Original image height in pixels (if available)
    pub image_height: Option<u32>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ThumbnailResponse {
    pub id: String,
    pub data_url: String,
    pub cached: bool,
    pub generation_time_ms: u64,
    pub has_transparency: bool,
    /// Original image width in pixels (if available)
    pub image_width: Option<u32>,
    /// Original image height in pixels (if available)
    pub image_height: Option<u32>,
}

pub struct ThumbnailService {
    cache: Arc<ThumbnailCache>,
    worker: Arc<ThumbnailWorker>,
}

impl ThumbnailService {
    pub async fn new() -> Result<Self, String> {
        let cache = Arc::new(ThumbnailCache::new().await?);
        let worker = Arc::new(ThumbnailWorker::new(cache.clone()).await?);

        Ok(ThumbnailService { cache, worker })
    }

    pub async fn request_thumbnail(
        &self,
        request: ThumbnailRequest,
    ) -> Result<ThumbnailResponse, String> {
        fn expects_image_dimensions(path: &str) -> bool {
            let ext = Path::new(path)
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase());

            matches!(
                ext.as_deref(),
                Some("jpg")
                    | Some("jpeg")
                    | Some("png")
                    | Some("gif")
                    | Some("webp")
                    | Some("bmp")
                    | Some("tiff")
                    | Some("tga")
                    | Some("ico")
            )
        }

        let request_clone = request.clone();

        // Try cache first (L1 memory, then L2 disk)
        if let Some((cached_data, has_transparency, image_width, image_height)) = self
            .cache
            .get(&request.path, request.size, request.accent.as_ref())
            .await
        {
            log::info!(
                "THUMBNAIL CACHE HIT: path={}, dimensions={:?}x{:?}",
                request.path,
                image_width,
                image_height
            );

            // SMB thumbnails created before we started persisting dimensions will load fine but never
            // show original image size in the UI. If the cache entry has no dimensions for an
            // image type, regenerate once to backfill metadata; fall back to cached thumbnail on
            // failure.
            if request.path.starts_with("smb://")
                && expects_image_dimensions(&request.path)
                && (image_width.is_none() || image_height.is_none())
            {
                log::info!(
                    "THUMBNAIL CACHE HIT MISSING DIMENSIONS: path={}, regenerating...",
                    request.path
                );
                match self.worker.submit_request(request_clone).await {
                    Ok(response) => return Ok(response),
                    Err(err) => {
                        log::warn!(
                            "Failed to regenerate SMB thumbnail to backfill dimensions (serving cached): path={}, error={}",
                            request.path,
                            err
                        );
                    }
                }
            }
            return Ok(ThumbnailResponse {
                id: request.id,
                data_url: cached_data,
                cached: true,
                generation_time_ms: 0,
                has_transparency,
                image_width,
                image_height,
            });
        }

        // Cache miss - submit to worker queue
        log::info!("THUMBNAIL CACHE MISS: path={}", request.path);
        self.worker.submit_request(request).await
    }

    pub async fn cancel_request(&self, id: &str) -> bool {
        self.worker.cancel_request(id).await
    }

    pub fn cancel_all(&self) -> bool {
        self.worker.cancel_all()
    }

    pub async fn get_cache_stats(&self) -> cache::CacheStats {
        self.cache.get_stats().await
    }

    pub async fn clear_cache(&self) -> Result<(), String> {
        self.cache.clear().await
    }

    /// Invalidate cache entries for the given paths.
    /// Called when files are modified/removed to ensure fresh thumbnails on next request.
    pub async fn invalidate_paths(&self, paths: &[String]) {
        self.cache.invalidate_paths(paths).await
    }
}

/// Generate a cache key for a thumbnail based on path, size, file identity, and accent color.
/// Uses CACHE_KEY_VERSION prefix to allow invalidation of old cache entries.
pub fn generate_cache_key(
    path: &str,
    thumb_size: u32,
    identity: &FileIdentity,
    accent: Option<&AccentColor>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(CACHE_KEY_VERSION.as_bytes());
    hasher.update(path.as_bytes());
    hasher.update(thumb_size.to_be_bytes());
    hasher.update(identity.size.to_be_bytes());
    hasher.update(identity.mtime_ns.to_be_bytes());
    if let Some(id) = identity.file_id {
        hasher.update(id.to_be_bytes());
    }
    if let Some(color) = accent {
        hasher.update([color.r, color.g, color.b]);
    }
    let result = hasher.finalize();
    format!("{:x}", result)[..16].to_string()
}

pub fn get_thumbnail_format_from_path(path: &Path) -> ThumbnailFormat {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => ThumbnailFormat::JPEG,
        Some("png") | Some("gif") | Some("svg") => ThumbnailFormat::PNG,
        _ => ThumbnailFormat::WebP, // Default to WebP for best compression
    }
}

pub fn generate_request_id() -> String {
    Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_cache_key_differs_with_different_file_size() {
        let id1 = FileIdentity {
            size: 1000,
            mtime_ns: 12345,
            file_id: Some(1),
        };
        let id2 = FileIdentity {
            size: 2000,
            mtime_ns: 12345,
            file_id: Some(1),
        };
        let key1 = generate_cache_key("test.jpg", 128, &id1, None);
        let key2 = generate_cache_key("test.jpg", 128, &id2, None);
        assert_ne!(key1, key2, "Keys should differ when file size differs");
    }

    #[test]
    fn test_cache_key_differs_with_different_inode() {
        let id1 = FileIdentity {
            size: 1000,
            mtime_ns: 12345,
            file_id: Some(1),
        };
        let id2 = FileIdentity {
            size: 1000,
            mtime_ns: 12345,
            file_id: Some(2),
        };
        let key1 = generate_cache_key("test.jpg", 128, &id1, None);
        let key2 = generate_cache_key("test.jpg", 128, &id2, None);
        assert_ne!(key1, key2, "Keys should differ when file_id (inode) differs");
    }

    #[test]
    fn test_cache_key_differs_with_different_mtime_ns() {
        let id1 = FileIdentity {
            size: 1000,
            mtime_ns: 12345000000,
            file_id: Some(1),
        };
        let id2 = FileIdentity {
            size: 1000,
            mtime_ns: 12345000001,
            file_id: Some(1),
        };
        let key1 = generate_cache_key("test.jpg", 128, &id1, None);
        let key2 = generate_cache_key("test.jpg", 128, &id2, None);
        assert_ne!(
            key1, key2,
            "Keys should differ when mtime_ns differs by even 1 nanosecond"
        );
    }

    #[test]
    fn test_cache_key_same_for_identical_identity() {
        let id1 = FileIdentity {
            size: 1000,
            mtime_ns: 12345,
            file_id: Some(1),
        };
        let id2 = FileIdentity {
            size: 1000,
            mtime_ns: 12345,
            file_id: Some(1),
        };
        let key1 = generate_cache_key("test.jpg", 128, &id1, None);
        let key2 = generate_cache_key("test.jpg", 128, &id2, None);
        assert_eq!(key1, key2, "Keys should be the same for identical identity");
    }

    #[test]
    fn test_cache_key_differs_with_different_path() {
        let id = FileIdentity {
            size: 1000,
            mtime_ns: 12345,
            file_id: Some(1),
        };
        let key1 = generate_cache_key("test1.jpg", 128, &id, None);
        let key2 = generate_cache_key("test2.jpg", 128, &id, None);
        assert_ne!(key1, key2, "Keys should differ when path differs");
    }

    #[test]
    fn test_cache_key_differs_with_different_thumb_size() {
        let id = FileIdentity {
            size: 1000,
            mtime_ns: 12345,
            file_id: Some(1),
        };
        let key1 = generate_cache_key("test.jpg", 128, &id, None);
        let key2 = generate_cache_key("test.jpg", 256, &id, None);
        assert_ne!(key1, key2, "Keys should differ when thumbnail size differs");
    }

    #[test]
    fn test_cache_key_differs_with_accent_color() {
        let id = FileIdentity {
            size: 1000,
            mtime_ns: 12345,
            file_id: Some(1),
        };
        let accent1 = AccentColor {
            r: 255,
            g: 0,
            b: 0,
        };
        let accent2 = AccentColor {
            r: 0,
            g: 255,
            b: 0,
        };
        let key1 = generate_cache_key("test.jpg", 128, &id, Some(&accent1));
        let key2 = generate_cache_key("test.jpg", 128, &id, Some(&accent2));
        let key3 = generate_cache_key("test.jpg", 128, &id, None);
        assert_ne!(key1, key2, "Keys should differ when accent colors differ");
        assert_ne!(key1, key3, "Keys should differ when one has accent and other doesn't");
    }

    #[test]
    fn test_cache_key_handles_none_file_id() {
        let id1 = FileIdentity {
            size: 1000,
            mtime_ns: 12345,
            file_id: None,
        };
        let id2 = FileIdentity {
            size: 1000,
            mtime_ns: 12345,
            file_id: Some(1),
        };
        let key1 = generate_cache_key("test.jpg", 128, &id1, None);
        let key2 = generate_cache_key("test.jpg", 128, &id2, None);
        // Keys should differ when one has file_id and other doesn't
        assert_ne!(
            key1, key2,
            "Keys should differ when one has file_id and other doesn't"
        );
    }

    #[test]
    fn test_get_file_identity_basic() {
        let dir = tempdir().expect("Failed to create temp dir");
        let path = dir.path().join("test.txt");

        // Create a file
        let mut file = File::create(&path).expect("Failed to create file");
        file.write_all(b"hello world").expect("Failed to write");
        drop(file);

        let identity = get_file_identity(&path);

        // File size should be 11 bytes
        assert_eq!(identity.size, 11, "File size should be 11 bytes");

        // mtime_ns should be non-zero
        assert!(identity.mtime_ns > 0, "mtime_ns should be non-zero");

        // On Unix/Windows, file_id should be Some
        #[cfg(any(target_family = "unix", target_family = "windows"))]
        assert!(
            identity.file_id.is_some(),
            "file_id should be Some on Unix/Windows"
        );
    }

    #[test]
    fn test_delete_recreate_produces_different_key() {
        let dir = tempdir().expect("Failed to create temp dir");
        let path = dir.path().join("test.jpg");

        // Create file with content1
        {
            let mut file = File::create(&path).expect("Failed to create file");
            file.write_all(b"content1").expect("Failed to write");
        }
        let id1 = get_file_identity(&path);
        let key1 = generate_cache_key(path.to_str().unwrap(), 128, &id1, None);

        // Delete and recreate with different content
        fs::remove_file(&path).expect("Failed to remove file");
        // Small delay to ensure different mtime on filesystems with low resolution
        std::thread::sleep(std::time::Duration::from_millis(10));
        {
            let mut file = File::create(&path).expect("Failed to create file");
            file.write_all(b"content2 longer").expect("Failed to write");
        }
        let id2 = get_file_identity(&path);
        let key2 = generate_cache_key(path.to_str().unwrap(), 128, &id2, None);

        // Keys should differ due to different inode and/or mtime and/or size
        assert_ne!(
            key1, key2,
            "Cache keys should differ after delete and recreate"
        );

        // Verify the identities actually differ
        let differs = id1.size != id2.size || id1.mtime_ns != id2.mtime_ns || id1.file_id != id2.file_id;
        assert!(
            differs,
            "File identity should differ after delete and recreate"
        );
    }

    #[test]
    fn test_get_file_identity_nonexistent_file() {
        // Use tempdir to create a portable path that definitely doesn't exist
        let dir = tempdir().expect("Failed to create temp dir");
        let path = dir.path().join("this_file_does_not_exist.jpg");

        let identity = get_file_identity(&path);

        // Should return zero values for nonexistent file
        assert_eq!(identity.size, 0, "Size should be 0 for nonexistent file");
        assert_eq!(
            identity.mtime_ns, 0,
            "mtime_ns should be 0 for nonexistent file"
        );
        assert!(
            identity.file_id.is_none(),
            "file_id should be None for nonexistent file"
        );
    }
}
