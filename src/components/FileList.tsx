import { useEffect, useMemo, useRef, useState, useCallback, type ReactNode } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { CaretUp, CaretDown, Play, AppWindow, Folder, Package, Disc } from 'phosphor-react';
import { FileItem, ViewPreferences } from '../types';
import { useAppStore } from '../store/useAppStore';
import { useDragStore } from '../store/useDragStore';
import AppIcon from '@/components/AppIcon';
import { createDragImageForSelection, createDragImageForSelectionAsync } from '@/utils/dragImage';
import { invoke } from '@tauri-apps/api/core';
// no direct invoke here; background opens the menu
import { useThumbnail } from '@/hooks/useThumbnail';
import { useFileIcon } from '@/hooks/useFileIcon';
import { usePlatform } from '@/hooks/usePlatform';
import { useVisibility } from '@/hooks/useVisibility';
import FileNameDisplay from './FileNameDisplay';
import SymlinkBadge from '@/components/SymlinkBadge';
import GitRepoBadge from '@/components/GitRepoBadge';
import { normalizePreviewIcon } from '@/utils/iconSizing';
import { isArchiveFile, isVideoExtension, isMacOSBundle } from '@/utils/fileTypes';

interface FileListProps {
  files: FileItem[];
  preferences: ViewPreferences;
}

// Stable, top-level preview component to avoid remount flicker
function ListFilePreview({
  file,
  isMac,
  fallbackIcon,
}: {
  file: FileItem;
  isMac: boolean;
  fallbackIcon: ReactNode;
}) {
  const { ref: previewRef, stage } = useVisibility<HTMLDivElement>({
    nearMargin: '800px',
    visibleMargin: '0px',
  });
  const ext = file.extension?.toLowerCase();
  const isImage =
    !!ext &&
    ['jpg', 'jpeg', 'png', 'gif', 'webp', 'bmp', 'tiff', 'tga', 'ico', 'svg'].includes(ext || '');
  const isPdf = ext === 'pdf';
  const isAi = ext === 'ai' || ext === 'eps';
  const isPsd = ext === 'psd' || ext === 'psb';
  const isSvg = ext === 'svg';
  const isStl = ext === 'stl';
  const isVideo = isVideoExtension(ext);

  const isThumbnailCandidate = isImage || isPdf || isAi || isPsd || isStl || isVideo;
  const dpr =
    typeof window !== 'undefined' ? Math.min(2, Math.max(1, window.devicePixelRatio || 1)) : 1;
  const priority = stage === 'visible' ? 'high' : 'medium';
  const { dataUrl, loading, hasTransparency } = useThumbnail(
    isThumbnailCandidate && stage !== 'far' ? file.path : undefined,
    { size: Math.round(64 * dpr), quality: 'medium', priority }
  );

  if (isMac) {
    const fileName = file.name.toLowerCase();
    if (file.is_directory && fileName.endsWith('.app')) {
      return (
        <AppIcon
          path={file.path}
          size={64}
          className="w-5 h-5"
          rounded={false}
          priority="high"
          fallback={<AppWindow className="w-5 h-5 text-accent" />}
        />
      );
    }
    if (fileName.endsWith('.pkg')) {
      return <Package className="w-5 h-5 text-blue-500" weight="fill" />;
    }
    if (fileName.endsWith('.dmg')) {
      return <Disc className="w-5 h-5 text-app-muted" weight="fill" />;
    }
  }

  if (isThumbnailCandidate) {
    if (dataUrl) {
      const isRaster = (isImage && !isSvg) || isVideo;
      return (
        <div
          ref={previewRef}
          className={`relative w-5 h-5 rounded-sm border border-app-border ${hasTransparency ? 'bg-checker' : ''} ${isRaster ? '' : 'p-[1px]'} overflow-hidden`}
        >
          <img
            src={dataUrl}
            alt=""
            className={`w-full h-full`}
            style={{
              objectFit: isRaster ? ('contain' as const) : ('contain' as const),
              transform: 'none',
            }}
            onLoad={(e) => {
              if (!isRaster) return;
              const img = e.currentTarget as HTMLImageElement;
              const iw = img.naturalWidth || 1;
              const ih = img.naturalHeight || 1;
              const r = iw / ih;
              if (r > 1 / 1.1 && r < 1.1) {
                img.style.objectFit = 'cover';
                img.style.transform = 'scale(1.01)';
              } else {
                img.style.objectFit = 'contain';
                img.style.transform = 'none';
              }
            }}
            draggable={false}
          />
          {isVideo && (
            <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
              <div className="flex items-center justify-center rounded-full bg-black/45 p-[2px]">
                <Play weight="fill" className="h-[10px] w-[10px] text-white/90" />
              </div>
            </div>
          )}
        </div>
      );
    }
    if (loading) {
      return (
        <div
          ref={previewRef}
          className="w-5 h-5 rounded-sm border border-app-border animate-pulse"
        />
      );
    }
    return <div ref={previewRef} className="w-5 h-5 rounded-sm border border-app-border" />;
  }

  const containerPx = 20; // matches w-5/h-5
  const padding = 2;
  const iconSize = Math.round(Math.min(containerPx - padding * 2, 18));
  const normalizedIcon = normalizePreviewIcon(fallbackIcon, iconSize);

  return (
    <div
      ref={previewRef}
      className="flex w-5 h-5 items-center justify-center rounded-sm border border-app-border bg-app-gray"
      style={{ padding }}
    >
      <div
        className="flex items-center justify-center"
        style={{ width: iconSize, height: iconSize }}
      >
        {normalizedIcon}
      </div>
    </div>
  );
}

