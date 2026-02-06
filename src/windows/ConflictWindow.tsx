import { useEffect, useRef, useCallback } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { X } from 'phosphor-react';
import type {
  ConflictPayload,
  ConflictAction,
  ConflictResolution,
  ConflictFileInfo,
  FileItem,
} from '@/types';
import { useConflictStore } from '@/store/useConflictStore';
import { useThumbnail } from '@/hooks/useThumbnail';
import { useFileIcon } from '@/hooks/useFileIcon';
import { usePlatform } from '@/hooks/usePlatform';
import { isVideoExtension } from '@/utils/fileTypes';
import { formatBytes } from '@/utils/formatBytes';
import { WINDOW_CONTENT_TOP_PADDING } from '@/windows/windowLayout';

const CONFLICT_INIT_EVENT = 'conflict:init';

const THUMBNAIL_IMAGE_EXTS = new Set([
  'jpg',
  'jpeg',
  'png',
  'gif',
  'webp',
  'bmp',
  'tiff',
  'tga',
  'ico',
  'svg',
]);

function shouldLoadThumbnail(ext?: string | null): boolean {
  if (!ext) return false;
  const e = ext.toLowerCase();
  return (
    THUMBNAIL_IMAGE_EXTS.has(e) ||
    e === 'pdf' ||
    e === 'ai' ||
    e === 'eps' ||
    e === 'psd' ||
    e === 'psb' ||
    e === 'stl' ||
    e === 'ttf' ||
    e === 'otf' ||
    isVideoExtension(e)
  );
}

function toFileItem(info: ConflictFileInfo): FileItem {
  return {
    name: info.name,
    path: info.path,
    size: info.size,
    modified: info.modified,
    is_directory: info.isDirectory,
    is_hidden: false,
    is_symlink: false,
    is_git_repo: false,
    extension: info.extension ?? undefined,
  };
}

function formatDate(iso: string): string {
  const date = new Date(iso);
  if (isNaN(date.getTime())) return iso;
  return new Intl.DateTimeFormat(undefined, {
    year: 'numeric',
    month: 'short',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  }).format(date);
}

function FilePreview({ info }: { info: ConflictFileInfo }) {
  const { isMac } = usePlatform();
  const getFileIcon = useFileIcon('large', isMac);

  const ext =
    info.extension?.toLowerCase() ??
    (info.name.includes('.') ? info.name.split('.').pop()?.toLowerCase() : undefined);
  const loadThumbnail = !info.isDirectory && shouldLoadThumbnail(ext);

  const { dataUrl, loading } = useThumbnail(loadThumbnail ? info.path : undefined, {
    size: 96,
    quality: 'medium',
    priority: 'high',
  });

  const containerClass =
    'flex h-20 w-20 items-center justify-center rounded-lg bg-app-gray/20 border border-app-border/30';

  if (loadThumbnail && loading) {
    return (
      <div className={containerClass}>
        <div className="h-6 w-6 animate-spin rounded-full border-2 border-app-border border-t-transparent" />
      </div>
    );
  }

  if (dataUrl) {
    return (
      <div className={`${containerClass} overflow-hidden`}>
        <img src={dataUrl} alt={info.name} className="max-h-full max-w-full object-contain" />
      </div>
    );
  }

  return <div className={containerClass}>{getFileIcon(toFileItem(info))}</div>;
}

interface FileComparisonProps {
  label: string;
  info: ConflictFileInfo;
}

function FileComparison({ label, info }: FileComparisonProps) {
  return (
    <div className="flex-1 min-w-0 flex flex-col items-center gap-2">
      <div className="text-[11px] uppercase tracking-wide text-app-muted">{label}</div>
      <FilePreview info={info} />
      <div className="w-full min-w-0 text-center space-y-0.5">
        <div className="text-sm font-medium truncate" title={info.name}>
          {info.name}
        </div>
        <div className="text-xs text-app-muted">
          {info.isDirectory ? 'Folder' : formatBytes(info.size)}
        </div>
        <div className="text-xs text-app-muted">{formatDate(info.modified)}</div>
      </div>
    </div>
  );
}

