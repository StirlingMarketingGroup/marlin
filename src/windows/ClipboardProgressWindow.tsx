import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { ClipboardProgressPayload, ClipboardProgressUpdatePayload } from '@/types';
import { useClipboardProgressStore } from '@/store/useClipboardProgressStore';
import QuickTooltip from '@/components/QuickTooltip';
import { WINDOW_CONTENT_TOP_PADDING } from '@/windows/windowLayout';

const CLIPBOARD_PROGRESS_EVENT = 'clipboard-progress:init';
const CLIPBOARD_PROGRESS_UPDATE_EVENT = 'clipboard-progress:update';

export default function ClipboardProgressWindow() {
  const windowRef = getCurrentWindow();
  const {
    operation,
    destination,
    totalItems,
    completed,
    currentItem,
    recentItems,
    finished,
    error,
    setContext,
    pushUpdate,
    reset,
  } = useClipboardProgressStore();

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let unlistenUpdate: (() => void) | undefined;
    let readyNotified = false;

    (async () => {
      try {
        unlisten = await listen<ClipboardProgressPayload>(CLIPBOARD_PROGRESS_EVENT, (event) => {
          if (event.payload) {
            setContext(event.payload);
          }
        });
      } catch (e) {
        console.warn('Failed to listen for clipboard progress init:', e);
      }

      try {
        unlistenUpdate = await listen<ClipboardProgressUpdatePayload>(
          CLIPBOARD_PROGRESS_UPDATE_EVENT,
          (event) => {
            if (event.payload) {
              pushUpdate(event.payload);
            }
          }
        );
      } catch (e) {
        console.warn('Failed to listen for clipboard progress updates:', e);
      }

      try {
        await invoke('clipboard_progress_window_ready');
        readyNotified = true;
      } catch (e) {
        console.warn('Failed to notify clipboard progress readiness:', e);
      }
    })();

    return () => {
      if (unlisten) unlisten();
      if (unlistenUpdate) unlistenUpdate();
      if (readyNotified) {
        void invoke('clipboard_progress_window_unready').catch((e) => {
          console.warn('Failed to reset clipboard progress readiness:', e);
        });
      }
      reset();
    };
  }, [pushUpdate, reset, setContext]);

  useEffect(() => {
    if (!finished) return;
    const timer = window.setTimeout(() => {
      void windowRef.close().catch((e) => console.warn('Failed to close clipboard progress', e));
    }, 1200);
    return () => window.clearTimeout(timer);
  }, [finished, windowRef]);

  const total = Math.max(0, totalItems || 0);
  const done = Math.max(0, completed || 0);
  const percent = total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0;

  const title = operation ? operation : 'Working…';
  const destinationLabel = destination ?? '';
  const currentLabel = currentItem ?? (finished ? 'Finishing up…' : 'Preparing…');

  return (
    <div className="min-h-screen bg-app-dark text-app-text">
      <div
        className="relative mx-auto flex h-full max-w-md flex-col gap-4 px-6 pb-7"
        style={{ paddingTop: WINDOW_CONTENT_TOP_PADDING }}
      >
        <div data-tauri-drag-region className="absolute inset-x-2 top-0 h-10 rounded-lg" />

        <div className="flex items-start gap-3">
          <div
            className={`w-10 h-10 border-2 border-app-border rounded-full ${finished ? 'border-app-border/40' : 'border-t-transparent animate-spin'}`}
          />
          <div className="flex-1 min-w-0 text-sm space-y-1 overflow-hidden">
            <QuickTooltip text={title}>
              {({ onBlur, onFocus, onMouseEnter, onMouseLeave, ref }) => (
                <div
                  ref={ref}
                  onMouseEnter={onMouseEnter}
                  onMouseLeave={onMouseLeave}
                  onFocus={onFocus}
                  onBlur={onBlur}
                  className="font-medium truncate"
                >
                  {title}
                </div>
              )}
            </QuickTooltip>
            {destinationLabel && (
              <QuickTooltip text={destinationLabel}>
                {({ onBlur, onFocus, onMouseEnter, onMouseLeave, ref }) => (
                  <div
                    ref={ref}
                    onMouseEnter={onMouseEnter}
                    onMouseLeave={onMouseLeave}
                    onFocus={onFocus}
                    onBlur={onBlur}
                    className="text-app-muted text-xs truncate"
                  >
                    Destination: {destinationLabel}
                  </div>
                )}
              </QuickTooltip>
            )}
          </div>
        </div>

        <div className="space-y-2">
          <div className="flex items-center justify-between text-xs text-app-muted">
            <div>
              {total > 0 ? (
                <>
                  {done}/{total}
                </>
              ) : (
                'Working…'
              )}
            </div>
            {total > 0 && <div>{percent}%</div>}
          </div>
          <div className="h-2 w-full rounded-full bg-app-gray/20 overflow-hidden border border-app-border/30">
            <div
              className="h-full bg-accent transition-[width] duration-150"
              style={{ width: `${total > 0 ? percent : 12}%` }}
            />
          </div>
        </div>

        <div className="flex flex-col gap-3 bg-app-gray/10 border border-app-border/40 rounded-lg p-4 text-xs text-app-text/90 min-h-[176px] overflow-hidden">
          <div>
            <div className="text-app-muted text-[11px] uppercase tracking-wide">
              Currently processing
            </div>
            <div className="mt-1 w-full min-w-0 text-app-text min-h-[2.25rem] flex items-center overflow-hidden">
              <QuickTooltip text={currentLabel}>
                {({ onBlur, onFocus, onMouseEnter, onMouseLeave, ref }) => (
                  <span
                    ref={ref}
                    onMouseEnter={onMouseEnter}
                    onMouseLeave={onMouseLeave}
                    onFocus={onFocus}
                    onBlur={onBlur}
                    className="truncate"
                  >
                    {currentLabel}
                  </span>
                )}
              </QuickTooltip>
            </div>
          </div>

          {error && (
            <div className="border-t border-app-border/30 pt-3 text-app-red/90">
              <div className="text-app-muted text-[11px] uppercase tracking-wide mb-1">Error</div>
              <div className="truncate">{error}</div>
            </div>
          )}

          {recentItems.length > 0 && (
            <div className="border-t border-app-border/30 pt-3">
              <div className="text-app-muted text-[11px] uppercase tracking-wide mb-2">Recent</div>
              <div className="max-h-40 overflow-y-auto overflow-x-hidden pr-1 space-y-1">
                {[...recentItems]
                  .slice(-8)
                  .reverse()
                  .map((entry, idx) => (
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

        {finished && <div className="text-center text-xs text-app-muted">Done</div>}
      </div>
    </div>
  );
}