export default function FileList({ files, preferences }: FileListProps) {
  const {
    selectedFiles,
    setSelectedFiles,
    navigateTo,
    selectionAnchor,
    setSelectionAnchor,
    setSelectionLead,
    pendingRevealTarget,
    setPendingRevealTarget,
    extractArchive,
    openFile,
    isStreamingComplete,
    streamingTotalCount,
    filterText,
  } = useAppStore();
  const { renameTargetPath, setRenameTarget, renameFile } = useAppStore();
  const { startNativeDrag, endNativeDrag, isDraggedDirectory } = useDragStore();
  const [renameText, setRenameText] = useState<string>('');
  const renameInputRef = useRef<HTMLInputElement>(null);
  const { fetchAppIcon } = useAppStore();
  const [draggedFile, setDraggedFile] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const scrollContainerRef = useRef<HTMLDivElement>(null);
  const lastClickRef = useRef<{ path: string; time: number; x: number; y: number } | null>(null);
  const lastHandledDoubleRef = useRef<{ path: string; time: number } | null>(null);

  // Row height for virtual scrolling (py-[2px] + leading-5 = ~24px)
  const ROW_HEIGHT = 24;

  // Clean up dragged state when drag ends
  // No longer needed - native drag handles cleanup

  const sortBy = preferences.sortBy;
  const sortOrder = preferences.sortOrder;
  const toggleSort = (field: typeof preferences.sortBy) => {
    const { updateDirectoryPreferences } = useAppStore.getState();
    if (sortBy === field) {
      updateDirectoryPreferences(useAppStore.getState().currentPath, {
        sortOrder: sortOrder === 'asc' ? 'desc' : 'asc',
      });
    } else {
      const defaultOrder: 'asc' | 'desc' =
        field === 'size' || field === 'modified' ? 'desc' : 'asc';
      updateDirectoryPreferences(useAppStore.getState().currentPath, {
        sortBy: field,
        sortOrder: defaultOrder,
      });
    }
  };

  const { isMac } = usePlatform();

  // Optionally warm cache for a small first screenful
  useEffect(() => {
    if (!isMac) return;
    const initial = files
      .filter((f) => {
        const fileName = f.name.toLowerCase();
        return f.is_directory && fileName.endsWith('.app');
      })
      .slice(0, 6);
    initial.forEach((f) => {
      void fetchAppIcon(f.path, 64);
    });
  }, [isMac, files, fetchAppIcon]);

  const getFileIcon = useFileIcon('small', isMac);

  const getTypeLabel = (file: FileItem) => {
    const name = file.name.toLowerCase();
    let base = 'File';
    if (file.is_directory && name.endsWith('.app')) {
      base = 'Application';
    } else if (file.is_directory) {
      base = 'Folder';
    } else if (file.extension) {
      base = file.extension.toUpperCase();
    }

    return file.is_symlink ? `${base} (symlink)` : base;
  };

  // (moved FilePreview to top-level ListFilePreview to avoid remounting)

  const handleDoubleClick = async (file: FileItem) => {
    const isBundle = isMacOSBundle(file);
    const shouldNavigate = file.is_directory && (!isBundle || file.is_symlink);

    if (shouldNavigate) {
      navigateTo(file.path);
      return;
    }

    const isArchive = !file.is_directory && isArchiveFile(file);

    if (isArchive) {
      await extractArchive(file);
      return;
    }

    await openFile(file);
  };

  // Begin rename UX when store renameTargetPath points to an item in this view
  useEffect(() => {
    if (!renameTargetPath) return;
    const f = files.find((ff) => ff.path === renameTargetPath);
    if (!f) return;
    // Clear first to avoid flashing previous value
    setRenameText('');
    const baseLen = (() => {
      if (f.is_directory) {
        return f.name.toLowerCase().endsWith('.app')
          ? Math.max(0, f.name.length - 4)
          : f.name.length;
      }
      const idx = f.name.lastIndexOf('.');
      return idx > 0 ? idx : f.name.length;
    })();
    const focusAndSelect = () => {
      const el = renameInputRef.current;
      if (!el) return;
      el.focus();
      try {
        el.setSelectionRange(0, baseLen);
      } catch (error) {
        console.warn('Failed to set rename selection range:', error);
      }
    };
    // Fill value on next frame, then select the base name
    requestAnimationFrame(() => {
      setRenameText(f.name);
      requestAnimationFrame(focusAndSelect);
    });
  }, [renameTargetPath, files]);

  const commitRename = async () => {
    const name = (renameText || '').trim();
    if (!name) {
      setRenameTarget(undefined);
      return;
    }
    await renameFile(name);
    // Clear to avoid flashing previous value on next rename
    setRenameText('');
  };
  const cancelRename = () => {
    const el = renameInputRef.current;
    const scroller = el ? (el.closest('.overflow-auto') as HTMLElement | null) : null;
    const top = scroller?.scrollTop ?? 0;
    const left = scroller?.scrollLeft ?? 0;
    setRenameText('');
    setRenameTarget(undefined);
    requestAnimationFrame(() => {
      if (scroller) scroller.scrollTo({ top, left, behavior: 'auto' });
      requestAnimationFrame(() => {
        if (scroller) scroller.scrollTo({ top, left, behavior: 'auto' });
        requestAnimationFrame(() => {
          if (scroller) scroller.scrollTo({ top, left, behavior: 'auto' });
        });
      });
    });
  };

  // Handle mouse down for drag initiation and right-click selection
  const handleMouseDownForFile = (e: React.MouseEvent, file: FileItem) => {
    // If we're renaming this item and the event started in an input, ignore to allow text selection
    const target = e.target as HTMLElement;
    if (
      renameTargetPath === file.path &&
      target &&
      target.closest('input, textarea, [contenteditable="true"]')
    ) {
      e.stopPropagation();
      return;
    }
    // Right-click: Pre-select for context menu
    if (e.button === 2) {
      if (!selectedFiles.includes(file.path)) {
        setSelectedFiles([file.path]);
      }
      return;
    }

    // Left-click: Handle drag initiation
    if (e.button === 0) {
      // Use native drag for both files AND directories
      const startX = e.clientX;
      const startY = e.clientY;
      const dragThreshold = 5; // pixels
      let dragStarted = false;

      const startDrag = () => {
        if (dragStarted) return;
        dragStarted = true;

        // If dragging a directory, track it for potential pinning
        if (file.is_directory) {
          startNativeDrag({ path: file.path, name: file.name });
        }

        // Clean up listeners
        document.removeEventListener('mousemove', onMouseMove);
        document.removeEventListener('mouseup', onMouseUp);

        // Update selection if dragging a non-selected file
        let actualSelectedFiles = selectedFiles;
        if (!selectedFiles.includes(file.path)) {
          actualSelectedFiles = [file.path];
          setSelectedFiles([file.path]);
        }

        // Determine which files to drag using the actual selection
        const selected =
          actualSelectedFiles.includes(file.path) && actualSelectedFiles.length > 0
            ? files.filter((f) => actualSelectedFiles.includes(f.path))
            : [file];

        // Set dragging state for visual feedback
        setDraggedFile(file.path);

        // Perform native drag
        void (async () => {
          try {
            // Create drag preview image with nice icons
            let dragImageDataUrl: string | undefined;
            try {
              // Try to use async version to render SVG icons properly
              const dragVisual = await createDragImageForSelectionAsync(selected, document.body);
              dragImageDataUrl = dragVisual.dataUrl;
            } catch (e) {
              console.warn('Failed to create async drag image, falling back:', e);
              // Fallback to synchronous version
              try {
                const dragVisual = createDragImageForSelection(selected, document.body);
                dragImageDataUrl = dragVisual.dataUrl;
              } catch (e2) {
                console.warn('Failed to create drag image:', e2);
              }
            }

            // Use new unified native drag API
            await invoke('start_native_drag', {
              paths: selected.map((f) => f.path),
              previewImage: dragImageDataUrl,
              dragOffsetY: 0,
            });
          } catch (error) {
            console.warn('Native drag failed:', error);
          } finally {
            // Clear dragging state
            setDraggedFile(null);
            // If we were tracking a directory drag, end it
            if (file.is_directory) {
              endNativeDrag();
            }
          }
        })();
      };

      const onMouseMove = (moveEvent: MouseEvent) => {
        const deltaX = moveEvent.clientX - startX;
        const deltaY = moveEvent.clientY - startY;
        const distance = Math.sqrt(deltaX * deltaX + deltaY * deltaY);

        if (distance >= dragThreshold) {
          startDrag();
        }
      };

      const onMouseUp = () => {
        document.removeEventListener('mousemove', onMouseMove);
        document.removeEventListener('mouseup', onMouseUp);
      };

      // Add temporary listeners to detect movement
      document.addEventListener('mousemove', onMouseMove);
      document.addEventListener('mouseup', onMouseUp);
    }
  };

  const nameCollator = useMemo(
    () => new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' }),
    []
  );
  const sortedFiles = [...files].sort((a, b) => {
    // Treat macOS bundles (.app, .photoslibrary, etc.) as files for sorting purposes
    const aIsBundle = isMacOSBundle(a);
    const bIsBundle = isMacOSBundle(b);
    const aIsFolder = a.is_directory && !aIsBundle;
    const bIsFolder = b.is_directory && !bIsBundle;

    // Optionally sort directories first (but not bundles)
    if (preferences.foldersFirst) {
      if (aIsFolder && !bIsFolder) return -1;
      if (!aIsFolder && bIsFolder) return 1;
    }

    let compareValue = 0;
    switch (preferences.sortBy) {
      case 'name':
        compareValue = nameCollator.compare(a.name, b.name);
        break;
      case 'size':
        compareValue = a.size - b.size;
        break;
      case 'modified':
        compareValue = new Date(a.modified).getTime() - new Date(b.modified).getTime();
        break;
      case 'type':
        compareValue = (a.extension || '').localeCompare(b.extension || '');
        break;
    }

    return preferences.sortOrder === 'asc' ? compareValue : -compareValue;
  });

  const hiddenFiltered = preferences.showHidden
    ? sortedFiles
    : sortedFiles.filter((file) => !file.is_hidden);

  const filteredFiles = filterText
    ? hiddenFiltered.filter((file) =>
        file.name.toLowerCase().includes(filterText.toLowerCase())
      )
    : hiddenFiltered;

  // Virtual scrolling for file rows
  const virtualizer = useVirtualizer({
    count: filteredFiles.length,
    getScrollElement: () => scrollContainerRef.current,
    estimateSize: useCallback(() => ROW_HEIGHT, []),
    overscan: 5,
  });

  useEffect(() => {
    if (!pendingRevealTarget) return;
    const targetPath = pendingRevealTarget;
    if (!filteredFiles.some((file) => file.path === targetPath)) return;

    setSelectedFiles([targetPath]);
    setSelectionAnchor(targetPath);
    setSelectionLead(targetPath);

    requestAnimationFrame(() => {
      const container = listRef.current;
      if (!container) return;
      const nodes = container.querySelectorAll<HTMLElement>('[data-file-item="true"]');
      for (const el of nodes) {
        if (el.getAttribute('data-file-path') === targetPath) {
          try {
            el.scrollIntoView({ block: 'nearest', inline: 'nearest' });
          } catch (error) {
            console.warn('Failed to scroll list reveal target:', error);
          }
          break;
        }
      }
    });

    setPendingRevealTarget(undefined);
  }, [
    pendingRevealTarget,
    filteredFiles,
    setPendingRevealTarget,
    setSelectedFiles,
    setSelectionAnchor,
    setSelectionLead,
  ]);

  if (filteredFiles.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center text-app-muted">
        <div className="text-center">
          <Folder className="w-16 h-16 mx-auto mb-4 opacity-50" />
          {filterText ? (
            <>
              <p className="text-sm">No files match "{filterText}"</p>
              <p className="text-xs mt-1">Press Escape to clear filter</p>
            </>
          ) : (
            <p>This folder is empty</p>
          )}
        </div>
      </div>
    );
  }

  // Click selection handling with Shift/Cmd/Ctrl support
  function handleFileClick(e: React.MouseEvent, file: FileItem) {
    const meta = e.ctrlKey || e.metaKey;
    const shift = e.shiftKey;
    const isPrimary = e.button === 0;

    if (isPrimary && !meta && !shift && !e.altKey) {
      const now = Date.now();
      const last = lastClickRef.current;
      const distance = last
        ? Math.hypot(e.clientX - last.x, e.clientY - last.y)
        : Number.POSITIVE_INFINITY;
      if (last && last.path === file.path && now - last.time <= 350 && distance <= 6) {
        lastClickRef.current = null;
        lastHandledDoubleRef.current = { path: file.path, time: now };
        setSelectedFiles([file.path]);
        setSelectionAnchor(file.path);
        setSelectionLead(file.path);
        void handleDoubleClick(file);
        return;
      }
      lastClickRef.current = { path: file.path, time: now, x: e.clientX, y: e.clientY };
    } else if (isPrimary) {
      lastClickRef.current = { path: file.path, time: Date.now(), x: e.clientX, y: e.clientY };
    } else {
      lastClickRef.current = null;
    }
    const order = filteredFiles.map((f) => f.path);

    if (shift) {
      const anchor =
        selectionAnchor && order.includes(selectionAnchor)
          ? selectionAnchor
          : selectedFiles.length > 0
            ? selectedFiles[selectedFiles.length - 1]
            : undefined;
      if (!anchor || !order.includes(anchor)) {
        // No anchor: add just this item without clearing others
        const merged = Array.from(new Set([...selectedFiles, file.path]));
        setSelectedFiles(merged);
        return;
      }
      const i1 = order.indexOf(anchor);
      const i2 = order.indexOf(file.path);
      if (i1 === -1 || i2 === -1) {
        const merged = Array.from(new Set([...selectedFiles, file.path]));
        setSelectedFiles(merged);
        return;
      }
      const start = Math.min(i1, i2);
      const end = Math.max(i1, i2);
      const range = order.slice(start, end + 1);
      const merged = Array.from(new Set([...selectedFiles, ...range]));
      setSelectedFiles(merged);
      const el = e.currentTarget as HTMLElement;
      if (el && el.scrollIntoView) {
        try {
          el.scrollIntoView({ block: 'nearest', inline: 'nearest' });
        } catch (error) {
          console.warn('Failed to scroll list item into view:', error);
        }
      }
      setSelectionLead(file.path);
      return;
    }

    if (meta) {
      const exists = selectedFiles.includes(file.path);
      const newSelection = exists
        ? selectedFiles.filter((path) => path !== file.path)
        : [...selectedFiles, file.path];
      setSelectedFiles(newSelection);
      setSelectionAnchor(file.path);
      setSelectionLead(file.path);
      return;
    }

    // Single click just selects (no navigation)
    setSelectedFiles([file.path]);
    setSelectionAnchor(file.path);
    setSelectionLead(file.path);
  }

  // Render a single file row (extracted for reuse in virtual rows)
  const renderFileRow = (file: FileItem, virtualIndex: number) => {
    const isSelected = selectedFiles.includes(file.path);
    const isDragged =
      (draggedFile !== null && (draggedFile === file.path || selectedFiles.includes(file.path))) ||
      isDraggedDirectory(file.path);
    // Use virtual index for alternating row colors since we're virtualizing
    const isOdd = virtualIndex % 2 === 1;

    return (
      <div
        key={file.path}
        className={`relative grid grid-cols-12 gap-3 py-[2px] leading-5 text-[13px] cursor-pointer transition-colors duration-75 rounded-full ${
          isSelected
            ? 'bg-accent-selected text-white'
            : isOdd
              ? 'bg-app-gray hover:bg-app-light'
              : 'hover:bg-app-light'
        } ${isDragged ? 'opacity-50' : ''} ${file.is_hidden ? 'opacity-60' : ''}`}
        data-testid="file-item"
        data-file-item="true"
        data-file-path={file.path}
        data-directory={file.is_directory ? 'true' : undefined}
        data-hidden={file.is_hidden ? 'true' : undefined}
        data-name={file.name}
        data-tauri-drag-region={false}
        onClick={(e) => {
          e.stopPropagation();
          handleFileClick(e, file);
        }}
        onDoubleClick={(e) => {
          e.stopPropagation();
          const now = Date.now();
          const recentlyHandled =
            lastHandledDoubleRef.current &&
            lastHandledDoubleRef.current.path === file.path &&
            now - lastHandledDoubleRef.current.time < 500;
          if (recentlyHandled) {
            return;
          }
          lastHandledDoubleRef.current = { path: file.path, time: now };
          setSelectedFiles([file.path]);
          setSelectionAnchor(file.path);
          setSelectionLead(file.path);
          void handleDoubleClick(file);
        }}
        onMouseDown={(e) => handleMouseDownForFile(e, file)}
        draggable={false}
      >
        {/* Name column */}
        <div className="col-span-5 flex items-center gap-2 min-w-0 pl-2 pr-2">
          <span className="relative flex-shrink-0 w-5 h-5">
            <div className="w-full h-full flex items-center justify-center">
              <ListFilePreview file={file} isMac={isMac} fallbackIcon={getFileIcon(file)} />
            </div>
            {file.is_git_repo && <GitRepoBadge size="sm" style={{ bottom: -2, left: -2 }} />}
            {file.is_symlink && <SymlinkBadge size="sm" style={{ bottom: -2, right: -2 }} />}
          </span>
          {renameTargetPath === file.path ? (
            <input
              ref={renameInputRef}
              className={`block flex-1 min-w-0 text-sm font-medium leading-5 h-5 bg-transparent border-0 rounded-none px-0 py-0 m-0 outline-none appearance-none ${isSelected ? 'text-white' : 'text-app-text'} truncate`}
              style={{ fontFamily: 'inherit', transform: 'translateY(-0.5px)' }}
              value={renameText}
              onChange={(e) => setRenameText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault();
                  e.stopPropagation();
                  void commitRename();
                }
                if (e.key === 'Escape') {
                  e.preventDefault();
                  e.stopPropagation();
                  cancelRename();
                }
              }}
              // Prevent row drag/open when interacting with the input
              onMouseDown={(e) => {
                e.stopPropagation();
              }}
              onClick={(e) => {
                e.stopPropagation();
              }}
              onDoubleClick={(e) => {
                e.stopPropagation();
              }}
              onDragStart={(e) => {
                e.stopPropagation();
              }}
              onBlur={cancelRename}
              data-tauri-drag-region={false}
              draggable={false}
            />
          ) : (
            <div className="flex-1 min-w-0 overflow-hidden" data-name-cell="true">
              <FileNameDisplay
                file={file}
                isSelected={isSelected}
                variant="list"
                className="block"
                highlightText={filterText}
              />
            </div>
          )}
        </div>

        {/* Size column */}
        <div
          className={`col-span-2 flex items-center ${isSelected ? 'text-white' : 'text-app-muted'}`}
        >
          {file.is_directory
            ? file.child_count != null
              ? `${file.child_count} item${file.child_count !== 1 ? 's' : ''}`
              : '—'
            : formatFileSize(file.size)}
        </div>

        {/* Type column */}
        <div
          className={`col-span-2 flex items-center ${isSelected ? 'text-white' : 'text-app-muted'}`}
        >
          {getTypeLabel(file)}
        </div>

        {/* Modified column */}
        <div
          className={`col-span-3 flex items-center ${isSelected ? 'text-white' : 'text-app-muted'} whitespace-nowrap`}
        >
          {formatDateFull(file.modified)}
        </div>
      </div>
    );
  };

  return (
    <div className="h-full flex flex-col" ref={listRef} data-testid="file-list">
      {/* Header */}
      <div className="grid grid-cols-12 gap-3 px-3 py-2 border-b border-app-border border-t-0 text-[12px] font-medium text-app-muted bg-transparent select-none flex-shrink-0">
        <button
          className={`col-span-5 text-left hover:text-app-text pl-2 ${sortBy === 'name' ? 'text-app-text' : ''}`}
          onClick={() => toggleSort('name')}
          data-tauri-drag-region={false}
        >
          <span className="inline-flex items-center gap-1">
            Name{' '}
            {sortBy === 'name' &&
              (sortOrder === 'asc' ? (
                <CaretUp className="w-3 h-3" />
              ) : (
                <CaretDown className="w-3 h-3" />
              ))}
          </span>
        </button>
        <button
          className={`col-span-2 text-left hover:text-app-text ${sortBy === 'size' ? 'text-app-text' : ''}`}
          onClick={() => toggleSort('size')}
          data-tauri-drag-region={false}
        >
          <span className="inline-flex items-center gap-1">
            Size{' '}
            {sortBy === 'size' &&
              (sortOrder === 'asc' ? (
                <CaretUp className="w-3 h-3" />
              ) : (
                <CaretDown className="w-3 h-3" />
              ))}
          </span>
        </button>
        <button
          className={`col-span-2 text-left hover:text-app-text ${sortBy === 'type' ? 'text-app-text' : ''}`}
          onClick={() => toggleSort('type')}
          data-tauri-drag-region={false}
        >
          <span className="inline-flex items-center gap-1">
            Type{' '}
            {sortBy === 'type' &&
              (sortOrder === 'asc' ? (
                <CaretUp className="w-3 h-3" />
              ) : (
                <CaretDown className="w-3 h-3" />
              ))}
          </span>
        </button>
        <button
          className={`col-span-3 text-left hover:text-app-text ${sortBy === 'modified' ? 'text-app-text' : ''}`}
          onClick={() => toggleSort('modified')}
          data-tauri-drag-region={false}
        >
          <span className="inline-flex items-center gap-1">
            Modified{' '}
            {sortBy === 'modified' &&
              (sortOrder === 'asc' ? (
                <CaretUp className="w-3 h-3" />
              ) : (
                <CaretDown className="w-3 h-3" />
              ))}
          </span>
        </button>
      </div>

      {/* Virtual scroll container for file rows */}
      <div
        ref={scrollContainerRef}
        className="flex-1 overflow-auto px-3 py-1"
        data-list-scroll-container="true"
      >
        <div
          style={{
            height: `${virtualizer.getTotalSize()}px`,
            width: '100%',
            position: 'relative',
          }}
        >
          {virtualizer.getVirtualItems().map((virtualRow) => {
            const file = filteredFiles[virtualRow.index];
            return (
              <div
                key={virtualRow.key}
                style={{
                  position: 'absolute',
                  top: 0,
                  left: 0,
                  width: '100%',
                  height: `${virtualRow.size}px`,
                  transform: `translateY(${virtualRow.start}px)`,
                }}
              >
                {renderFileRow(file, virtualRow.index)}
              </div>
            );
          })}
        </div>
        {/* Streaming progress indicator */}
        {!isStreamingComplete && (
          <div className="flex items-center justify-center py-4 text-app-muted text-sm gap-2">
            <div className="animate-spin w-4 h-4 border-2 border-accent border-t-transparent rounded-full" />
            <span>
              Loading files...
              {streamingTotalCount != null &&
                ` (${filteredFiles.length} of ~${streamingTotalCount})`}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}

function formatFileSize(bytes: number): string {
  if (bytes === 0) return '0 B';

  const k = 1024;
  const sizes = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));

  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
}

function formatDateFull(dateString: string): string {
  const date = new Date(dateString);
  return new Intl.DateTimeFormat(undefined, {
    year: 'numeric',
    month: 'short',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  }).format(date);
}
