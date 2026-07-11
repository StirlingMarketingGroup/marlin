import { useEffect, useCallback, useMemo, useRef, RefObject } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import { useDragStore } from '@/store/useDragStore';
import { useToastStore } from '@/store/useToastStore';
import { pushUndoAndShowToast } from '@/store/useUndoStore';

export interface DragModifiers {
  optionAlt: boolean;
  cmdCtrl: boolean;
}

const SAME_LOCATION_DROP_REASON = 'NO_OP_SAME_LOCATION';

const normalizeLocalPathForDrop = (path: string) =>
  path.replace(/\\/g, '/').replace(/\/+$/, '') || '/';

const localDirnameForDrop = (path: string) => {
  const normalized = normalizeLocalPathForDrop(path);
  const lastSlash = normalized.lastIndexOf('/');
  if (lastSlash <= 0) return '/';
  return normalized.slice(0, lastSlash);
};

const isSameLocationDrop = (paths: string[], targetPath: string) => {
  const normalizedTarget = normalizeLocalPathForDrop(targetPath);
  return (
    paths.length > 0 &&
    paths.every((path) => {
      const normalizedPath = normalizeLocalPathForDrop(path);
      return normalizedPath === normalizedTarget || localDirnameForDrop(path) === normalizedTarget;
    })
  );
};

export interface DragDropEvent {
  paths: string[];
  location: {
    x: number;
    y: number;
    targetId: string | null;
  };
  eventType: 'dragEnter' | 'dragOver' | 'dragLeave' | 'drop';
  modifiers: DragModifiers;
}

export interface DragDropHandlers {
  onDragEnter?: (event: DragDropEvent) => void;
  onDragOver?: (event: DragDropEvent) => void;
  onDragLeave?: (event: DragDropEvent) => void;
  onDrop?: (event: DragDropEvent) => void;
}

/**
 * Hook to handle native drag and drop events within the application
 * This uses a custom Tauri plugin to detect drops that the native drag API can't handle
 */
export function useDragDetector(handlers: DragDropHandlers) {
  const handlersRef = useRef(handlers);

  useEffect(() => {
    handlersRef.current = handlers;
  }, [handlers]);

  const enableDragDetection = useCallback(async () => {
    try {
      await invoke('enable_drag_detection');
    } catch (error) {
      console.error('Failed to enable drag detection:', error);
    }
  }, []);

  const setDropZone = useCallback(
    async (zoneId: string, enabled: boolean, config?: { width?: number }) => {
      try {
        await invoke('set_drop_zone', {
          zoneId,
          enabled,
          config: config ?? null,
        });
      } catch (error) {
        console.error('Failed to set drop zone:', error);
      }
    },
    []
  );

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let lastTargetId: string | null = null;
    let lastLogTime = 0;
    let disposed = false;

    const setupListener = async () => {
      // Enable drag detection when component mounts
      await enableDragDetection();
      if (disposed) return;

      // Listen for drag-drop events from the plugin
      const nextUnlisten = await listen<DragDropEvent>('drag-drop-event', (event) => {
        // Only handle hover/highlight events when this window is focused
        // This prevents unfocused windows from showing dropzone highlights
        // But allow drop events through - during cross-app drags, target window
        // may not be focused until the actual drop occurs
        const isDropEvent = event.payload.eventType === 'drop';
        if (!isDropEvent && !document.hasFocus()) {
          return;
        }

        const viewportHeight = window.innerHeight;

        let zoneId: string | null = event.payload.location.targetId ?? null;
        const domPoint = {
          x: event.payload.location.x,
          y: viewportHeight - event.payload.location.y,
        };

        if (!zoneId) {
          if (Number.isFinite(domPoint.x) && Number.isFinite(domPoint.y)) {
            if (
              domPoint.x >= 0 &&
              domPoint.x <= window.innerWidth &&
              domPoint.y >= 0 &&
              domPoint.y <= viewportHeight
            ) {
              const element = document.elementFromPoint(domPoint.x, domPoint.y);
              const zoneEl = element?.closest<HTMLElement>('[data-drop-zone-id]');
              if (zoneEl) {
                zoneId = zoneEl.dataset.dropZoneId ?? null;
              }
            }
          }
        }

        const normalizedEvent: DragDropEvent = {
          ...event.payload,
          location: {
            ...event.payload.location,
            x: domPoint.x,
            y: domPoint.y,
            targetId: zoneId,
          },
        };

        const now = performance.now();
        if (
          import.meta.env.DEV &&
          normalizedEvent.eventType !== 'dragOver' &&
          (normalizedEvent.location.targetId !== lastTargetId || now - lastLogTime > 250)
        ) {
          lastTargetId = normalizedEvent.location.targetId ?? null;
          lastLogTime = now;
          console.debug('[drag-detector]', normalizedEvent);
        }

        switch (normalizedEvent.eventType) {
          case 'dragEnter':
            handlersRef.current.onDragEnter?.(normalizedEvent);
            break;
          case 'dragOver':
            if (normalizedEvent.location.targetId) {
              handlersRef.current.onDragOver?.(normalizedEvent);
            } else {
              handlersRef.current.onDragLeave?.({
                ...normalizedEvent,
                eventType: 'dragLeave',
                location: {
                  ...normalizedEvent.location,
                  targetId: null,
                },
              });
            }
            break;
          case 'dragLeave':
            handlersRef.current.onDragLeave?.(normalizedEvent);
            break;
          case 'drop':
            handlersRef.current.onDrop?.(normalizedEvent);
            break;
        }
      });

      if (disposed) {
        nextUnlisten();
      } else {
        unlisten = nextUnlisten;
      }
    };

    setupListener();

    return () => {
      disposed = true;
      if (unlisten) {
        unlisten();
      }
      lastTargetId = null;
      lastLogTime = 0;
    };
  }, [enableDragDetection]);

  return {
    setDropZone,
    enableDragDetection,
  };
}

