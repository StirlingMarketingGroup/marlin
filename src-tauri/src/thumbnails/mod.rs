use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use sha2::{Sha256, Digest};
use uuid::Uuid;

pub mod cache;
pub mod worker;
pub mod generators;

use cache::ThumbnailCache;
use worker::ThumbnailWorker;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ThumbnailRequest {
    pub id: String,
    pub path: String,
    pub size: u32,
    pub quality: ThumbnailQuality,
    pub priority: ThumbnailPriority,
    pub format: ThumbnailFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ThumbnailQuality {
    Low,    // Fast generation, lower quality
    Medium, // Balanced
    High,   // Best quality, slower
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ThumbnailResponse {
    pub id: String,
    pub data_url: String,
    pub cached: bool,
    pub generation_time_ms: u64,
}

pub struct ThumbnailService {
    cache: Arc<ThumbnailCache>,
    worker: Arc<ThumbnailWorker>,
}

impl ThumbnailService {
    pub async fn new() -> Result<Self, String> {
        let cache = Arc::new(ThumbnailCache::new().await?);
        let worker = Arc::new(ThumbnailWorker::new(cache.clone()).await?);
        
        Ok(ThumbnailService {
            cache,
            worker,
        })
    }

    pub async fn request_thumbnail(&self, request: ThumbnailRequest) -> Result<ThumbnailResponse, String> {
        // Try cache first (L1 memory, then L2 disk)
        if let Some(cached_data) = self.cache.get(&request.path, request.size).await {
            return Ok(ThumbnailResponse {
                id: request.id,
                data_url: cached_data,
                cached: true,
                generation_time_ms: 0,
            });
        }

        // Submit to worker queue
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

pub fn generate_cache_key(path: &str, size: u32, mtime: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.as_bytes());
    hasher.update(size.to_be_bytes());
    hasher.update(mtime.to_be_bytes());
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
    match path.extension().and_then(|s| s.to_str()).map(|s| s.to_lowercase()).as_deref() {
        Some("jpg") | Some("jpeg") => ThumbnailFormat::JPEG,
        Some("png") | Some("gif") => ThumbnailFormat::PNG,
        _ => ThumbnailFormat::WebP, // Default to WebP for best compression
    }
}

pub fn generate_request_id() -> String {
    Uuid::new_v4().to_string()
}