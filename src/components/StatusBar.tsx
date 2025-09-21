import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useAppStore } from '@/store/useAppStore';
import type { DiskUsage } from '@/types';

// Cache per-path disk usage responses briefly so we avoid hammering the backend while
// navigating through the same directory.
const DISK_USAGE_TTL_MS = 15_000;

interface CacheEntry {
  timestamp: number;
  data: DiskUsage;
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return '0 B';
  }

  const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
  let value = bytes;
  let unitIndex = 0;

  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }

  const decimals = value >= 10 || unitIndex === 0 ? 0 : 1;
  return `${value.toFixed(decimals)} ${units[unitIndex]}`;
}

function formatPercent(value: number): string {
  if (!Number.isFinite(value)) {
    return '0%';
  }
  return `${Math.round(value)}%`;
}

function StatusSegment({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <div className="flex items-baseline gap-1 text-xs text-app-muted" title={title ?? value}>
      <span>{label}</span>
      <span className="text-app-text font-medium">{value}</span>
    </div>
  );
}

export default function StatusBar() {
  const files = useAppStore((state) => state.files);
  const currentPath = useAppStore((state) => state.currentPath);
  const loading = useAppStore((state) => state.loading);
  const globalPreferences = useAppStore((state) => state.globalPreferences);
  const directoryPreferences = useAppStore((state) => state.directoryPreferences);

  const numberFormatter = useMemo(
    () => new Intl.NumberFormat(undefined, { maximumFractionDigits: 0 }),
    []
  );

  const activePreferences = useMemo(() => {
    if (!currentPath) {
      return globalPreferences;
    }
    return {
      ...globalPreferences,
      ...(directoryPreferences[currentPath] ?? {}),
    };
  }, [currentPath, globalPreferences, directoryPreferences]);

  const visibleFiles = useMemo(() => {
    if (activePreferences.showHidden) {
      return files;
    }
    return files.filter((file) => !file.is_hidden);
  }, [files, activePreferences.showHidden]);

  const stats = useMemo(() => {
    let folderCount = 0;
    let fileCount = 0;
    let totalSize = 0;

    for (const file of visibleFiles) {
      if (file.is_directory) {
        folderCount += 1;
      } else {
        fileCount += 1;
      }
      totalSize += file.size;
    }

    return {
      totalFiles: fileCount,
      folderCount,
      totalSize,
    };
  }, [visibleFiles]);

  const cacheRef = useRef<Map<string, CacheEntry>>(new Map());
  const [diskUsage, setDiskUsage] = useState<DiskUsage | null>(null);
  const [diskError, setDiskError] = useState<string | null>(null);
  const [diskLoading, setDiskLoading] = useState(false);

  useEffect(() => {
    if (!currentPath) {
      setDiskUsage(null);
      setDiskError(null);
      setDiskLoading(false);
      return;
    }

    const now = Date.now();
    const cached = cacheRef.current.get(currentPath);
    if (cached && now - cached.timestamp < DISK_USAGE_TTL_MS) {
      setDiskUsage(cached.data);
      setDiskError(null);
      setDiskLoading(false);
      return;
    }

    let cancelled = false;
    setDiskLoading(true);
    setDiskError(null);

    invoke<DiskUsage>('get_disk_usage', { path: currentPath })
      .then((result) => {
        if (cancelled) return;
        const entry: CacheEntry = { data: result, timestamp: Date.now() };
        cacheRef.current.set(currentPath, entry);
        cacheRef.current.set(result.path, entry);
        setDiskUsage(result);
      })
      .catch((error) => {
        if (cancelled) return;
        console.warn('Failed to fetch disk usage:', error);
        setDiskUsage(null);
        setDiskError(error instanceof Error ? error.message : String(error));
      })
      .finally(() => {
        if (!cancelled) {
          setDiskLoading(false);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [currentPath, files]);

  const diskSummary = useMemo(() => {
    if (diskLoading && !diskUsage) {
      return 'Calculating...';
    }
    if (diskError) {
      return 'Unavailable';
    }
    if (!diskUsage) {
      return '--';
    }

    const percentFree = diskUsage.totalBytes
      ? (diskUsage.availableBytes / diskUsage.totalBytes) * 100
      : 0;

    const tooltip = `${formatBytes(diskUsage.availableBytes)} free of ${formatBytes(diskUsage.totalBytes)} (${formatPercent(percentFree)})`;

    return {
      display: `${formatBytes(diskUsage.availableBytes)} free`,
      detail: `${formatBytes(diskUsage.totalBytes)}`,
      tooltip,
      percent: percentFree,
    };
  }, [diskUsage, diskError, diskLoading]);

  return (
    <div className="pointer-events-auto border border-app-border/70 border-b-0 border-r-0 bg-app-darker/90 px-4 py-2 text-xs rounded-tl-lg backdrop-blur">
      <div className="flex flex-wrap items-center gap-4">
        <StatusSegment
          label="Folders"
          value={loading && files.length === 0 ? '...' : numberFormatter.format(stats.folderCount)}
        />
        <StatusSegment
          label="Files"
          value={loading && files.length === 0 ? '...' : numberFormatter.format(stats.totalFiles)}
        />
        <StatusSegment
          label="Size"
          value={loading && files.length === 0 ? '...' : formatBytes(stats.totalSize)}
          title={`${formatBytes(stats.totalSize)} (${numberFormatter.format(stats.totalSize)} B)`}
        />
        <div className="flex items-baseline gap-1 text-xs text-app-muted">
          <span>Disk</span>
          {typeof diskSummary === 'string' ? (
            <span className="text-app-text font-medium">{diskSummary}</span>
          ) : (
            <span className="text-app-text font-medium" title={diskSummary.tooltip}>
              {diskSummary.display}
              <span className="text-app-muted/70 ml-2">
                {diskSummary.detail} | {formatPercent(diskSummary.percent)} free
              </span>
            </span>
          )}
        </div>
      </div>
    </div>
  );
}