/**
 * Hook for file panel drop zone (main content area)
 * Handles drops onto folders or empty space in the file panel
 */
export function useFilePanelDropZone(
  currentPath: string,
  scrollRef: RefObject<HTMLElement | null>
) {
  const setDropTargetPath = useDragStore((s) => s.setDropTargetPath);
  const setPendingDropOperation = useDragStore((s) => s.setPendingDropOperation);
  const setIsDraggingOver = useDragStore((s) => s.setIsDraggingOver);

  // Cache for resolved operations: key = paths + dest + modifiers
  const operationCache = useRef<
    Map<string, { operation: 'move' | 'copy'; isRemote: boolean; valid: boolean; reason?: string }>
  >(new Map());
  const currentDragPaths = useRef<string[]>([]);

  // Throttle control
  const lastUpdateRef = useRef<number>(0);
  const lastDropRef = useRef<{ key: string; timestamp: number } | null>(null);

  const DROP_DEDUPE_WINDOW_MS = 400;

  const resolveOperation = useCallback(
    async (
      paths: string[],
      dest: string,
      modifiers: DragModifiers
    ): Promise<{
      operation: 'move' | 'copy';
      isRemote: boolean;
      valid: boolean;
      reason?: string;
    }> => {
      const key = `${paths.join('|')}::${dest}::${modifiers.optionAlt}::${modifiers.cmdCtrl}`;
      if (operationCache.current.has(key)) {
        return operationCache.current.get(key)!;
      }

      try {
        const result = await invoke<{
          operation: string;
          isRemote: boolean;
          valid: boolean;
          reason?: string;
        }>('resolve_drop_operation', {
          sources: paths,
          destination: dest,
          modifierOption: modifiers.optionAlt,
          modifierCmd: modifiers.cmdCtrl,
        });
        const typedResult = {
          operation: (result.operation === 'none' ? 'copy' : result.operation) as 'move' | 'copy',
          isRemote: result.isRemote,
          valid: result.valid,
          reason: result.reason,
        };
        operationCache.current.set(key, typedResult);
        return typedResult;
      } catch (e) {
        console.error('Failed to resolve drop operation', e);
        return {
          operation: 'copy',
          isRemote: false,
          valid: false,
          reason: 'Failed to validate drop',
        };
      }
    },
    []
  );

  const handlers = useMemo<DragDropHandlers>(
    () => ({
      onDragEnter: (event) => {
        if (event.location.targetId !== 'file-panel') return;
        setIsDraggingOver(true);
        currentDragPaths.current = event.paths;
        // Pre-warm cache for current directory
        void resolveOperation(event.paths, currentPath, event.modifiers);
      },

      onDragOver: (event) => {
        if (event.location.targetId !== 'file-panel') {
          setDropTargetPath(null);
          return;
        }

        // Auto-scroll logic
        if (scrollRef.current) {
          const rect = scrollRef.current.getBoundingClientRect();
          const threshold = 50;
          const y = event.location.y;

          let vy = 0;
          if (y < rect.top + threshold) {
            const intensity = (threshold - (y - rect.top)) / threshold;
            vy = -Math.min(20, 5 + intensity * 15);
          } else if (y > rect.bottom - threshold) {
            const intensity = (y - (rect.bottom - threshold)) / threshold;
            vy = Math.min(20, 5 + intensity * 15);
          }

          if (vy !== 0) {
            scrollRef.current.scrollBy(0, vy);
          }
        }

        const now = performance.now();
        // Throttle to ~60fps (16ms)
        if (now - lastUpdateRef.current < 16) return;
        lastUpdateRef.current = now;

        // Determine drop target (folder or current dir)
        let targetPath = currentPath;

        // Hit test for folder targets using data-folder-path attribute
        const elements = document.elementsFromPoint(event.location.x, event.location.y);
        const folderEl = elements.find((el) => el.hasAttribute('data-folder-path')) as
          HTMLElement | undefined;

        if (folderEl) {
          const folderPath = folderEl.getAttribute('data-folder-path');
          if (folderPath) {
            targetPath = folderPath;
          }
        }

        const updateState = async () => {
          const opInfo = await resolveOperation(event.paths, targetPath, event.modifiers);

          // Only highlight if valid
          if (opInfo.valid) {
            setDropTargetPath(targetPath === currentPath ? null : targetPath);
            setPendingDropOperation(opInfo.operation);
          } else {
            setDropTargetPath(null);
            setPendingDropOperation(null);
          }
        };

        void updateState();
      },

      onDragLeave: () => {
        setIsDraggingOver(false);
        setDropTargetPath(null);
        setPendingDropOperation(null);
      },

      onDrop: (event) => {
        if (event.location.targetId !== 'file-panel') return;

        // Determine final target first (needed for deduplication key)
        let targetPath = currentPath;
        const elements = document.elementsFromPoint(event.location.x, event.location.y);
        const folderEl = elements.find((el) => el.hasAttribute('data-folder-path')) as
          HTMLElement | undefined;

        if (folderEl) {
          const folderPath = folderEl.getAttribute('data-folder-path');
          if (folderPath) {
            targetPath = folderPath;
          }
        }

        const activeNativeDragPaths = useDragStore.getState().nativeDragPaths;
        if (
          isSameLocationDrop(event.paths, targetPath) ||
          isSameLocationDrop(activeNativeDragPaths, targetPath)
        ) {
          setIsDraggingOver(false);
          setDropTargetPath(null);
          setPendingDropOperation(null);
          operationCache.current.clear();
          return;
        }

        // Dedupe rapid drops (include target to allow different destinations)
        const sortedKey = `${event.paths.slice().sort().join('|')}::${targetPath}`;
        const now = Date.now();
        const lastDrop = lastDropRef.current;
        if (
          lastDrop &&
          lastDrop.key === sortedKey &&
          now - lastDrop.timestamp < DROP_DEDUPE_WINDOW_MS
        ) {
          setIsDraggingOver(false);
          setDropTargetPath(null);
          setPendingDropOperation(null);
          return;
        }
        lastDropRef.current = { key: sortedKey, timestamp: now };

        setIsDraggingOver(false);
        setDropTargetPath(null);
        setPendingDropOperation(null);
        operationCache.current.clear();

        // Final validation and execution
        void resolveOperation(event.paths, targetPath, event.modifiers).then(async (opInfo) => {
          if (opInfo.reason === SAME_LOCATION_DROP_REASON) {
            return;
          }

          if (!opInfo.valid) {
            useToastStore.getState().addToast({
              type: 'error',
              message: opInfo.reason ?? 'Invalid drop operation',
            });
            return;
          }

          const isCut = opInfo.operation === 'move';
          const toastStore = useToastStore.getState();
          const sourcePaths = event.paths.slice();

          try {
            const result = await invoke<{
              pastedPaths: string[];
              skippedCount: number;
              errorMessage?: string;
            }>('paste_items_to_location', {
              destination: targetPath,
              sourcePaths,
              isCut,
            });

            if (result.errorMessage) {
              toastStore.addToast({
                type: 'error',
                message: result.errorMessage,
              });
              return;
            }

            if (result.pastedPaths.length > 0) {
              // Push to undo stack and show toast with Undo button
              pushUndoAndShowToast(result, isCut, sourcePaths, toastStore);
            }

            if (result.skippedCount > 0) {
              toastStore.addToast({
                type: 'info',
                message: `${result.skippedCount} item${result.skippedCount !== 1 ? 's' : ''} skipped`,
              });
            }

            // File watcher will detect changes and animate them
          } catch (error) {
            const message = error instanceof Error ? error.message : String(error);
            toastStore.addToast({
              type: 'error',
              message: `Drop failed: ${message}`,
            });
          }
        });
      },
    }),
    [
      currentPath,
      resolveOperation,
      setDropTargetPath,
      setPendingDropOperation,
      setIsDraggingOver,
      scrollRef,
    ]
  );

  const { setDropZone } = useDragDetector(handlers);

  useEffect(() => {
    void setDropZone('file-panel', true);
    return () => {
      void setDropZone('file-panel', false);
    };
  }, [setDropZone]);

  // Clear state when path changes
  useEffect(() => {
    setDropTargetPath(null);
    setPendingDropOperation(null);
    operationCache.current.clear();
  }, [currentPath, setDropTargetPath, setPendingDropOperation]);
}

