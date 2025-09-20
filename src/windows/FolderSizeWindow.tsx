import { useEffect, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { CheckCircle, WarningCircle, XCircle } from 'phosphor-react';
import { FolderSizeInitPayload, FolderSizeProgressPayload } from '@/types';
import { useFolderSizeStore } from '@/store/useFolderSizeStore';

const formatBytes = (bytes: number): string => {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / Math.pow(1024, exponent);
  return `${value.toFixed(value >= 10 || exponent === 0 ? 0 : 1)} ${units[exponent]}`;
};

const formatNumber = (value: number): string =>
  new Intl.NumberFormat(undefined, { maximumFractionDigits: 0 }).format(value);

const CONTAINER_TOP_PAD = '3rem';

export default function FolderSizeWindow() {
  const windowRef = getCurrentWindow();
  const isMacPlatform = useMemo(() => /mac/i.test(navigator.userAgent), []);

  const {
    totalBytes,
    totalApparentBytes,
    totalItems,
    lastPath,
    isRunning,
    cancelRequested,
    cancelled,
    error,
    initializeAndStart,
    applyProgress,
    cancel,
    reset,
  } = useFolderSizeStore();

  useEffect(() => {
    let unlistenInit: (() => void) | undefined;
    let unlistenProgress: (() => void) | undefined;

    console.log('[FolderSizeWindow] Component mounted, setting up event listeners...');

    (async () => {
      try {
        unlistenInit = await listen<FolderSizeInitPayload>('folder-size:init', async (event) => {
          console.log('[FolderSizeWindow] Received folder-size:init event:', event.payload);
          if (!event.payload) {
            console.warn('[FolderSizeWindow] Event payload is empty');
            return;
          }
          reset();
          const { requestId, targets: payloadTargets } = event.payload;
          console.log(
            `[FolderSizeWindow] Starting calculation for ${payloadTargets.length} targets`
          );
          await initializeAndStart(
            requestId,
            payloadTargets.map((target) => ({
              path: target.path,
              name: target.name,
              isDirectory: target.isDirectory,
            }))
          );
        });
        console.log('[FolderSizeWindow] Successfully set up folder-size:init listener');
      } catch (initError) {
        console.warn('Failed to listen for folder size init event:', initError);
      }

      try {
        unlistenProgress = await listen<FolderSizeProgressPayload>(
          'folder-size-progress',
          (event) => {
            if (event.payload) {
              console.log(
                '[FolderSizeWindow] Progress update:',
                event.payload.totalBytes,
                'physical bytes,',
                event.payload.totalApparentBytes,
                'logical bytes,',
                event.payload.totalItems,
                'items'
              );
              applyProgress(event.payload);
            }
          }
        );
        console.log('[FolderSizeWindow] Successfully set up folder-size-progress listener');
      } catch (progressError) {
        console.warn('Failed to listen for progress events:', progressError);
      }
    })();

    return () => {
      if (unlistenInit) {
        unlistenInit();
      }
      if (unlistenProgress) {
        unlistenProgress();
      }
      reset();
    };
  }, [initializeAndStart, applyProgress, reset, isMacPlatform]);

  useEffect(() => {
    void invoke('folder_size_window_ready').catch((error) => {
      console.warn('Failed to notify folder size readiness:', error);
    });
    return () => {
      void invoke('folder_size_window_unready').catch((error) => {
        console.warn('Failed to reset folder size readiness:', error);
      });
    };
  }, []);

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        if (isRunning && !cancelRequested) {
          void cancel();
        }
        void windowRef.close();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [windowRef, isRunning, cancelRequested, cancel]);

  const statusIndicator = () => {
    if (error) {
      return (
        <div className="flex items-center gap-2 text-sm text-red-400">
          <WarningCircle className="h-5 w-5" weight="fill" />
          <span>{error}</span>
        </div>
      );
    }
    if (cancelled) {
      return (
        <div className="flex items-center gap-2 text-sm text-white/60">
          <XCircle className="h-5 w-5" weight="fill" />
          <span>Scan cancelled</span>
        </div>
      );
    }
    if (isRunning) {
      return (
        <div className="flex items-center gap-2 text-sm text-white/70">
          <div
            className="h-4 w-4 animate-spin rounded-full border-2 border-white/30 border-t-transparent"
            aria-hidden
          />
          <span>{cancelRequested ? 'Cancelling…' : 'Scanning directories…'}</span>
        </div>
      );
    }
    return (
      <div className="flex items-center gap-2 text-sm text-emerald-400">
        <CheckCircle className="h-5 w-5" weight="fill" />
        <span>Scan completed</span>
      </div>
    );
  };

  const sharedBytes = Math.max(totalApparentBytes - totalBytes, 0);
  const hasSharedBytes = sharedBytes > 0;

  return (
    <div className="min-h-screen bg-app-dark text-app-text">
      <div
        className="relative mx-auto flex h-full max-w-lg flex-col gap-6 px-6 pb-10"
        style={{ paddingTop: CONTAINER_TOP_PAD }}
      >
        <div data-tauri-drag-region className="absolute inset-x-2 top-0 h-12 rounded-lg" />
        <div className="absolute inset-x-0 top-0 h-12 flex items-center justify-center pointer-events-none">
          <span className="text-sm font-medium text-white/90">Folder Size</span>
        </div>

        <section className="space-y-3 rounded-xl border border-white/10 bg-[rgba(34,34,36,0.92)] p-6 shadow-[0_24px_70px_rgba(0,0,0,0.45)]">
          <div className="flex items-center justify-between text-sm text-white/80">
            <span>Space on disk</span>
            <span className="font-medium text-white">
              {formatBytes(totalBytes)} ({formatNumber(totalBytes)} B)
            </span>
          </div>
          <div className="flex items-center justify-between text-sm text-white/80">
            <span>Logical size</span>
            <span className="font-medium text-white">
              {formatBytes(totalApparentBytes)} ({formatNumber(totalApparentBytes)} B)
            </span>
          </div>
          {hasSharedBytes ? (
            <div className="flex items-center justify-between text-xs text-white/60">
              <span>Shared / sparse data</span>
              <span className="font-medium text-white/70">
                {formatBytes(sharedBytes)} ({formatNumber(sharedBytes)} B)
              </span>
            </div>
          ) : null}
          <div className="flex items-center justify-between text-sm text-white/80">
            <span>Items scanned</span>
            <span className="font-medium text-white">{formatNumber(totalItems)}</span>
          </div>
          {lastPath ? (
            <div className="truncate text-xs text-white/50">
              <span className="uppercase tracking-wide text-[10px] text-white/50">Last path:</span>
              <div className="truncate" title={lastPath}>
                {lastPath}
              </div>
            </div>
          ) : null}
          {statusIndicator()}
        </section>

        <footer className="flex flex-wrap items-center justify-end gap-2">
          {isRunning || cancelRequested ? (
            <button
              type="button"
              onClick={() => {
                if (!cancelRequested) {
                  void cancel();
                }
              }}
              className="rounded-md bg-white/10 px-3 py-2 text-sm text-white transition hover:bg-white/20 disabled:cursor-not-allowed disabled:opacity-60"
              disabled={cancelRequested}
            >
              {cancelRequested ? 'Cancelling…' : 'Cancel'}
            </button>
          ) : null}
        </footer>
      </div>
    </div>
  );
}