const FILE_ACTIONS: { value: ConflictAction; label: string; description: string }[] = [
  { value: 'replace', label: 'Replace', description: 'Overwrite the existing file' },
  { value: 'skip', label: 'Skip', description: 'Keep the existing file, skip this one' },
  { value: 'keepBoth', label: 'Keep Both', description: 'Auto-rename the new file' },
  { value: 'rename', label: 'Rename to...', description: 'Choose a custom name' },
];

const FOLDER_ACTIONS: { value: ConflictAction; label: string; description: string }[] = [
  { value: 'merge', label: 'Merge', description: 'Combine folder contents' },
  { value: 'replace', label: 'Replace', description: 'Replace the entire folder' },
  { value: 'skip', label: 'Skip', description: 'Keep the existing folder' },
  { value: 'keepBoth', label: 'Keep Both', description: 'Auto-rename the new folder' },
  { value: 'rename', label: 'Rename to...', description: 'Choose a custom name' },
];

export default function ConflictWindow() {
  const windowRef = getCurrentWindow();
  const renameInputRef = useRef<HTMLInputElement>(null);
  const {
    conflict,
    customName,
    applyToAll,
    selectedAction,
    setConflict,
    setCustomName,
    setApplyToAll,
    setSelectedAction,
    reset,
  } = useConflictStore();

  // Event handshake: listen for conflict payloads and signal readiness
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    let readyNotified = false;

    (async () => {
      try {
        const unlistenFn = await listen<ConflictPayload>(CONFLICT_INIT_EVENT, (event) => {
          if (event.payload) {
            setConflict(event.payload);
          }
        });
        if (cancelled) {
          // Unmounted before listen resolved — clean up immediately
          unlistenFn();
          return;
        }
        unlisten = unlistenFn;
      } catch (e) {
        console.warn('Failed to listen for conflict init:', e);
      }

      if (cancelled) return;

      try {
        await invoke('conflict_window_ready');
        readyNotified = true;
      } catch (e) {
        console.warn('Failed to notify conflict readiness:', e);
      }
    })();

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
      if (readyNotified) {
        void invoke('conflict_window_unready').catch((e) => {
          console.warn('Failed to reset conflict readiness:', e);
        });
      }
      reset();
    };
  }, [reset, setConflict]);

  // Focus rename input when rename is selected
  useEffect(() => {
    if (selectedAction === 'rename' && renameInputRef.current) {
      renameInputRef.current.focus();
      renameInputRef.current.select();
    }
  }, [selectedAction]);

  const handleContinue = useCallback(() => {
    if (!conflict || !selectedAction) return;

    // Validate rename has a non-empty name
    if (selectedAction === 'rename') {
      const trimmed = customName.trim();
      if (!trimmed || trimmed.includes('/') || trimmed.includes('\\')) return;
    }

    const resolution: ConflictResolution = {
      conflictId: conflict.conflictId,
      action: selectedAction,
      customName: selectedAction === 'rename' ? customName.trim() : null,
      applyToAll,
    };

    invoke('resolve_conflict', { resolution }).catch((e) => {
      console.warn('Failed to resolve conflict:', e);
    });
  }, [conflict, selectedAction, customName, applyToAll]);

  const handleCancel = useCallback(() => {
    void windowRef.close().catch((e) => {
      console.warn('Failed to close conflict window:', e);
    });
  }, [windowRef]);

  // Keyboard shortcuts
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Enter' && selectedAction) {
        // Don't submit on Enter while typing in the rename input
        if (selectedAction === 'rename' && document.activeElement === renameInputRef.current) {
          // Only submit on Enter if the name is valid
          const trimmed = customName.trim();
          if (trimmed && !trimmed.includes('/') && !trimmed.includes('\\')) {
            e.preventDefault();
            handleContinue();
          }
          return;
        }
        e.preventDefault();
        handleContinue();
      } else if (e.key === 'Escape') {
        e.preventDefault();
        handleCancel();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [handleContinue, handleCancel, selectedAction, customName]);

  if (!conflict) {
    return (
      <div className="flex min-h-screen flex-col bg-app-dark text-app-text">
        {/* Titlebar drag region */}
        <div
          data-tauri-drag-region
          className="shrink-0"
          style={{ height: WINDOW_CONTENT_TOP_PADDING }}
        />
        <div className="flex flex-1 flex-col items-center justify-center gap-4 pb-12">
          <div className="h-8 w-8 animate-spin rounded-full border-2 border-app-border border-t-transparent" />
          <div className="text-sm text-app-muted">Waiting for conflict...</div>
        </div>
      </div>
    );
  }

  const bothDirectories = conflict.source.isDirectory && conflict.destination.isDirectory;
  const actions = bothDirectories ? FOLDER_ACTIONS : FILE_ACTIONS;
  const heading = bothDirectories ? 'A folder already exists' : 'A file already exists';

  return (
    <div className="flex min-h-screen flex-col bg-app-dark text-app-text">
      {/* Titlebar — draggable, title sits next to traffic lights */}
      <div
        data-tauri-drag-region
        className="flex shrink-0 items-center justify-between pl-20 pr-3"
        style={{ height: WINDOW_CONTENT_TOP_PADDING }}
      >
        <h1 className="text-sm font-semibold" data-tauri-drag-region>
          {heading}
        </h1>
        <div className="flex items-center gap-3" data-tauri-drag-region>
          {conflict.remainingItems > 0 && (
            <span className="text-xs text-app-muted">{conflict.remainingItems} remaining</span>
          )}
          <button
            type="button"
            onClick={handleCancel}
            className="p-1.5 rounded hover:bg-app-light/50 text-app-muted hover:text-app-text transition-colors"
            aria-label="Close"
            data-tauri-drag-region={false}
          >
            <X className="h-4 w-4" weight="bold" />
          </button>
        </div>
      </div>

      {/* Content below titlebar */}
      <div className="mx-auto flex max-w-lg flex-col gap-4 px-6 pb-5">
        {/* Side-by-side comparison */}
        <div className="flex gap-4 rounded-lg bg-app-gray/10 border border-app-border/40 p-4">
          <FileComparison label="Incoming" info={conflict.source} />
          <div className="flex items-center text-app-muted text-lg">&rarr;</div>
          <FileComparison label="Existing" info={conflict.destination} />
        </div>

        {/* Action selection */}
        <div className="space-y-1.5">
          {actions.map(({ value, label, description }) => {
            const isRename = value === 'rename';
            const disabled = isRename && applyToAll;

            return (
              <label
                key={value}
                className={`flex items-start gap-2.5 rounded-md px-3 py-2 text-sm transition-colors cursor-pointer ${
                  disabled
                    ? 'opacity-40 cursor-not-allowed'
                    : selectedAction === value
                      ? 'bg-[var(--accent)]/10'
                      : 'hover:bg-app-gray/10'
                }`}
              >
                <input
                  type="radio"
                  name="conflict-action"
                  value={value}
                  checked={selectedAction === value}
                  disabled={disabled}
                  onChange={() => setSelectedAction(value)}
                  className="mt-0.5 accent-[var(--accent)]"
                />
                <div className="flex-1 min-w-0">
                  <div className="font-medium">{label}</div>
                  <div className="text-xs text-app-muted">{description}</div>
                  {isRename && selectedAction === 'rename' && (
                    <input
                      ref={renameInputRef}
                      type="text"
                      value={customName}
                      onChange={(e) => setCustomName(e.target.value)}
                      className="mt-2 w-full rounded border border-app-border bg-app-dark px-2 py-1 text-sm text-app-text outline-none focus:border-[var(--accent)]"
                      placeholder="Enter new name..."
                    />
                  )}
                </div>
              </label>
            );
          })}
        </div>

        {/* Apply to all */}
        <label className="flex items-center gap-2 px-3 text-sm cursor-pointer">
          <input
            type="checkbox"
            checked={applyToAll}
            onChange={(e) => setApplyToAll(e.target.checked)}
            className="accent-[var(--accent)]"
          />
          <span>Apply to all remaining conflicts</span>
        </label>

        {/* Footer buttons */}
        <div className="flex items-center justify-between pt-1">
          <button
            onClick={handleCancel}
            className="rounded-md px-4 py-1.5 text-sm text-app-muted hover:text-app-text hover:bg-app-gray/20 transition-colors"
          >
            Cancel All
          </button>
          <button
            onClick={handleContinue}
            disabled={!selectedAction}
            className="rounded-md bg-[var(--accent)] px-5 py-1.5 text-sm font-medium text-white disabled:opacity-40 disabled:cursor-not-allowed hover:brightness-110 transition-all"
          >
            Continue
          </button>
        </div>
      </div>
    </div>
  );
}
