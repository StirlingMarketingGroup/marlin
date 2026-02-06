import { useEffect, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { CheckCircle, WarningCircle, XCircle } from 'phosphor-react';
import { FolderSizeInitPayload, FolderSizeProgressPayload } from '@/types';
import { useFolderSizeStore } from '@/store/useFolderSizeStore';
import { WINDOW_CONTENT_TOP_PADDING } from '@/windows/windowLayout';
import { formatBytes } from '@/utils/formatBytes';

const formatNumber = (value: number): string =>
  new Intl.NumberFormat(undefined, { maximumFractionDigits: 0 }).format(value);

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
    let mounted = true;
    let readyNotified = false;

    (async () => {
      try {
        unlistenInit = await listen<FolderSizeInitPayload>('folder-size:init', async (event) => {
          if (!event.payload) {
            console.warn('[FolderSizeWindow] Event payload is empty');
            return;
          }

          reset();
          const { requestId, targets: payloadTargets, autoStart, initialError } = event.payload;
          const mappedTargets = payloadTargets.map((target) => ({
            path: target.path,
            name: target.name,
            isDirectory: target.isDirectory,
          }));

          await initializeAndStart(requestId, mappedTargets, {
            invokeBackend: !autoStart,
            markRunning: autoStart && !initialError,
          });

          if (initialError) {
            applyProgress({
              requestId,
              totalBytes: 0,
              totalApparentBytes: 0,
              totalItems: 0,
              currentPath: null,
              finished: true,
              cancelled: false,
              error: initialError,
            });
          }
        });
      } catch (initError) {
        console.warn('Failed to listen for folder size init event:', initError);
      }

      try {
        unlistenProgress = await listen<FolderSizeProgressPayload>(
          'folder-size-progress',
          (event) => {
            if (event.payload) {
              applyProgress(event.payload);
            }
          }
        );
      } catch (progressError) {
        console.warn('Failed to listen for progress events:', progressError);
      }

      if (!mounted) {
        return;
      }

      try {
        await invoke('folder_size_window_ready');
        readyNotified = true;
      } catch (error) {
        console.warn('Failed to notify folder size readiness:', error);
      }
    })();

    return () => {
      mounted = false;
      if (unlistenInit) {
        unlistenInit();
      }
      if (unlistenProgress) {
        unlistenProgress();
      }
      if (readyNotified) {
        void invoke('folder_size_window_unready').catch((error) => {
          console.warn('Failed to reset folder size readiness:', error);
        });
      }
      reset();
    };
  }, [initializeAndStart, applyProgress, reset, isMacPlatform]);

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
        <div className="flex items-center gap-2 text-sm text-app-muted">
          <XCircle className="h-5 w-5" weight="fill" />
          <span>Scan cancelled</span>
        </div>
      );
    }
    if (isRunning) {
      return (
        <div className="flex items-center gap-2 text-sm text-app-muted">
          <div
            className="h-4 w-4 animate-spin rounded-full border-2 border-app-muted/40 border-t-transparent"
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
        style={{ paddingTop: WINDOW_CONTENT_TOP_PADDING }}
      >
        <div data-tauri-drag-region className="absolute inset-x-2 top-0 h-12 rounded-lg" />
        <div className="absolute inset-x-0 top-0 h-12 flex items-center justify-center pointer-events-none">
          <span className="text-sm font-medium text-app-text">Folder Size</span>
        </div>

        <section className="space-y-3 rounded-xl border border-app-border bg-app-gray/60 p-6 shadow-lg">
          <div className="flex items-center justify-between text-sm text-app-muted">
            <span>Space on disk</span>
            <span className="font-medium text-app-text">
              {formatBytes(totalBytes)} ({formatNumber(totalBytes)} B)
            </span>
          </div>
          <div className="flex items-center justify-between text-sm text-app-muted">
            <span>Logical size</span>
            <span className="font-medium text-app-text">
              {formatBytes(totalApparentBytes)} ({formatNumber(totalApparentBytes)} B)
            </span>
          </div>
          {hasSharedBytes ? (
            <div className="flex items-center justify-between text-xs text-app-muted">
              <span>Shared / sparse data</span>
              <span className="font-medium text-app-text/80">
                {formatBytes(sharedBytes)} ({formatNumber(sharedBytes)} B)
              </span>
            </div>
          ) : null}
          <div className="flex items-center justify-between text-sm text-app-muted">
            <span>Items scanned</span>
            <span className="font-medium text-app-text">{formatNumber(totalItems)}</span>
          </div>
          {lastPath ? (
            <div className="truncate text-xs text-app-muted">
              <span className="uppercase tracking-wide text-[10px] text-app-muted/80">
                Last path:
              </span>
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
              className="rounded-md bg-app-gray/60 px-3 py-2 text-sm text-app-text transition hover:bg-app-gray/80 disabled:cursor-not-allowed disabled:opacity-60"
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
