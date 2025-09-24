import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { ArchiveProgressPayload, ArchiveProgressUpdatePayload } from '@/types';
import { useArchiveProgressStore } from '@/store/useArchiveProgressStore';
import QuickTooltip from '@/components/QuickTooltip';

const CONTAINER_TOP_PAD = '3rem';

export default function ArchiveProgressWindow() {
  const windowRef = getCurrentWindow();
  const {
    archiveName,
    destinationDir,
    format,
    currentEntry,
    entries,
    finished,
    setContext,
    pushUpdate,
    reset,
  } = useArchiveProgressStore();

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let unlistenUpdate: (() => void) | undefined;
    let readyNotified = false;

    (async () => {
      try {
        unlisten = await listen<ArchiveProgressPayload>(ARCHIVE_PROGRESS_EVENT, (event) => {
          if (event.payload) {
            setContext(event.payload);
          }
        });
      } catch (error) {
        console.warn('Failed to listen for archive progress events:', error);
      }

      try {
        unlistenUpdate = await listen<ArchiveProgressUpdatePayload>(
          ARCHIVE_PROGRESS_UPDATE_EVENT,
          (event) => {
            if (event.payload) {
              pushUpdate(event.payload);
            }
          }
        );
      } catch (error) {
        console.warn('Failed to listen for archive progress updates:', error);
      }

      try {
        await invoke('archive_progress_window_ready');
        readyNotified = true;
      } catch (error) {
        console.warn('Failed to notify archive progress readiness:', error);
      }
    })();

    return () => {
      if (unlisten) {
        unlisten();
      }
      if (unlistenUpdate) {
        unlistenUpdate();
      }
      if (readyNotified) {
        void invoke('archive_progress_window_unready').catch((error) => {
          console.warn('Failed to reset archive progress readiness:', error);
        });
      }
      reset();
    };
  }, [pushUpdate, reset, setContext]);

  useEffect(() => {
    if (!finished) return;
    const timer = window.setTimeout(() => {
      void windowRef.close().catch((error) => {
        console.warn('Failed to close archive progress window:', error);
      });
    }, 1200);
    return () => window.clearTimeout(timer);
  }, [finished, windowRef]);

  const message = archiveName ? `Extracting ${archiveName}` : 'Extracting archive';
  const formatLabel = format ? format.toUpperCase() : undefined;
  const currentEntryLabel = currentEntry ?? 'Preparing files…';

  return (
    <div className="min-h-screen bg-app-dark text-app-text">
      <div
        className="relative mx-auto flex h-full max-w-md flex-col gap-5 px-6 pb-8"
        style={{ paddingTop: CONTAINER_TOP_PAD }}
      >
        <div data-tauri-drag-region className="absolute inset-x-2 top-0 h-10 rounded-lg" />
        <div className="flex items-start gap-3">
          <div className="w-10 h-10 border-2 border-app-border border-t-transparent rounded-full animate-spin" />
          <div className="flex-1 min-w-0 text-sm space-y-1 overflow-hidden">
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
            {destinationDir && (
              <QuickTooltip text={destinationDir}>
                {({ onBlur, onFocus, onMouseEnter, onMouseLeave, ref }) => (
                  <div
                    ref={ref}
                    onMouseEnter={onMouseEnter}
                    onMouseLeave={onMouseLeave}
                    onFocus={onFocus}
                    onBlur={onBlur}
                    className="text-app-muted text-xs truncate"
                  >
                    Destination: {destinationDir}
                  </div>
                )}
              </QuickTooltip>
            )}
            {formatLabel && (
              <div className="text-app-muted text-[11px] uppercase tracking-wide">
                {formatLabel}
              </div>
            )}
          </div>
        </div>
        <div className="flex flex-col gap-3 bg-app-gray/10 border border-app-border/40 rounded-lg p-4 text-xs text-app-text/90 min-h-[176px] overflow-hidden">
          <div>
            <div className="text-app-muted text-[11px] uppercase tracking-wide">
              Currently extracting
            </div>
            <div className="mt-1 w-full min-w-0 text-app-text min-h-[2.5rem] flex items-center overflow-hidden">
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
          {entries.length > 0 && (
            <div className="border-t border-app-border/30 pt-3">
              <div className="text-app-muted text-[11px] uppercase tracking-wide mb-2">
                Recently extracted
              </div>
              <div className="max-h-40 overflow-y-auto overflow-x-hidden pr-1 space-y-1">
                {entries
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
        {finished && <div className="text-center text-xs text-app-muted">Finishing up…</div>}
      </div>
    </div>
  );
}

const ARCHIVE_PROGRESS_EVENT = 'archive-progress:init';
const ARCHIVE_PROGRESS_UPDATE_EVENT = 'archive-progress:update';
