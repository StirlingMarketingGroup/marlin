import { useEffect, useRef, useState } from 'react';
import { useAppStore } from '../store/useAppStore';
import FileGrid from './FileGrid';
import FileList from './FileList';
import ContextMenu from './ContextMenu';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';

export default function MainPanel() {
  const {
    files,
    error,
    globalPreferences,
    currentPath,
    directoryPreferences,
    setSelectedFiles,
    loading,
  } = useAppStore();

  // We rely solely on the native OS context menu now
  const scrollRef = useRef<HTMLDivElement>(null);
  const fileCtxCaptureRef = useRef<boolean>(false);
  const fileCtxPathRef = useRef<string | null>(null);
  const [fallbackCtx, setFallbackCtx] = useState<{
    x: number;
    y: number;
    isFileCtx: boolean;
  } | null>(null);

  const currentPrefs = {
    ...globalPreferences,
    ...directoryPreferences[currentPath],
  };

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
      const state = useAppStore.getState();
      const path = state.currentPath;
      const prefs = { ...state.globalPreferences, ...state.directoryPreferences[path] };
      const sortBy = (prefs.sortBy ?? state.globalPreferences.sortBy) || 'name';
      const sortOrder = (prefs.sortOrder ?? state.globalPreferences.sortOrder) || 'asc';

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
      if (isFileCtx && ctxPath) {
        if (!state.selectedFiles.includes(ctxPath)) {
          setSelectedFiles([ctxPath]);
          filePaths = [ctxPath];
        } else {
          // Right-clicked within existing selection: use full selection
          filePaths = state.selectedFiles;
        }
        const activeSelection = filePaths ?? state.selectedFiles;
        if (activeSelection && activeSelection.length === 1) {
          const map = new Map(state.files.map((f) => [f.path, f]));
          const file = map.get(activeSelection[0]);
          selectedIsSymlink = file?.is_symlink ?? false;
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
        // Always send a boolean (never undefined)
        hasFileContext: !!isFileCtx,
        // Only send file paths when clicking on a file
        filePaths: isFileCtx ? filePaths : undefined,
        selectedIsSymlink,
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
    const target = e.target as HTMLElement;
    // Ignore clicks on obvious controls
    if (
      target.closest('button, a, input, select, textarea, [role="button"], [data-prevent-deselect]')
    )
      return;
    setSelectedFiles([]);
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
        className="relative flex-1 min-h-0 overflow-auto"
        onContextMenuCapture={handleContextMenuCapture}
        onContextMenu={handleContextMenu}
        onClick={handleContainerBackgroundClick}
      >
        {/* Mask old content while loading to avoid layout flicker during view changes */}
        <div className={`${loading ? 'invisible' : 'visible'}`}>
          {currentPrefs.viewMode === 'grid' ? (
            <FileGrid files={files} preferences={currentPrefs} />
          ) : (
            <FileList files={files} preferences={currentPrefs} />
          )}
        </div>

        {/* In-app drag ghost removed; rely on setDragImage */}

        {loading && (
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
