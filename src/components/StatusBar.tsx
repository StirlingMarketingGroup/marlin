import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useAppStore } from '@/store/useAppStore';
import type { DiskUsage } from '@/types';
import { ArrowSquareOut } from 'phosphor-react';
import { openUrl } from '@tauri-apps/plugin-opener';

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

  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'];
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
  const gitStatus = useAppStore((state) => state.gitStatus);
  const gitStatusLoading = useAppStore((state) => state.gitStatusLoading);
  const gitStatusError = useAppStore((state) => state.gitStatusError);

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
  }, [currentPath]);

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

  const handleOpenRemote = useCallback(async () => {
    const targetUrl = gitStatus?.remoteBranchUrl ?? gitStatus?.remoteUrl;
    if (!targetUrl) {
      return;
    }

    try {
      await openUrl(targetUrl);
    } catch (primaryError) {
      try {
        await invoke('open_path', { path: targetUrl });
      } catch (fallbackError) {
        console.warn('Failed to open remote repository:', primaryError, fallbackError);
      }
    }
  }, [gitStatus?.remoteBranchUrl, gitStatus?.remoteUrl]);

  const gitSegment = useMemo(() => {
    if (gitStatusLoading) {
      return (
        <div className="flex items-baseline gap-1 text-xs text-app-muted">
          <span>Git</span>
          <span className="text-app-text font-medium">Checking...</span>
        </div>
      );
    }

    if (gitStatusError) {
      return (
        <div className="flex items-baseline gap-1 text-xs text-app-muted" title={gitStatusError}>
          <span>Git</span>
          <span className="text-amber-400 font-medium">Unavailable</span>
        </div>
      );
    }

    if (!gitStatus) {
      return null;
    }

    const branchLabel = gitStatus.detached
      ? gitStatus.branch
        ? `detached @ ${gitStatus.branch}`
        : 'detached HEAD'
      : (gitStatus.branch ?? 'HEAD');

    const remoteHost = (() => {
      if (!gitStatus.remoteUrl) return null;
      try {
        return new URL(gitStatus.remoteUrl).host;
      } catch (error) {
        console.warn('Failed to parse remote URL host:', error);
        return gitStatus.remoteUrl.replace(/^https?:\/\//i, '');
      }
    })();

    const indicatorItems: ReactNode[] = [];
    const hasDirtyChanges = gitStatus.dirty || gitStatus.hasUntracked;
    const dirtyTitle = hasDirtyChanges
      ? [
          gitStatus.dirty ? 'Working tree has staged or unstaged changes' : null,
          gitStatus.hasUntracked ? 'Untracked files present' : null,
        ]
          .filter(Boolean)
          .join('. ')
      : 'Working tree clean';

    indicatorItems.push(
      <span
        key="dirty"
        className={`flex items-center gap-1 text-xs ${
          hasDirtyChanges ? 'text-amber-400' : 'text-emerald-400'
        }`}
        title={dirtyTitle}
      >
        <span aria-hidden className="inline-flex h-2 w-2 rounded-full bg-current" />
        <span>{hasDirtyChanges ? 'Dirty' : 'Clean'}</span>
      </span>
    );

    if (gitStatus.ahead > 0) {
      indicatorItems.push(
        <span
          key="ahead"
          className="flex items-center gap-1 text-xs text-emerald-400"
          title={`Ahead of upstream by ${gitStatus.ahead} commit${gitStatus.ahead === 1 ? '' : 's'}`}
        >
          <span aria-hidden>+</span>
          <span>{gitStatus.ahead}</span>
        </span>
      );
    }

    if (gitStatus.behind > 0) {
      indicatorItems.push(
        <span
          key="behind"
          className="flex items-center gap-1 text-xs text-rose-400"
          title={`Behind upstream by ${gitStatus.behind} commit${gitStatus.behind === 1 ? '' : 's'}`}
        >
          <span aria-hidden>-</span>
          <span>{gitStatus.behind}</span>
        </span>
      );
    }

    const branchTitle = `Repository: ${gitStatus.repositoryRoot}`;

    const targetUrl = gitStatus.remoteBranchUrl ?? gitStatus.remoteUrl;

    const branchNode = targetUrl ? (
      <button
        type="button"
        onClick={handleOpenRemote}
        className="text-app-text font-medium transition-colors hover:text-app-text/90 focus:outline-none focus-visible:ring-1 focus-visible:ring-sky-300/40"
        title={`${branchTitle}\nClick to open remote`}
      >
        {branchLabel}
      </button>
    ) : (
      <span className="text-app-text font-medium" title={branchTitle}>
        {branchLabel}
      </span>
    );

    const remoteLabel = remoteHost === 'github.com' ? 'GitHub' : (remoteHost ?? 'Remote');

    const remoteButton = targetUrl ? (
      <button
        type="button"
        onClick={handleOpenRemote}
        className="inline-flex items-center gap-1 rounded-sm px-1.5 py-0.5 text-[11px] font-medium text-app-muted transition-colors hover:text-app-text focus:outline-none focus-visible:ring-1 focus-visible:ring-sky-300/40"
        title={`Open ${targetUrl}`}
        aria-label={`Open remote ${remoteHost ?? 'repository'} in browser`}
      >
        <span>{remoteLabel}</span>
        <ArrowSquareOut size={12} weight="bold" />
      </button>
    ) : null;

    return (
      <div className="flex items-baseline gap-1 text-xs text-app-muted">
        <span>Git</span>
        <div className="flex items-center gap-2 text-app-text font-medium">
          {branchNode}
          {remoteButton}
          {indicatorItems.length > 0 ? (
            <div className="flex items-center gap-2 text-xs font-normal text-app-muted/90">
              {indicatorItems}
            </div>
          ) : null}
        </div>
      </div>
    );
  }, [gitStatus, gitStatusError, gitStatusLoading, handleOpenRemote]);

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
        {gitSegment}
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
