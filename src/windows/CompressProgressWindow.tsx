import { useEffect, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { CheckCircle, WarningCircle } from 'phosphor-react';
import type { CompressProgressPayload } from '@/types';
import { useCompressProgressStore } from '@/store/useCompressProgressStore';
import QuickTooltip from '@/components/QuickTooltip';
import { WINDOW_CONTENT_TOP_PADDING } from '@/windows/windowLayout';

const COMPRESS_PROGRESS_INIT_EVENT = 'compress-progress:init';
const COMPRESS_PROGRESS_UPDATE_EVENT = 'compress-progress:update';

export default function CompressProgressWindow() {
  const windowRef = getCurrentWindow();
  const {
    archiveName,
    entries,
    currentEntry,
    completed,
    total,
    finished,
    error,
    setContext,
    pushUpdate,
    reset,
  } = useCompressProgressStore();

  useEffect(() => {
    let unlistenInit: (() => void) | undefined;
    let unlistenUpdate: (() => void) | undefined;
    let readyNotified = false;

    (async () => {
      try {
        unlistenInit = await listen<CompressProgressPayload>(
          COMPRESS_PROGRESS_INIT_EVENT,
          (event) => {
            if (event.payload) {
              setContext(event.payload);
            }
          }
        );
      } catch (listenErr) {
        console.warn('Failed to listen for compress progress context:', listenErr);
      }

      try {
        unlistenUpdate = await listen<CompressProgressPayload>(
          COMPRESS_PROGRESS_UPDATE_EVENT,
          (event) => {
            if (event.payload) {
              pushUpdate(event.payload);
            }
          }
        );
      } catch (updateErr) {
        console.warn('Failed to listen for compress progress updates:', updateErr);
      }

      try {
        await invoke('compress_progress_window_ready');
        readyNotified = true;
      } catch (readyErr) {
        console.warn('Failed to notify compress progress readiness:', readyErr);
      }
    })();

    return () => {
      if (unlistenInit) {
        unlistenInit();
      }
      if (unlistenUpdate) {
        unlistenUpdate();
      }
      if (readyNotified) {
        void invoke('compress_progress_window_unready').catch((err) => {
          console.warn('Failed to reset compress progress readiness:', err);
        });
      }
      reset();
    };
  }, [pushUpdate, reset, setContext]);

  useEffect(() => {
    if (!finished) return;
    const timer = window.setTimeout(() => {
      void windowRef.close().catch((closeErr) => {
        console.warn('Failed to close compress progress window:', closeErr);
      });
    }, 1200);
    return () => window.clearTimeout(timer);
  }, [finished, windowRef]);

  const progressRatio = total > 0 ? Math.min(completed / total, 1) : 0;
  const currentEntryLabel = currentEntry ?? 'Preparing files...';
  const statusLabel = error
    ? 'Compression failed'
    : finished
      ? 'Compression complete'
      : total > 0
        ? `Compressing ${Math.min(completed + 1, total)} of ${total}`
        : 'Preparing compression';
  const message = archiveName ? `Creating ${archiveName}` : 'Creating ZIP archive';
  const recentEntries = useMemo(() => entries.slice(-8).reverse(), [entries]);

  return (
    <div className="min-h-screen bg-app-dark text-app-text">
      <div
        className="relative mx-auto flex h-full max-w-md flex-col gap-5 px-6 pb-8"
        style={{ paddingTop: WINDOW_CONTENT_TOP_PADDING }}
      >
        <div data-tauri-drag-region className="absolute inset-x-2 top-0 h-10 rounded-lg" />
        <header className="flex items-start gap-3">
          <div className="w-10 h-10 border-2 border-app-border border-t-transparent rounded-full animate-spin" />
          <div className="flex-1 min-w-0 space-y-1 text-sm">
            <QuickTooltip text={message}>
              {({ onBlur, onFocus, onMouseEnter, onMouseLeave, ref }) => (
                <div
                  ref={ref}
                  onMouseEnter={onMouseEnter}
                  onMouseLeave={onMouseLeave}
                  onFocus={onFocus}
                  onBlur={onBlur}
                  className="font-medium truncate"
                >
                  {message}
                </div>
              )}
            </QuickTooltip>
            <div className="text-xs text-app-muted">{statusLabel}</div>
            {finished && !error && (
              <div className="text-xs text-emerald-400 flex items-center gap-1">
                <CheckCircle weight="duotone" className="h-4 w-4" />
                <span>Archive ready.</span>
              </div>
            )}
            {error && (
              <div className="text-xs text-red-400 flex items-center gap-1">
                <WarningCircle weight="duotone" className="h-4 w-4" />
                <span>{error}</span>
              </div>
            )}
          </div>
        </header>

        <div className="bg-app-gray/10 border border-app-border/40 rounded-lg p-4 text-xs space-y-3">
          <div>
            <div className="text-[11px] uppercase tracking-wide text-app-muted">
              Overall progress
            </div>
            <div className="mt-2 h-2 rounded-full bg-app-gray/40 overflow-hidden">
              <div
                className="h-full rounded-full bg-[var(--accent)] transition-all duration-200"
                style={{ width: `${progressRatio * 100}%` }}
              />
            </div>
            <div className="mt-1 text-app-muted">
              {total > 0 ? `${completed} of ${total} items compressed` : 'Preparing files'}
            </div>
          </div>

          <div>
            <div className="text-[11px] uppercase tracking-wide text-app-muted">
              Currently compressing
            </div>
            <div className="mt-1 min-h-[2.5rem] flex items-center overflow-hidden">
              <QuickTooltip text={currentEntryLabel}>
                {({ onBlur, onFocus, onMouseEnter, onMouseLeave, ref }) => (
                  <span
                    ref={ref}
                    onMouseEnter={onMouseEnter}
                    onMouseLeave={onMouseLeave}
                    onFocus={onFocus}
                    onBlur={onBlur}
                    className="truncate"
                  >
                    {currentEntryLabel}
                  </span>
                )}
              </QuickTooltip>
            </div>
          </div>

          {recentEntries.length > 0 && (
            <div className="border-t border-app-border/30 pt-3">
              <div className="text-[11px] uppercase tracking-wide text-app-muted mb-2">
                Recently compressed
              </div>
              <div className="max-h-40 overflow-y-auto overflow-x-hidden pr-1 space-y-1">
                {recentEntries.map((entry, idx) => (
                  <div key={`${entry}-${idx}`} className="min-w-0">
                    <QuickTooltip text={entry}>
                      {({ onBlur, onFocus, onMouseEnter, onMouseLeave, ref }) => (
                        <div
                          ref={ref}
                          onMouseEnter={onMouseEnter}
                          onMouseLeave={onMouseLeave}
                          onFocus={onFocus}
                          onBlur={onBlur}
                          className="text-app-text/80 truncate leading-tight"
                        >
                          {entry}
                        </div>
                      )}
                    </QuickTooltip>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
        {finished && !error && (
          <div className="text-center text-xs text-app-muted">Finishing up...</div>
        )}
      </div>
    </div>
  );
}
