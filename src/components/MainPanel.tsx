import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type React from 'react';
import { useAppStore } from '../store/useAppStore';
import type { FileItem } from '../types';
import { getSuggestedZipName } from '../utils/zipNaming';
import FileGrid from './FileGrid';
import FileList from './FileList';
import ContextMenu from './ContextMenu';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';
import { ScrollContext } from '../contexts/ScrollContext';
import { getArchiveBaseName, isArchiveFile } from '@/utils/fileTypes';
import { getPlatform } from '@/hooks/usePlatform';
import {
  GRID_SIZE_MIN,
  GRID_SIZE_MAX,
  GRID_SIZE_DEFAULT,
  GRID_SIZE_STEP,
  ZOOM_SCROLL_THRESHOLD,
  ZOOM_BUFFER_RESET_MS,
  NAV_SCROLL_THRESHOLD,
  NAV_COOLDOWN_MS,
} from '@/utils/gridConstants';
import { useFilePanelDropZone } from '../hooks/useDragDetector';

const arraysEqual = (a: string[], b: string[]) => {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
};

export default function MainPanel() {
  // Use selectors to only subscribe to state we actually use
  // This prevents re-renders when unrelated state (like selectedFiles) changes
  const files = useAppStore((state) => state.files);
  const error = useAppStore((state) => state.error);
  const globalPreferences = useAppStore((state) => state.globalPreferences);
  const currentPath = useAppStore((state) => state.currentPath);
  const directoryPreferences = useAppStore((state) => state.directoryPreferences);
  const setSelectedFiles = useAppStore((state) => state.setSelectedFiles);
  const loading = useAppStore((state) => state.loading);

  // Debounce spinner to avoid flash on fast directory loads
  const [debouncedLoading, setDebouncedLoading] = useState(false);
  useEffect(() => {
    if (loading) {
      // Only show spinner after 500ms of loading
      const timer = setTimeout(() => setDebouncedLoading(true), 500);
      return () => clearTimeout(timer);
    } else {
      // Clear immediately when loading finishes
      setDebouncedLoading(false);
    }
  }, [loading]);

  // We rely solely on the native OS context menu now
  const scrollRef = useRef<HTMLDivElement>(null);
  useFilePanelDropZone(currentPath, scrollRef);
  const fileCtxCaptureRef = useRef<boolean>(false);
  const fileCtxPathRef = useRef<string | null>(null);

  // Refs for wheel-based navigation and zoom
  const navDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const zoomBufferRef = useRef(0);
  const zoomRafRef = useRef<number | null>(null);
  const zoomBufferResetRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Wheel handler for horizontal scroll navigation and Ctrl/Cmd+scroll zoom
  const onWheel = useCallback((e: WheelEvent) => {
    const { isMac } = getPlatform();
    // On Mac, use metaKey OR ctrlKey (pinch-to-zoom sets ctrlKey)
    const modKey = isMac ? e.metaKey || e.ctrlKey : e.ctrlKey;

    // Skip if target is in editable element (guard for non-Element targets)
    const target = e.target;
    if (!(target instanceof Element)) return;
    if (target.closest('input, textarea, [contenteditable="true"]')) return;

    // Ctrl/Cmd + vertical scroll = zoom (only in grid view)
    if (modKey && Math.abs(e.deltaY) > Math.abs(e.deltaX)) {
      const state = useAppStore.getState();
      const merged = {
        ...state.globalPreferences,
        ...state.directoryPreferences[state.currentPath],
      };
      if (merged.viewMode !== 'grid') return;

      e.preventDefault();

      // Reset the buffer reset timer on each scroll event
      if (zoomBufferResetRef.current) {
        clearTimeout(zoomBufferResetRef.current);
      }
      zoomBufferResetRef.current = setTimeout(() => {
        zoomBufferRef.current = 0;
        zoomBufferResetRef.current = null;
      }, ZOOM_BUFFER_RESET_MS);

      // Accumulate delta for smooth zooming
      zoomBufferRef.current += e.deltaY;

      if (zoomRafRef.current === null) {
        zoomRafRef.current = requestAnimationFrame(() => {
          const state = useAppStore.getState();
          const merged = {
            ...state.globalPreferences,
            ...state.directoryPreferences[state.currentPath],
          };
          const current = merged.gridSize ?? GRID_SIZE_DEFAULT;

          if (Math.abs(zoomBufferRef.current) > ZOOM_SCROLL_THRESHOLD) {
            const direction = zoomBufferRef.current < 0 ? 1 : -1; // scroll up = zoom in
            const newSize = Math.max(
              GRID_SIZE_MIN,
              Math.min(GRID_SIZE_MAX, current + direction * GRID_SIZE_STEP)
            );
            state.updateDirectoryPreferences(state.currentPath, { gridSize: newSize });
            zoomBufferRef.current = 0;
          }

          zoomRafRef.current = null;
        });
      }
      return;
    }

    // Horizontal scroll (no modifier) = back/forward navigation with debounce
    if (
      !modKey &&
      Math.abs(e.deltaX) > Math.abs(e.deltaY) &&
      Math.abs(e.deltaX) > NAV_SCROLL_THRESHOLD
    ) {
      if (navDebounceRef.current) return; // Still in cooldown

      e.preventDefault();
      const state = useAppStore.getState();

      let didNavigate = false;
      if (e.deltaX < 0 && state.canGoBack()) {
        state.goBack();
        didNavigate = true;
      } else if (e.deltaX > 0 && state.canGoForward()) {
        state.goForward();
        didNavigate = true;
      }

      if (didNavigate) {
        navDebounceRef.current = setTimeout(() => {
          navDebounceRef.current = null;
        }, NAV_COOLDOWN_MS);
      }
    }
  }, []);

  // Attach wheel event listener to scrollRef
  useEffect(() => {
    const container = scrollRef.current;
    if (!container) return;

    container.addEventListener('wheel', onWheel, { passive: false });

    return () => {
      container.removeEventListener('wheel', onWheel);
      if (navDebounceRef.current) {
        clearTimeout(navDebounceRef.current);
        navDebounceRef.current = null;
      }
      if (zoomRafRef.current) {
        cancelAnimationFrame(zoomRafRef.current);
        zoomRafRef.current = null;
      }
      if (zoomBufferResetRef.current) {
        clearTimeout(zoomBufferResetRef.current);
        zoomBufferResetRef.current = null;
      }
    };
  }, [onWheel]);
  const [fallbackCtx, setFallbackCtx] = useState<{
    x: number;
    y: number;
    isFileCtx: boolean;
  } | null>(null);

  type MarqueeRect = {
    visualLeft: number;
    visualTop: number;
    visualWidth: number;
    visualHeight: number;
  };
  interface MarqueeState {
    pointerId: number;
    originX: number;
    originY: number;
    originClientX: number;
    originClientY: number;
    lastClientX: number;
    lastClientY: number;
    meta: boolean;
    shift: boolean;
    baseSelection: string[];
    indexMap: Map<string, number>;
    didMove: boolean;
  }

  const marqueeStateRef = useRef<MarqueeState | null>(null);
  const [marqueeRect, setMarqueeRect] = useState<MarqueeRect | null>(null);
  const skipBackgroundClearRef = useRef(false);
  const autoScrollFrameRef = useRef<number | null>(null);
  const autoScrollStateRef = useRef<{
    vx: number;
    vy: number;
    target: HTMLDivElement;
  } | null>(null);

  // Memoize to prevent FileGrid/FileList re-renders when unrelated store state changes
  const currentPrefs = useMemo(
    () => ({
      ...globalPreferences,
      ...directoryPreferences[currentPath],
    }),
    [globalPreferences, directoryPreferences, currentPath]
  );

  const handleContextMenuCapture = (e: React.MouseEvent) => {
    const target = e.target as Element | null;
    const fileEl =
      target && 'closest' in target
        ? ((target as Element).closest('[data-file-item="true"]') as HTMLElement | null)
        : null;
    fileCtxCaptureRef.current = !!fileEl;
    fileCtxPathRef.current = fileEl ? fileEl.getAttribute('data-file-path') || null : null;
  };

  const handleContextMenu = async (e: React.MouseEvent) => {
    e.preventDefault();
    try {
      const win = getCurrentWindow();
      // Keep clipboard flags fresh for paste enable/disable.
      await useAppStore.getState().syncClipboardState();
      const state = useAppStore.getState();
      const path = state.currentPath;
      const prefs = { ...state.globalPreferences, ...state.directoryPreferences[path] };
      const sortBy = (prefs.sortBy ?? state.globalPreferences.sortBy) || 'name';
      const sortOrder = (prefs.sortOrder ?? state.globalPreferences.sortOrder) || 'asc';
      const filterText = state.filterText;

      // Derive file context directly from event target for reliability
      const tgt = e.target as Element | null;
      const fileEl =
        tgt && 'closest' in tgt
          ? ((tgt as Element).closest('[data-file-item="true"]') as HTMLElement | null)
          : null;
      const ctxPathFromTarget = fileEl ? fileEl.getAttribute('data-file-path') || null : null;

      // Prefer target-derived context; fall back to capture ref
      const isFileCtx = !!ctxPathFromTarget || fileCtxCaptureRef.current;
      const ctxPath = ctxPathFromTarget || fileCtxPathRef.current;
      // If right-clicked a file, ensure it is selected and pass it explicitly
      let filePaths: string[] | undefined;
      let selectedIsSymlink: boolean | undefined;
      let selectionHasDirectory = false;
      let selectionHasArchive = false;
      let selectionIsSingleArchive = false;
      let suggestedZipName: string | undefined;
      let extractSuggestedName: string | undefined;
      if (isFileCtx && ctxPath) {
        if (!state.selectedFiles.includes(ctxPath)) {
          setSelectedFiles([ctxPath]);
          filePaths = [ctxPath];
        } else {
          // Right-clicked within existing selection: use full selection
          filePaths = state.selectedFiles;
        }
        const activeSelection = filePaths ?? state.selectedFiles;
        const map = new Map(state.files.map((f) => [f.path, f]));
        if (activeSelection && activeSelection.length > 0) {
          selectionHasDirectory = activeSelection.some((path) => map.get(path)?.is_directory);
          if (activeSelection.length === 1) {
            const file = map.get(activeSelection[0]);
            selectedIsSymlink = file?.is_symlink ?? false;
          }
          const selectedItems = activeSelection
            .map((path) => map.get(path))
            .filter((file): file is FileItem => Boolean(file));
          const archiveItems = selectedItems.filter(
            (file) =>
              !file.is_directory && isArchiveFile(file) && !file.path.startsWith('archive://')
          );
          selectionHasArchive = archiveItems.length > 0;
          selectionIsSingleArchive = archiveItems.length === 1 && selectedItems.length === 1;
          if (selectionIsSingleArchive) {
            const archiveItem = archiveItems[0];
            if (archiveItem) {
              extractSuggestedName = getArchiveBaseName(archiveItem.name);
            }
          }
          const hiddenFiltered = prefs.showHidden
            ? state.files
            : state.files.filter((file) => !file.is_hidden);
          const visibleItems = filterText
            ? hiddenFiltered.filter((file) =>
                file.name.toLowerCase().includes(filterText.toLowerCase())
              )
            : hiddenFiltered;
          suggestedZipName = getSuggestedZipName({
            selectedItems,
            visibleItems,
            currentPath: path,
          });
        }
      } else {
        filePaths = undefined;
      }
      fileCtxCaptureRef.current = false;
      fileCtxPathRef.current = null;

      await invoke('show_native_context_menu', {
        windowLabel: win.label,
        x: e.clientX,
        y: e.clientY,
        sortBy: sortBy,
        sortOrder: sortOrder,
        path,
        canPasteFiles: state.canPasteFiles,
        canPasteImage: state.canPasteImage,
        canPasteInternal: state.clipboardPaths.length > 0,
        // Always send a boolean (never undefined)
        hasFileContext: !!isFileCtx,
        // Only send file paths when clicking on a file
        filePaths: isFileCtx ? filePaths : undefined,
        compressSuggestedName: suggestedZipName,
        extractSuggestedName,
        selectedIsSymlink,
        selectionHasDirectory,
        selectionHasArchive,
        selectionIsSingleArchive,
      });
      return;
    } catch (error) {
      console.warn('Falling back to React context menu due to error:', error);
      // If native menu fails (e.g., web preview), show React fallback
      const tgt = e.target as HTMLElement;
      const isFile = !!(tgt && tgt.closest && tgt.closest('[data-file-item="true"]'));
      setFallbackCtx({ x: e.clientX, y: e.clientY, isFileCtx: isFile });
    }
  };

  // No custom context menu fallback

  // Reset scroll when navigating to a new path
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTo({ top: 0, left: 0, behavior: 'auto' });
  }, [currentPath]);

  // Clear selection when clicking anywhere that's not an interactive control
  const handleContainerBackgroundClick = (e: React.MouseEvent<HTMLDivElement>) => {
    if (skipBackgroundClearRef.current) {
      skipBackgroundClearRef.current = false;
      return;
    }
    const target = e.target as HTMLElement;
    // Ignore clicks on obvious controls
    if (
      target.closest('button, a, input, select, textarea, [role="button"], [data-prevent-deselect]')
    )
      return;
    setSelectedFiles([]);
  };

  const handleContainerBackgroundDoubleClick = (e: React.MouseEvent<HTMLDivElement>) => {
    const target = e.target as HTMLElement;
    if (target.closest('[data-file-item="true"]')) return;
    if (
      target.closest('button, a, input, select, textarea, [role="button"], [data-prevent-deselect]')
    )
      return;

    const active = document.activeElement as HTMLElement | null;
    const inEditable =
      !!active &&
      (active.tagName === 'INPUT' || active.tagName === 'TEXTAREA' || active.isContentEditable);
    if (inEditable) return;

    const hasModal = !!document.querySelector(
      '[role="dialog"], [aria-modal="true"], [data-modal="true"]'
    );
    if (hasModal) return;

    const state = useAppStore.getState();
    if (state.renameTargetPath) return;

    e.preventDefault();
    void state.createNewFile();
  };

  const stopAutoScroll = () => {
    if (autoScrollFrameRef.current !== null) {
      cancelAnimationFrame(autoScrollFrameRef.current);
      autoScrollFrameRef.current = null;
    }
    autoScrollStateRef.current = null;
  };

  const applySelectionFromMarquee = useCallback((hitPaths: string[]) => {
    const state = marqueeStateRef.current;
    if (!state) return;

    const store = useAppStore.getState();
    const { meta, shift, baseSelection, indexMap } = state;
    const sortedHits = [...new Set(hitPaths)].sort((a, b) => {
      const ai = indexMap.get(a) ?? Number.MAX_SAFE_INTEGER;
      const bi = indexMap.get(b) ?? Number.MAX_SAFE_INTEGER;
      return ai - bi;
    });

    if (shift) {
      // Shift marquee acts as additive selection; it keeps the existing block and unions touched items.
      if (sortedHits.length === 0) return;
      const existingSet = new Set(baseSelection);
      const merged = baseSelection.slice();
      for (const path of sortedHits) {
        if (!existingSet.has(path)) {
          merged.push(path);
          existingSet.add(path);
        }
      }
      if (!arraysEqual(merged, store.selectedFiles)) {
        store.setSelectedFiles(merged);
      }
      store.setSelectionLead(sortedHits[sortedHits.length - 1]);
      return;
    }

    if (meta) {
      if (sortedHits.length === 0) return;
      const existingSet = new Set(baseSelection);
      const merged = baseSelection.slice();
      for (const path of sortedHits) {
        if (!existingSet.has(path)) {
          merged.push(path);
          existingSet.add(path);
        }
      }
      if (!arraysEqual(merged, store.selectedFiles)) {
        store.setSelectedFiles(merged);
      }
      return;
    }

    if (!arraysEqual(sortedHits, store.selectedFiles)) {
      store.setSelectedFiles(sortedHits);
    }
    if (sortedHits.length > 0) {
      store.setSelectionAnchor(sortedHits[0]);
      store.setSelectionLead(sortedHits[sortedHits.length - 1]);
    } else {
      store.setSelectionAnchor(undefined);
      store.setSelectionLead(undefined);
    }
  }, []);

  const updateMarqueeFromClient = useCallback(
    (clientX: number, clientY: number, fromAutoScroll = false) => {
      const state = marqueeStateRef.current;
      const scrollEl = scrollRef.current;
      if (!state || !scrollEl) return;

      if (!fromAutoScroll) {
        state.lastClientX = clientX;
        state.lastClientY = clientY;
      }

      const rect = scrollEl.getBoundingClientRect();
      const scrollLeft = scrollEl.scrollLeft;
      const scrollTop = scrollEl.scrollTop;
      const currentX = clientX - rect.left + scrollLeft;
      const currentY = clientY - rect.top + scrollTop;

      const dx = currentX - state.originX;
      const dy = currentY - state.originY;
      if (!state.didMove) {
        const distance = Math.hypot(dx, dy);
        if (distance > 2) {
          state.didMove = true;
          skipBackgroundClearRef.current = true;
        }
      }

      const left = Math.min(state.originX, currentX);
      const top = Math.min(state.originY, currentY);
      const width = Math.abs(dx);
      const height = Math.abs(dy);

      setMarqueeRect({
        visualLeft: left,
        visualTop: top,
        visualWidth: width,
        visualHeight: height,
      });

      if (!state.didMove && width < 1 && height < 1) {
        return;
      }

      const hits: string[] = [];
      const right = left + width;
      const bottom = top + height;
      const nodes = Array.from(
        scrollEl.querySelectorAll<HTMLElement>('[data-file-item="true"][data-file-path]')
      );

      for (const node of nodes) {
        const path = node.getAttribute('data-file-path');
        if (!path) continue;
        const nodeRect = node.getBoundingClientRect();
        const nodeLeft = nodeRect.left - rect.left + scrollLeft;
        const nodeTop = nodeRect.top - rect.top + scrollTop;
        const nodeRight = nodeLeft + nodeRect.width;
        const nodeBottom = nodeTop + nodeRect.height;
        if (nodeRight < left || nodeLeft > right || nodeBottom < top || nodeTop > bottom) {
          continue;
        }
        hits.push(path);
      }

      applySelectionFromMarquee(hits);
    },
    [applySelectionFromMarquee]
  );

  useEffect(() => {
    const scrollEl = scrollRef.current;
    if (!scrollEl || !marqueeRect) return;

    const handleScroll = () => {
      const state = marqueeStateRef.current;
      if (!state) return;
      updateMarqueeFromClient(state.lastClientX, state.lastClientY, true);
    };

    scrollEl.addEventListener('scroll', handleScroll, { passive: true });
    return () => {
      scrollEl.removeEventListener('scroll', handleScroll);
    };
  }, [marqueeRect, updateMarqueeFromClient]);

  const autoScrollLoop = () => {
    const state = autoScrollStateRef.current;
    const marquee = marqueeStateRef.current;
    if (!state || !marquee) {
      stopAutoScroll();
      return;
    }
    const { target, vx, vy } = state;
    if (vx !== 0 || vy !== 0) {
      target.scrollBy({ left: vx, top: vy, behavior: 'auto' });
      updateMarqueeFromClient(marquee.lastClientX, marquee.lastClientY, true);
      autoScrollFrameRef.current = requestAnimationFrame(autoScrollLoop);
    } else {
      stopAutoScroll();
    }
  };

  const updateAutoScroll = (clientX: number, clientY: number) => {
    const scrollEl = scrollRef.current;
    if (!scrollEl) return;
    const rect = scrollEl.getBoundingClientRect();
    const threshold = 36;

    let vy = 0;
    if (clientY < rect.top + threshold) {
      const intensity = (threshold - (clientY - rect.top)) / threshold;
      vy = -Math.min(28, 8 + intensity * 24);
    } else if (clientY > rect.bottom - threshold) {
      const intensity = (clientY - (rect.bottom - threshold)) / threshold;
      vy = Math.min(28, 8 + intensity * 24);
    }

    let vx = 0;
    if (clientX < rect.left + threshold) {
      const intensity = (threshold - (clientX - rect.left)) / threshold;
      vx = -Math.min(24, 6 + intensity * 18);
    } else if (clientX > rect.right - threshold) {
      const intensity = (clientX - (rect.right - threshold)) / threshold;
      vx = Math.min(24, 6 + intensity * 18);
    }

    if (vx === 0 && vy === 0) {
      stopAutoScroll();
      return;
    }

    autoScrollStateRef.current = { vx, vy, target: scrollEl };
    if (autoScrollFrameRef.current === null) {
      autoScrollFrameRef.current = requestAnimationFrame(autoScrollLoop);
    }
  };

  const endMarquee = () => {
    stopAutoScroll();
    marqueeStateRef.current = null;
    setMarqueeRect(null);
  };

  const handlePointerMove = (e: PointerEvent) => {
    const state = marqueeStateRef.current;
    if (!state || e.pointerId !== state.pointerId) return;
    updateMarqueeFromClient(e.clientX, e.clientY);
    updateAutoScroll(e.clientX, e.clientY);
    e.preventDefault();
  };

  const handlePointerUp = (e: PointerEvent) => {
    const state = marqueeStateRef.current;
    if (!state || e.pointerId !== state.pointerId) return;

    if (!state.didMove) {
      const store = useAppStore.getState();
      if (!state.meta && !state.shift && store.selectedFiles.length > 0) {
        store.setSelectedFiles([]);
        store.setSelectionAnchor(undefined);
        store.setSelectionLead(undefined);
      }
    } else {
      window.setTimeout(() => {
        skipBackgroundClearRef.current = false;
      }, 0);
    }

    window.removeEventListener('pointermove', handlePointerMove);
    window.removeEventListener('pointerup', handlePointerUp);
    window.removeEventListener('pointercancel', handlePointerUp);
    endMarquee();
  };

  const startMarquee = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    const targetEl = e.target as HTMLElement | null;
    if (targetEl?.closest('[data-file-item="true"]')) return;
    if (
      targetEl?.closest(
        'button, a, input, select, textarea, [role="button"], [data-prevent-deselect]'
      )
    )
      return;

    const scrollEl = scrollRef.current;
    if (!scrollEl) return;

    stopAutoScroll();
    skipBackgroundClearRef.current = false;

    const rect = scrollEl.getBoundingClientRect();
    const scrollLeft = scrollEl.scrollLeft;
    const scrollTop = scrollEl.scrollTop;
    const originX = e.clientX - rect.left + scrollLeft;
    const originY = e.clientY - rect.top + scrollTop;

    const nodes = Array.from(
      scrollEl.querySelectorAll<HTMLElement>('[data-file-item="true"][data-file-path]')
    );
    const indexMap = new Map<string, number>();
    let nextIndex = 0;
    for (const node of nodes) {
      const path = node.getAttribute('data-file-path');
      if (!path || indexMap.has(path)) continue;
      indexMap.set(path, nextIndex++);
    }

    const state: MarqueeState = {
      pointerId: e.pointerId,
      originX,
      originY,
      originClientX: e.clientX,
      originClientY: e.clientY,
      lastClientX: e.clientX,
      lastClientY: e.clientY,
      meta: e.metaKey || e.ctrlKey,
      shift: e.shiftKey,
      baseSelection: useAppStore.getState().selectedFiles.slice(),
      indexMap,
      didMove: false,
    };

    marqueeStateRef.current = state;
    setMarqueeRect({
      visualLeft: originX,
      visualTop: originY,
      visualWidth: 0,
      visualHeight: 0,
    });

    window.addEventListener('pointermove', handlePointerMove, { passive: false });
    window.addEventListener('pointerup', handlePointerUp, { passive: false });
    window.addEventListener('pointercancel', handlePointerUp, { passive: false });
  };

  if (error) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <div className="text-app-red">Error: {error}</div>
      </div>
    );
  }

  return (
    <div className="flex-1 flex flex-col select-none min-h-0">
      {/* File content only */}
      <div
        ref={scrollRef}
        data-main-content
        className="relative flex-1 min-h-0 overflow-auto pb-20"
        onContextMenuCapture={handleContextMenuCapture}
        onContextMenu={handleContextMenu}
        onClick={handleContainerBackgroundClick}
        onDoubleClick={handleContainerBackgroundDoubleClick}
        onPointerDown={startMarquee}
        data-drop-zone-id="file-panel"
      >
        {/* Mask old content while loading to avoid layout flicker during view changes */}
        <ScrollContext.Provider value={scrollRef}>
          <div className={`${loading ? 'invisible' : 'visible'}`}>
            {currentPrefs.viewMode === 'grid' ? (
              <FileGrid files={files} preferences={currentPrefs} />
            ) : (
              <FileList files={files} preferences={currentPrefs} />
            )}
          </div>
        </ScrollContext.Provider>

        {/* In-app drag ghost removed; rely on setDragImage */}

        {marqueeRect && (
          <div
            className="pointer-events-none absolute z-50 rounded-sm"
            style={{
              left: marqueeRect.visualLeft,
              top: marqueeRect.visualTop,
              width: marqueeRect.visualWidth,
              height: marqueeRect.visualHeight,
              border: '1px solid var(--accent)',
              backgroundColor: 'rgba(var(--accent-rgb), 0.18)',
            }}
          />
        )}

        {debouncedLoading && (
          <div className="absolute inset-0 flex items-center justify-center bg-app-dark">
            <div className="animate-spin w-6 h-6 border-2 border-[var(--accent)] border-t-transparent rounded-full" />
          </div>
        )}
      </div>

      {/* React context menu fallback (web preview or native failure) */}
      {fallbackCtx && (
        <ContextMenu
          x={fallbackCtx.x}
          y={fallbackCtx.y}
          isFileContext={fallbackCtx.isFileCtx}
          onRequestClose={() => setFallbackCtx(null)}
        />
      )}
    </div>
  );
}
