use chrono::{DateTime, Utc};
use lru::LruCache;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::RwLock;

use super::AccentColor;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub data_url: String,
    pub created_at: DateTime<Utc>,
    pub last_accessed: DateTime<Utc>,
    pub size_bytes: usize,
    pub generation_time_ms: u64,
    pub has_transparency: bool,
    /// Original image width in pixels (if available)
    #[serde(default)]
    pub image_width: Option<u32>,
    /// Original image height in pixels (if available)
    #[serde(default)]
    pub image_height: Option<u32>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheStats {
    pub memory_entries: usize,
    pub memory_size_bytes: usize,
    pub disk_entries: usize,
    pub disk_size_bytes: usize,
    pub hit_rate: f32,
    pub total_hits: u64,
    pub total_misses: u64,
}

pub struct ThumbnailCache {
    // L1: In-memory LRU cache for hot thumbnails
    memory_cache: Arc<RwLock<LruCache<String, CacheEntry>>>,

    // L2: Disk cache directory
    disk_cache_dir: PathBuf,
    disk_cache_index: Arc<RwLock<HashMap<String, CacheEntry>>>,

    // Stats tracking
    stats: Arc<RwLock<CacheStats>>,

    // Configuration
    max_memory_entries: usize,
    max_memory_size_bytes: usize,
    max_disk_size_bytes: usize,
}

impl ThumbnailCache {
    pub async fn new() -> Result<Self, String> {
        let cache_dir = dirs::cache_dir()
            .ok_or("Could not determine cache directory")?
            .join("marlin_thumbnails");

        // Create cache directory if it doesn't exist
        fs::create_dir_all(&cache_dir)
            .await
            .map_err(|e| format!("Failed to create cache directory: {}", e))?;

        let memory_cache = LruCache::new(
            NonZeroUsize::new(1000).unwrap(), // Max 1000 entries in memory
        );

        let mut cache = ThumbnailCache {
            memory_cache: Arc::new(RwLock::new(memory_cache)),
            disk_cache_dir: cache_dir,
            disk_cache_index: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(CacheStats {
                memory_entries: 0,
                memory_size_bytes: 0,
                disk_entries: 0,
                disk_size_bytes: 0,
                hit_rate: 0.0,
                total_hits: 0,
                total_misses: 0,
            })),
            max_memory_entries: 1000,
            max_memory_size_bytes: 100 * 1_024 * 1_024, // 100 MiB
            max_disk_size_bytes: 500 * 1_024 * 1_024,   // 500 MiB
        };

        // Load disk cache index
        cache.load_disk_cache_index().await?;

