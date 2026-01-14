import { useEffect, useState, useRef, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';

export interface AccentColor {
  r: number;
  g: number;
  b: number;
}

export interface ThumbnailRequest {
  path: string;
  size?: number;
  quality?: 'low' | 'medium' | 'high';
  priority?: 'high' | 'medium' | 'low';
  format?: 'webp' | 'png' | 'jpeg';
  accent?: AccentColor;
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

const clampByte = (value: number) => Math.max(0, Math.min(255, Math.round(value)));

const parseCssColorToRgb = (raw: string): AccentColor | undefined => {
  const value = raw.trim();
  if (!value) {
    return undefined;
  }

  if (value.startsWith('#')) {
    const hex = value.slice(1);
    if (hex.length === 6) {
      const r = parseInt(hex.slice(0, 2), 16);
      const g = parseInt(hex.slice(2, 4), 16);
      const b = parseInt(hex.slice(4, 6), 16);
      if ([r, g, b].every((component) => Number.isFinite(component))) {
        return { r, g, b };
      }
    } else if (hex.length === 3) {
      const r = parseInt(hex[0] + hex[0], 16);
      const g = parseInt(hex[1] + hex[1], 16);
      const b = parseInt(hex[2] + hex[2], 16);
      if ([r, g, b].every((component) => Number.isFinite(component))) {
        return { r, g, b };
      }
    }
    return undefined;
  }

  const rgbaMatch = value.match(/^rgba?\(([^)]+)\)$/i);
  if (rgbaMatch) {
    const parts = rgbaMatch[1]
      .split(',')
      .map((part) => parseFloat(part.trim()))
      .filter((component) => Number.isFinite(component));
    if (parts.length >= 3) {
      return {
        r: clampByte(parts[0]),
        g: clampByte(parts[1]),
        b: clampByte(parts[2]),
      };
    }
  }

  const parseNumericList = (input: string, separator: RegExp | string) => {
    const parts = input
      .split(separator)
      .map((part) => part.trim())
      .filter(Boolean);
    if (parts.length !== 3) {
      return undefined;
    }
    const numeric = parts.map((component) => {
      if (!/^-?\d+(?:\.\d+)?$/.test(component)) {
        return undefined;
      }
      return clampByte(parseFloat(component));
    });
    if (numeric.some((component) => component === undefined)) {
      return undefined;
    }
    const [r, g, b] = numeric as number[];
    return { r, g, b };
  };

  const commaSeparated = parseNumericList(value, /\s*,\s*/);
  if (commaSeparated) {
    return commaSeparated;
  }

  const spaceSeparated = parseNumericList(value.replace(/\s+/g, ' '), ' ');
  if (spaceSeparated) {
    return spaceSeparated;
  }

  return undefined;
};

const getCssAccentColor = (): AccentColor | undefined => {
  if (typeof window === 'undefined') {
    return undefined;
  }

  const root = document.documentElement;
  const computed = window.getComputedStyle(root);
  const candidates = [
    computed.getPropertyValue('--accent'),
    computed.getPropertyValue('--color-accent'),
    computed.getPropertyValue('--accent-rgb'),
  ];

  for (const candidate of candidates) {
    const color = parseCssColorToRgb(candidate);
    if (color) {
      return color;
    }
  }

  return undefined;
};

const accentKeyFor = (color: AccentColor | undefined) =>
  color ? `${color.r}-${color.g}-${color.b}` : 'none';

function useAccentColor() {
  const [accent, setAccent] = useState<AccentColor | undefined>(() => getCssAccentColor());

  useEffect(() => {
    if (typeof window === 'undefined') {
      return;
    }

    const root = document.documentElement;
    const updateAccent = () => {
      const next = getCssAccentColor();
      setAccent((prev) => {
        const prevKey = accentKeyFor(prev);
        const nextKey = accentKeyFor(next);
        if (prevKey === nextKey) {
          return prev;
        }
        return next;
      });
    };

    // Ensure we pick up late accent updates on mount
    updateAccent();

    const observer = new MutationObserver(updateAccent);
    observer.observe(root, { attributes: true, attributeFilter: ['style'] });

    return () => observer.disconnect();
  }, []);

  return { accent, accentKey: accentKeyFor(accent) } as const;
}

// Global cache for thumbnail promises to prevent duplicate requests
const thumbnailPromises = new Map<string, Promise<ThumbnailResponse>>();

// Active request IDs for cancellation
const activeRequests = new Map<string, string>();

export interface ThumbnailOptions extends Omit<ThumbnailRequest, 'path'> {
  /** Remote thumbnail URL (e.g., from Google Drive) - used directly instead of generating */
  thumbnailUrl?: string;
}

export function useThumbnail(path: string | undefined, options: ThumbnailOptions = {}) {
  const { accent } = useAccentColor();
  const { size, quality, priority, format, accent: accentOverride, thumbnailUrl } = options;
  const effectiveAccent = accentOverride ?? accent;
  const effectiveAccentKey = accentKeyFor(effectiveAccent);
  const [dataUrl, setDataUrl] = useState<string | undefined>(undefined);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | undefined>(undefined);
  const [cached, setCached] = useState(false);
  const [generationTimeMs, setGenerationTimeMs] = useState<number>(0);
  const [hasTransparency, setHasTransparency] = useState<boolean>(false);

  const abortControllerRef = useRef<AbortController | null>(null);
  const currentPathRef = useRef<string | null>(null);
  const requestIdRef = useRef<string | null>(null);
  const cancelCurrentRequest = useCallback(() => {
    if (abortControllerRef.current) {
      abortControllerRef.current.abort();
      abortControllerRef.current = null;
    }
    if (requestIdRef.current) {
      const requestId = requestIdRef.current;
      requestIdRef.current = null;
      invoke('cancel_thumbnail', { requestId }).catch(() => {
        // Ignore cancellation errors
      });
    }
    currentPathRef.current = null;
  }, []);

  // If we have a remote thumbnail URL (e.g., from Google Drive), use it directly
  useEffect(() => {
    if (thumbnailUrl && path) {
      setDataUrl(thumbnailUrl);
      setCached(true);
      setGenerationTimeMs(0);
      setHasTransparency(false);
      setLoading(false);
      setError(undefined);
    }
  }, [thumbnailUrl, path]);

  const fetchThumbnail = useCallback(
    async (thumbnailPath: string, requestOptions: Omit<ThumbnailRequest, 'path'>) => {
      // Create cache key
      const accentKey = accentKeyFor(requestOptions.accent);
      const cacheKey = `${thumbnailPath}:${requestOptions.size || 128}:${requestOptions.quality || 'medium'}:${requestOptions.format || 'webp'}:${accentKey}`;

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
        accent: requestOptions.accent ?? null,
      });

      // Store promise in cache
      thumbnailPromises.set(cacheKey, requestPromise);

      // Do not expire successful promises eagerly; they serve as an L1 in-memory cache
      // Remove failed requests immediately so they can be retried
      requestPromise.catch(() => {
        thumbnailPromises.delete(cacheKey);
      });

      return requestPromise;
    },
    []
  );

  useEffect(() => {
    if (!path) {
      // If path becomes undefined, cancel any in-flight request but keep current dataUrl
      cancelCurrentRequest();
      setLoading(false);
      return;
    }

    // If we have a remote thumbnail URL, skip backend generation
    if (thumbnailUrl) {
      return;
    }

    // Cancel any existing request
    cancelCurrentRequest();

    // Create new abort controller
    abortControllerRef.current = new AbortController();
    currentPathRef.current = path;

    setLoading(true);
    setError(undefined);

    fetchThumbnail(path, { size, quality, priority, format, accent: effectiveAccent })
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
      cancelCurrentRequest();
    };
  }, [
    path,
    thumbnailUrl,
    size,
    quality,
    priority,
    format,
    effectiveAccent,
    effectiveAccentKey,
    fetchThumbnail,
    cancelCurrentRequest,
  ]);

  const retry = useCallback(() => {
    if (path) {
      setError(undefined);
      // Force a new request by clearing the cache for this item
      const cacheKey = `${path}:${size || 128}:${quality || 'medium'}:${format || 'webp'}:${effectiveAccentKey}`;
      thumbnailPromises.delete(cacheKey);

      // Trigger re-fetch
      setLoading(true);
      fetchThumbnail(path, { size, quality, priority, format, accent: effectiveAccent })
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
  }, [path, size, quality, priority, format, effectiveAccent, effectiveAccentKey, fetchThumbnail]);

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