/**
 * Hook specifically for sidebar drop zone
 */
interface SidebarDropZoneHandlers {
  onDragEnter?: () => void;
  onDragOver?: () => void;
  onDragLeave?: () => void;
}

interface SidebarDropZoneOptions extends SidebarDropZoneHandlers {
  width?: number;
}

export function useSidebarDropZone(
  onDrop: (paths: string[]) => void,
  options?: SidebarDropZoneOptions
) {
  const { onDragEnter, onDragOver, onDragLeave, width } = options ?? {};

  const lastDropRef = useRef<{ key: string; timestamp: number } | null>(null);
  const DROP_DEDUPE_WINDOW_MS = 400;

  const handleDrop = useCallback(
    (event: DragDropEvent) => {
      if (event.location.targetId !== 'sidebar' || event.paths.length === 0) {
        return;
      }
      // Include zone in key for consistency
      const sortedKey = `${event.paths.slice().sort().join('|')}::sidebar`;
      const now = Date.now();
      const lastDrop = lastDropRef.current;
      if (
        lastDrop &&
        lastDrop.key === sortedKey &&
        now - lastDrop.timestamp < DROP_DEDUPE_WINDOW_MS
      ) {
        onDragLeave?.();
        return;
      }

      lastDropRef.current = { key: sortedKey, timestamp: now };
      onDrop(event.paths);
    },
    [onDrop, onDragLeave]
  );

  const detectorHandlers = useMemo<DragDropHandlers>(
    () => ({
      onDrop: handleDrop,
      onDragEnter: (event) => {
        if (event.location.targetId === 'sidebar') {
          onDragEnter?.();
        }
      },
      onDragOver: (event) => {
        if (event.location.targetId === 'sidebar') {
          onDragOver?.();
        } else {
          onDragLeave?.();
        }
      },
      onDragLeave: () => {
        onDragLeave?.();
      },
    }),
    [handleDrop, onDragEnter, onDragOver, onDragLeave]
  );

  const { setDropZone } = useDragDetector(detectorHandlers);

  useEffect(() => {
    void setDropZone('sidebar', true);
    return () => {
      void setDropZone('sidebar', false);
    };
  }, [setDropZone]);

  useEffect(() => {
    if (width == null) return;
    void setDropZone('sidebar', true, { width });
  }, [setDropZone, width]);
}