        Ok(cache)
    }

    /// Returns (data_url, has_transparency, image_width, image_height)
    pub async fn get(
        &self,
        path: &str,
        size: u32,
        accent: Option<&AccentColor>,
    ) -> Option<(String, bool, Option<u32>, Option<u32>)> {
        let cache_key = self.generate_cache_key(path, size, accent).await?;

        // Try L1 memory cache first
        {
            let mut memory_cache = self.memory_cache.write().await;
            if let Some(entry) = memory_cache.get_mut(&cache_key) {
                entry.last_accessed = Utc::now();
                self.record_hit().await;
                return Some((entry.data_url.clone(), entry.has_transparency, entry.image_width, entry.image_height));
            }
        }

        // Try L2 disk cache
        if let Some((data_url, has_transparency, image_width, image_height)) = self.get_from_disk(&cache_key).await {
            // Promote to memory cache
            self.put_memory(&cache_key, &data_url, 0, has_transparency, image_width, image_height)
                .await;
            self.record_hit().await;
            return Some((data_url, has_transparency, image_width, image_height));
        }

        self.record_miss().await;
        None
    }

    pub async fn put(
        &self,
        path: &str,
        size: u32,
        accent: Option<&AccentColor>,
        data_url: String,
        generation_time_ms: u64,
        has_transparency: bool,
        image_width: Option<u32>,
        image_height: Option<u32>,
    ) -> Result<(), String> {
        let cache_key = self
            .generate_cache_key(path, size, accent)
            .await
            .ok_or("Failed to generate cache key")?;

        // Store in both memory and disk cache
        self.put_memory(&cache_key, &data_url, generation_time_ms, has_transparency, image_width, image_height)
            .await;
        self.put_disk(&cache_key, &data_url, generation_time_ms, has_transparency, image_width, image_height)
            .await?;

        // Cleanup if necessary
        self.cleanup_if_needed().await?;

        Ok(())
    }

    async fn put_memory(
        &self,
        key: &str,
        data_url: &str,
        generation_time_ms: u64,
        has_transparency: bool,
        image_width: Option<u32>,
        image_height: Option<u32>,
    ) {
        let entry = CacheEntry {
            data_url: data_url.to_string(),
            created_at: Utc::now(),
            last_accessed: Utc::now(),
            size_bytes: data_url.len(),
            generation_time_ms,
            has_transparency,
            image_width,
            image_height,
        };

        let mut memory_cache = self.memory_cache.write().await;

        // Check memory limits and evict if necessary
        while memory_cache.len() >= self.max_memory_entries {
            // LRU will automatically evict the least recently used item
            if memory_cache.pop_lru().is_none() {
                break; // Cache is empty, shouldn't happen but be safe
            }
        }

        // Check memory size limit
        let mut total_size = entry.size_bytes;
        for (_, existing_entry) in memory_cache.iter() {
            total_size += existing_entry.size_bytes;
        }

        while total_size > self.max_memory_size_bytes {
            if let Some((_, evicted_entry)) = memory_cache.pop_lru() {
                total_size -= evicted_entry.size_bytes;
            } else {
                break; // Cache is empty
            }
        }

        memory_cache.put(key.to_string(), entry);
    }

    async fn put_disk(
        &self,
        key: &str,
        data_url: &str,
        generation_time_ms: u64,
        has_transparency: bool,
        image_width: Option<u32>,
        image_height: Option<u32>,
    ) -> Result<(), String> {
        let entry = CacheEntry {
            data_url: data_url.to_string(),
            created_at: Utc::now(),
            last_accessed: Utc::now(),
            size_bytes: data_url.len(),
            generation_time_ms,
            has_transparency,
            image_width,
            image_height,
        };

        // Write to disk
        let file_path = self.disk_cache_dir.join(format!("{}.json", key));
        let json = serde_json::to_string(&entry)
            .map_err(|e| format!("Failed to serialize cache entry: {}", e))?;

        fs::write(&file_path, json)
            .await
            .map_err(|e| format!("Failed to write cache file: {}", e))?;

        // Update index
        let mut index = self.disk_cache_index.write().await;
        index.insert(key.to_string(), entry);

        Ok(())
    }

    async fn get_from_disk(&self, key: &str) -> Option<(String, bool, Option<u32>, Option<u32>)> {
        // Check index first
        {
            let index = self.disk_cache_index.read().await;
            if !index.contains_key(key) {
                return None;
            }
        }

        // Read from disk
        let file_path = self.disk_cache_dir.join(format!("{}.json", key));
        let json = fs::read_to_string(&file_path).await.ok()?;
        let entry: CacheEntry = serde_json::from_str(&json).ok()?;

        // Update last accessed time
        {
            let mut index = self.disk_cache_index.write().await;
            if let Some(existing_entry) = index.get_mut(key) {
                existing_entry.last_accessed = Utc::now();
            }
        }

        Some((entry.data_url, entry.has_transparency, entry.image_width, entry.image_height))
    }

    async fn generate_cache_key(
        &self,
        path: &str,
        size: u32,
        accent: Option<&AccentColor>,
    ) -> Option<String> {
        // Handle SMB paths specially - they can't use std::path for mtime
        let mtime = if path.starts_with("smb://") {
            match super::generators::smb::get_smb_file_mtime(path) {
                Ok(mtime) => mtime,
                Err(_) => {
                    // If we can't determine SMB mtime (transient network/auth failure),
                    // avoid using a constant mtime=0 which can cause indefinitely-stale cache keys.
                    // Instead, bucket to a short-lived time window.
                    let now = Utc::now().timestamp() as u64;
                    (now / 3600) * 3600
                }
            }
        } else {
            let path_obj = Path::new(path);
            super::get_file_mtime(path_obj)
        };
        Some(super::generate_cache_key(path, size, mtime, accent))
    }

    async fn load_disk_cache_index(&mut self) -> Result<(), String> {
        let mut index = HashMap::new();
        let mut total_size = 0u64;

        let mut entries = fs::read_dir(&self.disk_cache_dir)
            .await
            .map_err(|e| format!("Failed to read cache directory: {}", e))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| format!("Failed to read directory entry: {}", e))?
        {
            if let Some(extension) = entry.path().extension() {
                if extension == "json" {
                    if let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str()) {
                        if let Ok(json) = fs::read_to_string(&entry.path()).await {
                            if let Ok(mut cache_entry) = serde_json::from_str::<CacheEntry>(&json) {
                                // Handle backward compatibility for entries without has_transparency
                                if !json.contains("has_transparency") {
                                    cache_entry.has_transparency = false; // Default to false for existing entries
                                }
                                total_size += cache_entry.size_bytes as u64;
                                index.insert(stem.to_string(), cache_entry);
                            }
                        }
                    }
                }
            }
        }

        let index_len = index.len();
        *self.disk_cache_index.write().await = index;

        // Update stats
        let mut stats = self.stats.write().await;
        stats.disk_entries = index_len;
        stats.disk_size_bytes = total_size as usize;

        Ok(())
    }

    async fn cleanup_if_needed(&self) -> Result<(), String> {
        // Check if we need to cleanup disk cache
        let stats = self.stats.read().await;
        if stats.disk_size_bytes > self.max_disk_size_bytes {
            drop(stats);
            self.cleanup_disk_cache().await?;
        }

        Ok(())
    }

    async fn cleanup_disk_cache(&self) -> Result<(), String> {
        let mut entries_to_remove = Vec::new();

        {
            let index = self.disk_cache_index.read().await;
            let mut entries: Vec<_> = index.iter().collect();

            // Sort by last accessed time (oldest first)
            entries.sort_by_key(|(_, entry)| entry.last_accessed);

            let mut current_size = 0;
            for (key, entry) in entries.iter().rev() {
                current_size += entry.size_bytes;
                if current_size > self.max_disk_size_bytes * 3 / 4 {
                    // Keep 75% of max size
                    entries_to_remove.push(key.to_string());
                }
            }
        }

        // Remove old entries
        for key in entries_to_remove {
            self.remove_from_disk(&key).await?;
        }

        Ok(())
    }

    async fn remove_from_disk(&self, key: &str) -> Result<(), String> {
        let file_path = self.disk_cache_dir.join(format!("{}.json", key));
        let _ = fs::remove_file(&file_path).await; // Ignore errors for missing files

        let mut index = self.disk_cache_index.write().await;
        index.remove(key);

        Ok(())
    }

    async fn record_hit(&self) {
        let mut stats = self.stats.write().await;
        stats.total_hits += 1;
        stats.hit_rate = stats.total_hits as f32 / (stats.total_hits + stats.total_misses) as f32;
    }

    async fn record_miss(&self) {
        let mut stats = self.stats.write().await;
        stats.total_misses += 1;
        stats.hit_rate = stats.total_hits as f32 / (stats.total_hits + stats.total_misses) as f32;
    }

    pub async fn get_stats(&self) -> CacheStats {
        let mut stats = self.stats.write().await;

        // Update memory stats
        let memory_cache = self.memory_cache.read().await;
        stats.memory_entries = memory_cache.len();
        stats.memory_size_bytes = memory_cache.iter().map(|(_, entry)| entry.size_bytes).sum();

        stats.clone()
    }

    pub async fn clear(&self) -> Result<(), String> {
        // Clear memory cache
        {
            let mut memory_cache = self.memory_cache.write().await;
            memory_cache.clear();
        }

        // Clear disk cache
        let mut entries = fs::read_dir(&self.disk_cache_dir)
            .await
            .map_err(|e| format!("Failed to read cache directory: {}", e))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| format!("Failed to read directory entry: {}", e))?
        {
            let _ = fs::remove_file(entry.path()).await;
        }

        // Clear index
        {
            let mut index = self.disk_cache_index.write().await;
            index.clear();
        }

        // Reset stats
        {
            let mut stats = self.stats.write().await;
            *stats = CacheStats {
                memory_entries: 0,
                memory_size_bytes: 0,
                disk_entries: 0,
                disk_size_bytes: 0,
                hit_rate: 0.0,
                total_hits: 0,
                total_misses: 0,
            };
        }

        Ok(())
    }
}
