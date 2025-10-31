import { useEffect, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { CheckCircle, WarningCircle } from 'phosphor-react';
import type { DeleteProgressPayload, DeleteProgressUpdatePayload } from '@/types';
import { useDeleteProgressStore } from '@/store/useDeleteProgressStore';
import QuickTooltip from '@/components/QuickTooltip';

const CONTAINER_TOP_PAD = '3rem';

const DELETE_PROGRESS_EVENT = 'delete-progress:init';
const DELETE_PROGRESS_UPDATE_EVENT = 'delete-progress:update';

export default function DeleteProgressWindow() {
  const windowRef = getCurrentWindow();
  const {
    requestId,
    items,
    totalItems,
    completed,
    currentPath,
    finished,
    error,
    history,
    setContext,
    applyUpdate,
    reset,
  } = useDeleteProgressStore();

  useEffect(() => {
    let unlistenInit: (() => void) | undefined;
    let unlistenUpdate: (() => void) | undefined;
    let readyNotified = false;

    (async () => {
      try {
        unlistenInit = await listen<DeleteProgressPayload>(DELETE_PROGRESS_EVENT, (event) => {
          if (event.payload) {
            setContext(event.payload);
          }
        });
      } catch (listenErr) {
        console.warn('Failed to listen for delete progress context:', listenErr);
      }

      try {
        unlistenUpdate = await listen<DeleteProgressUpdatePayload>(
          DELETE_PROGRESS_UPDATE_EVENT,
          (event) => {
            if (event.payload) {
              applyUpdate(event.payload);
            }
          }
        );
      } catch (updateErr) {
        console.warn('Failed to listen for delete progress updates:', updateErr);
      }

      try {
        await invoke('delete_progress_window_ready');
        readyNotified = true;
      } catch (readyErr) {
        console.warn('Failed to notify delete progress readiness:', readyErr);
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
        void invoke('delete_progress_window_unready').catch((err) => {
          console.warn('Failed to reset delete progress readiness:', err);
        });
      }
      reset();
    };
  }, [applyUpdate, reset, setContext]);

  useEffect(() => {
    if (!finished) return;
    const timer = window.setTimeout(() => {
      void windowRef.close().catch((closeErr) => {
        console.warn('Failed to close delete progress window:', closeErr);
      });
    }, 1200);
    return () => window.clearTimeout(timer);
  }, [finished, windowRef]);

  const progressRatio = totalItems > 0 ? Math.min(completed / totalItems, 1) : 0;
  const currentItem = useMemo(() => {
    if (!currentPath) return undefined;
    try {
      const normalized = currentPath.replace(/\\+/g, '/');
      const [, last] = normalized.match(/([^/]+)\/?$/) ?? [];
      return last ?? currentPath;
    } catch {
      return currentPath;
    }
  }, [currentPath]);

  const statusLabel = error
    ? 'Delete failed'
    : finished
      ? 'Delete complete'
      : totalItems > 0
        ? `Deleting item ${Math.min(completed + 1, totalItems)} of ${totalItems}`
        : 'Preparing delete';

  return (
    <div className="min-h-screen bg-app-dark text-app-text">
      <div
        className="relative mx-auto flex h-full max-w-md flex-col gap-5 px-6 pb-8"
        style={{ paddingTop: CONTAINER_TOP_PAD }}
      >
        <div data-tauri-drag-region className="absolute inset-x-2 top-0 h-10 rounded-lg" />
        <header className="flex items-start gap-3">
          <div className="w-10 h-10 border-2 border-app-border border-t-transparent rounded-full animate-spin" />
          <div className="flex-1 min-w-0 space-y-1 text-sm">
            <div className="font-medium truncate">{statusLabel}</div>
            {currentItem && !finished && (
              <QuickTooltip text={currentItem}>
                {({ onBlur, onFocus, onMouseEnter, onMouseLeave, ref }) => (
                  <div
                    ref={ref}
                    onMouseEnter={onMouseEnter}
                    onMouseLeave={onMouseLeave}
                    onFocus={onFocus}
                    onBlur={onBlur}
                    className="text-xs text-app-muted truncate"
                  >
                    {currentItem}
                  </div>
                )}
              </QuickTooltip>
            )}
            {finished && (
              <div className="text-xs text-emerald-400 flex items-center gap-1">
                <CheckCircle weight="duotone" className="h-4 w-4" />
                <span>All selected items removed.</span>
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
              {completed} of {totalItems} items removed
            </div>
          </div>

          {items.length > 0 && (
            <div>
              <div className="text-[11px] uppercase tracking-wide text-app-muted mb-2">Items</div>
              <div className="max-h-40 overflow-y-auto space-y-1 pr-1">
                {items.map((item, index) => {
                  const status =
                    index < completed
                      ? 'done'
                      : index === completed && !finished
                        ? 'current'
                        : 'pending';
                  const baseClasses =
                    'flex items-center gap-2 rounded-md px-2 py-1 text-sm transition-colors truncate';
                  const className =
                    status === 'done'
                      ? `${baseClasses} text-emerald-300/80`
                      : status === 'current'
                        ? `${baseClasses} bg-[var(--accent)]/10 text-app-text`
                        : `${baseClasses} text-app-muted`;

                  return (
                    <QuickTooltip key={`${item.path}-${index}`} text={item.name ?? item.path}>
                      {({ onBlur, onFocus, onMouseEnter, onMouseLeave, ref }) => (
                        <div
                          ref={ref}
                          onMouseEnter={onMouseEnter}
                          onMouseLeave={onMouseLeave}
                          onFocus={onFocus}
                          onBlur={onBlur}
                          className={className}
                        >
                          <span className="flex-1 truncate">{item.name ?? item.path}</span>
                          {status === 'done' && <CheckCircle className="h-4 w-4" />}
                        </div>
                      )}
                    </QuickTooltip>
                  );
                })}
              </div>
            </div>
          )}

          {history.length > 0 && (
            <div>
              <div className="text-[11px] uppercase tracking-wide text-app-muted mb-2">
                Recently processed
              </div>
              <div className="space-y-1 max-h-32 overflow-y-auto">
                {[...history]
                  .slice(-8)
                  .reverse()
                  .map((entry, idx) => (
                    <QuickTooltip key={`${entry}-${idx}`} text={entry}>
                      {({ onBlur, onFocus, onMouseEnter, onMouseLeave, ref }) => (
                        <div
                          ref={ref}
                          onMouseEnter={onMouseEnter}
                          onMouseLeave={onMouseLeave}
                          onFocus={onFocus}
                          onBlur={onBlur}
                          className="truncate text-app-text/80"
                        >
                          {entry}
                        </div>
                      )}
                    </QuickTooltip>
                  ))}
              </div>
            </div>
          )}
        </div>

        {requestId && (
          <div className="text-center text-[11px] text-app-muted">Request ID: {requestId}</div>
        )}
      </div>
    </div>
  );
}
