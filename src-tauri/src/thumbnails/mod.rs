use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use uuid::Uuid;

pub mod cache;
pub mod generators;
pub mod worker;

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

    pub async fn get_cache_stats(&self) -> cache::CacheStats {
        self.cache.get_stats().await
    }

    pub async fn clear_cache(&self) -> Result<(), String> {
        self.cache.clear().await
    }
}

pub fn generate_cache_key(
    path: &str,
    size: u32,
    mtime: u64,
    accent: Option<&AccentColor>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.as_bytes());
    hasher.update(size.to_be_bytes());
    hasher.update(mtime.to_be_bytes());
    if let Some(color) = accent {
        hasher.update([color.r, color.g, color.b]);
    }
    let result = hasher.finalize();
    format!("{:x}", result)[..16].to_string()
}

pub fn get_file_mtime(path: &Path) -> u64 {
    path.metadata()
        .and_then(|m| m.modified())
        .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs())
        .unwrap_or(0)
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
