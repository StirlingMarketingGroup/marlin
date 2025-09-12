import { useEffect, useState, useRef, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';

export interface ThumbnailRequest {
  path: string;
  size?: number;
  quality?: 'low' | 'medium' | 'high';
  priority?: 'high' | 'medium' | 'low';
  format?: 'webp' | 'png' | 'jpeg';
}

export interface ThumbnailResponse {
  id: string;
  data_url: string;
  cached: boolean;
  generation_time_ms: number;
  has_transparency: boolean;
}

export interface ThumbnailCacheStats {
  memory_entries: number;
  memory_size_bytes: number;
  disk_entries: number;
  disk_size_bytes: number;
  hit_rate: number;
  total_hits: number;
  total_misses: number;
}

// Global cache for thumbnail promises to prevent duplicate requests
const thumbnailPromises = new Map<string, Promise<ThumbnailResponse>>();

// Active request IDs for cancellation
const activeRequests = new Map<string, string>();

export function useThumbnail(
  path: string | undefined,
  options: Omit<ThumbnailRequest, 'path'> = {}
) {
  const [dataUrl, setDataUrl] = useState<string | undefined>(undefined);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | undefined>(undefined);
  const [cached, setCached] = useState(false);
  const [generationTimeMs, setGenerationTimeMs] = useState<number>(0);
  const [hasTransparency, setHasTransparency] = useState<boolean>(false);
  
  const abortControllerRef = useRef<AbortController>();
  const currentPathRef = useRef<string>();
  const requestIdRef = useRef<string>();

  const fetchThumbnail = useCallback(async (thumbnailPath: string, requestOptions: Omit<ThumbnailRequest, 'path'>) => {
    // Create cache key
    const cacheKey = `${thumbnailPath}:${requestOptions.size || 128}:${requestOptions.quality || 'medium'}:${requestOptions.format || 'webp'}`;
    
    // Check if we already have a promise for this request
    if (thumbnailPromises.has(cacheKey)) {
      return thumbnailPromises.get(cacheKey)!;
    }

    // Create new request promise
    const requestPromise = invoke<ThumbnailResponse>('request_thumbnail', {
      path: thumbnailPath,
      size: requestOptions.size,
      quality: requestOptions.quality,
      priority: requestOptions.priority,
      format: requestOptions.format,
    });

    // Store promise in cache
    thumbnailPromises.set(cacheKey, requestPromise);

    // Do not expire successful promises eagerly; they serve as an L1 in-memory cache
    // Remove failed requests immediately so they can be retried
    requestPromise.catch(() => {
      thumbnailPromises.delete(cacheKey);
    });

    return requestPromise;
  }, []);

  useEffect(() => {
    if (!path) {
      // If path becomes undefined, cancel any in-flight request but keep current dataUrl
      if (abortControllerRef.current) {
        abortControllerRef.current.abort();
      }
      if (requestIdRef.current) {
        invoke('cancel_thumbnail', { requestId: requestIdRef.current }).catch(() => {})
      }
      setLoading(false)
      return
    }

    // Cancel any existing request
    if (abortControllerRef.current) {
      abortControllerRef.current.abort();
    }

    if (requestIdRef.current) {
      invoke('cancel_thumbnail', { requestId: requestIdRef.current }).catch(() => {
        // Ignore cancellation errors
      });
    }

    // Create new abort controller
    abortControllerRef.current = new AbortController();
    currentPathRef.current = path;

    setLoading(true);
    setError(undefined);

    fetchThumbnail(path, options)
      .then((response) => {
        // Check if this is still the current request
        if (currentPathRef.current === path && !abortControllerRef.current?.signal.aborted) {
          setDataUrl(response.data_url);
          setCached(response.cached);
          setGenerationTimeMs(response.generation_time_ms);
          setHasTransparency(response.has_transparency);
          requestIdRef.current = response.id;
          setLoading(false);
        }
      })
      .catch((err) => {
        // Check if this is still the current request and wasn't cancelled
        if (currentPathRef.current === path && !abortControllerRef.current?.signal.aborted) {
          setError(err.toString());
          setLoading(false);
        }
      });

    return () => {
      if (abortControllerRef.current) {
        abortControllerRef.current.abort();
      }
      if (requestIdRef.current) {
        invoke('cancel_thumbnail', { requestId: requestIdRef.current }).catch(() => {
          // Ignore cancellation errors
        });
      }
    };
  }, [path, options.size, options.quality, options.priority, options.format, fetchThumbnail]);

  const retry = useCallback(() => {
    if (path) {
      setError(undefined);
      // Force a new request by clearing the cache for this item
      const cacheKey = `${path}:${options.size || 128}:${options.quality || 'medium'}:${options.format || 'webp'}`;
      thumbnailPromises.delete(cacheKey);
      
      // Trigger re-fetch
      setLoading(true);
      fetchThumbnail(path, options)
        .then((response) => {
          if (currentPathRef.current === path) {
            setDataUrl(response.data_url);
            setCached(response.cached);
            setGenerationTimeMs(response.generation_time_ms);
            setHasTransparency(response.has_transparency);
            requestIdRef.current = response.id;
            setLoading(false);
          }
        })
        .catch((err) => {
          if (currentPathRef.current === path) {
            setError(err.toString());
            setLoading(false);
          }
        });
    }
  }, [path, options, fetchThumbnail]);

  return {
    dataUrl,
    loading,
    error,
    cached,
    generationTimeMs,
    hasTransparency,
    retry,
  };
}

// Hook for managing thumbnail cache
export function useThumbnailCache() {
  const [stats, setStats] = useState<ThumbnailCacheStats | undefined>(undefined);
  const [loading, setLoading] = useState(false);

  const getStats = useCallback(async () => {
    setLoading(true);
    try {
      const cacheStats = await invoke<ThumbnailCacheStats>('get_thumbnail_cache_stats');
      setStats(cacheStats);
    } catch (err) {
      console.error('Failed to get thumbnail cache stats:', err);
    } finally {
      setLoading(false);
    }
  }, []);

  const clearCache = useCallback(async () => {
    setLoading(true);
    try {
      await invoke('clear_thumbnail_cache');
      // Clear in-memory promise cache as well
      thumbnailPromises.clear();
      activeRequests.clear();
      await getStats(); // Refresh stats
    } catch (err) {
      console.error('Failed to clear thumbnail cache:', err);
    } finally {
      setLoading(false);
    }
  }, [getStats]);

  return {
    stats,
    loading,
    getStats,
    clearCache,
  };
}
