import {
  useState,
  useMemo,
  useEffect,
  useLayoutEffect,
  useRef,
  useCallback,
  type ReactNode,
} from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { Play, AppWindow, Folder } from 'phosphor-react';
import { FileItem, ViewPreferences } from '../types';
import { useAppStore } from '../store/useAppStore';
import { useDragStore } from '../store/useDragStore';
import AppIcon from '@/components/AppIcon';
import { invoke } from '@tauri-apps/api/core';
import { createDragImageForSelection, createDragImageForSelectionAsync } from '@/utils/dragImage';
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
import { useScrollContainerRef } from '@/contexts/ScrollContext';
import { useSortedFiles } from '@/hooks/useSortedFiles';

interface FileGridProps {
  files: FileItem[];
  preferences: ViewPreferences;
}

// Stable, top-level preview component to avoid remount flicker
function GridFilePreview({
  file,
  isMac,
  fallbackIcon,
  tile,
  isSymlink,
  isGitRepo,
}: {
  file: FileItem;
  isMac: boolean;
  fallbackIcon: ReactNode;
  tile: number;
  isSymlink: boolean;
  isGitRepo: boolean;
}) {
  const { ref: previewRef, stage } = useVisibility<HTMLDivElement>({
    nearMargin: '900px',
    visibleMargin: '0px',
  });
  const ext = file.extension?.toLowerCase();
  const isImage =
    !!ext &&
    ['jpg', 'jpeg', 'png', 'gif', 'webp', 'bmp', 'tiff', 'tga', 'ico', 'svg'].includes(ext || '');
  const isPdf = ext === 'pdf';
  const isAi = ext === 'ai' || ext === 'eps';
  const isPsd = ext === 'psd' || ext === 'psb';

  const isStl = ext === 'stl';
  const isVideo = isVideoExtension(ext);
  const isAppBundle = isMac && file.is_directory && file.name.toLowerCase().endsWith('.app');

  const badgeSize: 'sm' | 'md' | 'lg' = tile >= 200 ? 'lg' : tile >= 120 ? 'md' : 'sm';
  const badgeOffset =
    tile >= 200 ? 'bottom-3 left-3' : tile >= 140 ? 'bottom-2 left-2' : 'bottom-1 left-1';
  const gitBadgeOffset =
    tile >= 200 ? 'bottom-3 right-3' : tile >= 140 ? 'bottom-2 right-2' : 'bottom-1 right-1';

  // (Rendering handled below with a fixed preview box for alignment)

  // Device pixel ratio quantized to 1 or 2 for cache reuse
  const dpr =
    typeof window !== 'undefined'
      ? window.devicePixelRatio && window.devicePixelRatio > 1.5
        ? 2
        : 1
      : 1;
  // Padding is small and stable (more noticeable for breathing room)
  const pad = Math.max(3, Math.min(8, Math.round(tile * 0.03)));
  // Fixed preview box to keep rows aligned
  const box = Math.max(48, Math.min(tile - pad * 2, 320));

  // Bucket request sizes to improve cache hits across navigation and zoom
  const pickBucket = (target: number) => {
    const buckets = [64, 96, 128, 192, 256, 320, 384, 512];
    let best = buckets[0];
    let bestDiff = Math.abs(buckets[0] - target);
    for (let i = 1; i < buckets.length; i++) {
      const diff = Math.abs(buckets[i] - target);
      if (diff < bestDiff) {
        best = buckets[i];
        bestDiff = diff;
      }
    }
    return best;
  };

  const shouldLoadThumbnail = isImage || isPdf || isAi || isPsd || isStl || isVideo;
  const requestSize = pickBucket(Math.round((box - pad * 2) * dpr));
  const thumbnailPriority = stage === 'visible' ? 'high' : 'medium';
  const { dataUrl, loading, hasTransparency } = useThumbnail(
    shouldLoadThumbnail && stage !== 'far' ? file.path : undefined,
    { size: requestSize, quality: 'medium', priority: thumbnailPriority, thumbnailUrl: file.thumbnail_url }
  );

  // Image-like previews (real thumbnails)
  if (shouldLoadThumbnail) {
    if (dataUrl) {
      return (
        <div
          ref={previewRef}
          className={`relative rounded-md border border-app-border ${hasTransparency ? 'bg-checker' : ''} overflow-hidden`}
          style={{
            width: box,
            height: box,
            padding: pad,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
          }}
        >
          {isGitRepo && <GitRepoBadge className={gitBadgeOffset} size={badgeSize} />}
          {isSymlink && <SymlinkBadge className={badgeOffset} size={badgeSize} />}
          <img
            src={dataUrl}
            alt={file.name}
            className={`max-w-full max-h-full w-full h-full`}
            style={{ objectFit: 'contain', transform: 'none' }}
            draggable={false}
          />
          {isVideo && (
            <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
              <div className="flex items-center justify-center rounded-full bg-black/45 p-3">
                <Play weight="fill" className="h-6 w-6 text-white/90" />
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
          className="relative rounded-md border border-app-border animate-pulse"
          style={{ width: box, height: box, padding: pad }}
        >
          {isGitRepo && <GitRepoBadge className={gitBadgeOffset} size={badgeSize} />}
          {isSymlink && <SymlinkBadge className={badgeOffset} size={badgeSize} />}
        </div>
      );
    }
    return (
      <div
        ref={previewRef}
        className="relative rounded-md border border-app-border"
        style={{ width: box, height: box, padding: pad }}
      >
        {isGitRepo && <GitRepoBadge className={gitBadgeOffset} size={badgeSize} />}
        {isSymlink && <SymlinkBadge className={badgeOffset} size={badgeSize} />}
      </div>
    );
  }

  // macOS .app Application icons (native icons)
  if (isAppBundle) {
    const requestSize = pickBucket(Math.round((box - pad * 2) * dpr));
    if (stage === 'far') {
      return (
        <div
          ref={previewRef}
          className="relative overflow-hidden rounded-md border border-app-border bg-checker"
          style={{ width: box, height: box, padding: pad }}
        >
          {isGitRepo && <GitRepoBadge className={gitBadgeOffset} size={badgeSize} />}
          {isSymlink && <SymlinkBadge className={badgeOffset} size={badgeSize} />}
        </div>
      );
    }
    return (
      <div
        ref={previewRef}
        className="relative overflow-hidden"
        style={{ width: box, height: box, padding: pad }}
      >
        {isGitRepo && <GitRepoBadge className={gitBadgeOffset} size={badgeSize} />}
        {isSymlink && <SymlinkBadge className={badgeOffset} size={badgeSize} />}
        <AppIcon
          path={file.path}
          size={requestSize}
          className="w-full h-full"
          priority={stage === 'visible' ? 'high' : 'medium'}
          fallback={<AppWindow className="w-14 h-14 text-accent" />}
        />
      </div>
    );
  }

  // Non-thumbnail fallback icons should share the same footprint as thumbnails
  const contentSize = Math.max(32, box - pad * 2);
  const iconSize = Math.round(Math.min(contentSize, Math.max(36, contentSize * 0.85)));
  const normalizedIcon = normalizePreviewIcon(fallbackIcon, iconSize);
  return (
    <div
      ref={previewRef}
      className="relative overflow-hidden rounded-md border border-app-border bg-app-gray flex items-center justify-center"
      style={{ width: box, height: box, padding: pad }}
    >
      <div className="flex items-center justify-center w-full h-full">
        <div
          className="flex items-center justify-center"
          style={{ width: iconSize, height: iconSize }}
        >
          {normalizedIcon}
        </div>
      </div>
      {isGitRepo && <GitRepoBadge className={gitBadgeOffset} size={badgeSize} />}
      {isSymlink && <SymlinkBadge className={badgeOffset} size={badgeSize} />}
    </div>
  );
}

export default function FileGrid({ files, preferences }: FileGridProps) {
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
  const [draggedFile, setDraggedFile] = useState<string | null>(null);
  const renameInputRef = useRef<HTMLInputElement>(null);
  const gridRef = useRef<HTMLDivElement>(null);
  const lastClickRef = useRef<{ path: string; time: number; x: number; y: number } | null>(null);
  const lastHandledDoubleRef = useRef<{ path: string; time: number } | null>(null);

  // Clean up dragged state when drag ends
  // No longer needed - native drag handles cleanup

  const [renameInputOffset, setRenameInputOffset] = useState<number>(0);
  const [renameInputWidth, setRenameInputWidth] = useState<number | undefined>(undefined);

  // Tile width from preferences (default 120)
  // Allow full range up to 320 to match ZoomSlider
  const tile = Math.max(80, Math.min(320, preferences.gridSize ?? 120));

  // Container dimensions for virtual scrolling
  const parentRef = useRef<HTMLDivElement>(null);
  const scrollContainerRef = useScrollContainerRef();
  const [containerWidth, setContainerWidth] = useState(0);

  // Measure container width for column calculation
  useLayoutEffect(() => {
    const el = parentRef.current;
    if (!el) return;

    let rafId: number | undefined;

    const measureWidth = () => {
      if (el.clientWidth > 0) {
        setContainerWidth(el.clientWidth);
      } else {
        // Element not yet laid out, retry on next frame
        rafId = requestAnimationFrame(measureWidth);
      }
    };

    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        if (entry.contentRect.width > 0) {
          setContainerWidth(entry.contentRect.width);
        }
      }
    });

    observer.observe(el);
    // Initial measurement with retry for race conditions
    measureWidth();

    return () => {
      observer.disconnect();
      if (rafId !== undefined) {
        cancelAnimationFrame(rafId);
      }
    };
  }, []);

  // Safety: re-measure if width is still 0 when files change (navigation)
  useEffect(() => {
    if (containerWidth === 0 && parentRef.current) {
      const width = parentRef.current.clientWidth;
      if (width > 0) {
        setContainerWidth(width);
      }
    }
  }, [containerWidth, files]);

  // Calculate grid layout constants
  const gap = 8; // gap-2 = 8px
  const itemWidth = tile + 24 + gap; // tile + padding + gap
  const columnCount = Math.max(1, Math.floor((containerWidth + gap) / itemWidth));

  // Row height calculation - must be >= actual rendered height to prevent overlap
  const previewPad = Math.max(3, Math.min(8, Math.round(tile * 0.03)));
  const previewHeight = Math.max(48, Math.min(tile - previewPad * 2, 320));
  // Text area: filename (2 lines ~40px) + size (~16px) + dimensions (~16px) + spacing
  const textHeight = 80;
  const rowPadding = 16; // py-2 = 8px top + 8px bottom
  const verticalGap = gap; // Match horizontal gap (8px)
  const rowHeight = previewHeight + textHeight + rowPadding + verticalGap;

  const { isMac } = usePlatform();

  const getFileIcon = useFileIcon('large', isMac);

  // (moved FilePreview to top-level GridFilePreview to avoid remounting)

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
      // Track directories for potential pinning to sidebar
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

  // Begin rename UX when store renameTargetPath points to an item in this view
  useEffect(() => {
    if (!renameTargetPath) return;
    const f = files.find((ff) => ff.path === renameTargetPath);
    if (!f) return;
    // Clear first to avoid flashing a previous value
    setRenameText('');
    const baseLen = (() => {
      if (f.is_directory)
        return f.name.toLowerCase().endsWith('.app')
          ? Math.max(0, f.name.length - 4)
          : f.name.length;
      const idx = f.name.lastIndexOf('.');
      return idx > 0 ? idx : f.name.length;
    })();
    const focusAndSelect = () => {
      const el = renameInputRef.current;
      if (!el) return;
      el.focus();
      try {
        if (el.value !== f.name) el.value = f.name;
        el.setSelectionRange(0, baseLen);
      } catch (error) {
        console.warn('Failed to preset rename selection:', error);
      }
    };
    // Fill value on next frame, then select the base name
    requestAnimationFrame(() => {
      setRenameText(f.name);
      requestAnimationFrame(focusAndSelect);
    });
  }, [renameTargetPath, files]);

  // Auto-size and edge-aware centering for the rename input in grid view
  useLayoutEffect(() => {
    if (!renameTargetPath) return;
    const el = renameInputRef.current;
    if (!el) return;

    const computeLayout = () => {
      const tileEl = el.closest('[data-file-item="true"]') as HTMLElement | null;
      const tileRect = tileEl ? tileEl.getBoundingClientRect() : undefined;
      const centerX = tileRect ? tileRect.left + tileRect.width / 2 : window.innerWidth / 2;
      // Use nearest overflow-auto scroller to avoid overlapping sidebar
      const scroller =
        (tileEl && (tileEl.closest('.overflow-auto') as HTMLElement | null)) || undefined;
      const containerRect = scroller?.getBoundingClientRect();
      const margin = 16;

      // Measure text width using canvas to compute desired width
      const cs = window.getComputedStyle(el);
      const font = `${cs.fontStyle} ${cs.fontVariant || 'normal'} ${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`;
      const canvas = document.createElement('canvas');
      const ctx = canvas.getContext('2d');
      let textWidth = 0;
      if (ctx) {
        ctx.font = font;
        textWidth = Math.ceil(ctx.measureText(renameText || '').width);
      } else {
        textWidth = (renameText || '').length * (parseFloat(cs.fontSize) || 13) * 0.6;
      }

      const horizPad = 8 + 8 + 1 + 1; // px-2 padding + 1px borders
      const desiredWidth = Math.max(tile, textWidth + horizPad);
      const minX = (containerRect?.left ?? 0) + margin;
      const maxX = (containerRect?.right ?? window.innerWidth) - margin;
      const containerAvail = Math.max(0, maxX - minX);
      const width = Math.min(desiredWidth, containerAvail);

      // Shift left/right to keep entire input within container instead of shrinking first
      const minCenter = minX + width / 2;
      const maxCenter = maxX - width / 2;
      const clampedCenter = Math.max(minCenter, Math.min(maxCenter, centerX));
      const offset = clampedCenter - centerX;

      setRenameInputWidth(width);
      setRenameInputOffset(offset);
    };

    computeLayout();
    const onResize = () => computeLayout();
    window.addEventListener('resize', onResize);
    const tileEl = el.closest('[data-file-item="true"]') as HTMLElement | null;
    const scroller =
      (tileEl && (tileEl.closest('.overflow-auto') as HTMLElement | null)) || undefined;
    const onScroll = () => computeLayout();
    if (scroller) scroller.addEventListener('scroll', onScroll);
    return () => {
      window.removeEventListener('resize', onResize);
      if (scroller) scroller.removeEventListener('scroll', onScroll);
    };
  }, [renameTargetPath, renameText, tile]);

  const commitRename = async () => {
    const name = (renameText || '').trim();
    if (!name) {
      setRenameTarget(undefined);
      return;
    }
    await renameFile(name);
    // Clear local value to avoid flashing on next rename session
    setRenameText('');
  };
  const cancelRename = () => {
    // Preserve scroll position of the main scroller to prevent jump-to-top
    const el = renameInputRef.current;
    const scroller = el ? (el.closest('.overflow-auto') as HTMLElement | null) : null;
    const top = scroller?.scrollTop ?? 0;
    const left = scroller?.scrollLeft ?? 0;
    // Clear local value and sizing so the next open starts empty and measured fresh
    setRenameText('');
    setRenameInputWidth(undefined);
    setRenameInputOffset(0);
    setRenameTarget(undefined);
    // Restore on next frame after reflow
    requestAnimationFrame(() => {
      if (scroller) scroller.scrollTo({ top, left, behavior: 'auto' });
      // Do another frame to counter subsequent layout shifts
      requestAnimationFrame(() => {
        if (scroller) scroller.scrollTo({ top, left, behavior: 'auto' });
        // One more just in case React updates late on some platforms
        requestAnimationFrame(() => {
          if (scroller) scroller.scrollTo({ top, left, behavior: 'auto' });
        });
      });
    });
  };

  // Use shared hook for streaming-aware sorting
  const sortedFiles = useSortedFiles(files, preferences);

  const hiddenFiltered = preferences.showHidden
    ? sortedFiles
    : sortedFiles.filter((file) => !file.is_hidden);

  const filteredFiles = filterText
    ? hiddenFiltered.filter((file) => file.name.toLowerCase().includes(filterText.toLowerCase()))
    : hiddenFiltered;

  // Group files into rows for virtual scrolling
  const rows = useMemo(() => {
    if (columnCount === 0) return [];
    const result: FileItem[][] = [];
    for (let i = 0; i < filteredFiles.length; i += columnCount) {
      result.push(filteredFiles.slice(i, i + columnCount));
    }
    return result;
  }, [filteredFiles, columnCount]);

  // Stable callback for scroll element to prevent virtualizer resets on re-render
  const getScrollElement = useCallback(
    () => scrollContainerRef?.current ?? null,
    [scrollContainerRef]
  );

  // Virtual scrolling for rows - use shared scroll container from MainPanel
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement,
    estimateSize: useCallback(() => rowHeight, [rowHeight]),
    overscan: 3,
  });

  useEffect(() => {
    if (!pendingRevealTarget) return;
    const targetPath = pendingRevealTarget;
    if (!files.some((file) => file.path === targetPath)) return;

    setSelectedFiles([targetPath]);
    setSelectionAnchor(targetPath);
    setSelectionLead(targetPath);

    requestAnimationFrame(() => {
      const container = gridRef.current;
      if (!container) return;
      const candidates = container.querySelectorAll<HTMLElement>('[data-file-item="true"]');
      for (const el of candidates) {
        if (el.getAttribute('data-file-path') === targetPath) {
          try {
            el.scrollIntoView({ block: 'nearest', inline: 'nearest' });
          } catch (error) {
            console.warn('Failed to scroll reveal target into view:', error);
          }
          break;
        }
      }
    });

    setPendingRevealTarget(undefined);
  }, [
    pendingRevealTarget,
    files,
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
              <p className="text-sm">No files match &ldquo;{filterText}&rdquo;</p>
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
        const mergedNoAnchor = Array.from(new Set([...selectedFiles, file.path]));
        setSelectedFiles(mergedNoAnchor);
        return;
      }
      const i1 = order.indexOf(anchor);
      const i2 = order.indexOf(file.path);
      if (i1 === -1 || i2 === -1) {
        const mergedMissing = Array.from(new Set([...selectedFiles, file.path]));
        setSelectedFiles(mergedMissing);
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
          console.warn('Failed to scroll clicked item into view:', error);
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

  // Render a single file item (extracted for reuse in virtual rows)
  const renderFileItem = (file: FileItem) => {
    const isSelected = selectedFiles.includes(file.path);
    const isDragged =
      (draggedFile !== null && (draggedFile === file.path || selectedFiles.includes(file.path))) ||
      isDraggedDirectory(file.path);
    const isRenaming = renameTargetPath === file.path;

    return (
      <div
        key={file.path}
        className={`relative flex flex-col items-center px-3 py-2 rounded-md cursor-pointer transition-all duration-75 ${
          isSelected || isRenaming
            ? 'bg-accent-selected z-20 overflow-visible'
            : 'hover:bg-app-light/70'
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
        <div
          className="mb-2 flex-shrink-0"
          style={{
            width: tile,
            display: 'flex',
            justifyContent: 'center',
            height: Math.max(
              48,
              Math.min(tile - Math.max(3, Math.min(8, Math.round(tile * 0.03))) * 2, 320)
            ),
          }}
        >
          <GridFilePreview
            file={file}
            isMac={isMac}
            fallbackIcon={getFileIcon(file)}
            tile={tile}
            isSymlink={file.is_symlink}
            isGitRepo={file.is_git_repo}
          />
        </div>

        {isRenaming ? (
          <div
            className="text-center w-full relative overflow-visible"
            style={{ height: '1.25rem' }}
          >
            <input
              ref={renameInputRef}
              className={`text-sm font-medium bg-app-dark border border-app-border rounded px-2 py-[2px] ${isSelected ? 'text-white' : ''} whitespace-nowrap absolute left-1/2 text-center`}
              style={{
                minWidth: `${tile}px`,
                width: renameInputWidth ? `${renameInputWidth}px` : `${tile}px`,
                transform: `translateX(-50%) translateX(${renameInputOffset}px)`,
                zIndex: 50,
              }}
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
              // Prevent grid item drag/open when interacting with the input
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
          </div>
        ) : (
          <div className="text-center w-full">
            <FileNameDisplay
              file={file}
              maxWidth={tile - 16} // Account for padding
              isSelected={isSelected}
              variant="grid"
              showSize={true}
              highlightText={filterText}
              style={{ margin: '0 auto' }}
            />
          </div>
        )}
      </div>
    );
  };

  return (
    <div ref={parentRef} className="p-2" data-testid="file-grid" data-grid-scroll-container="true">
      <div ref={gridRef}>
        {/* Virtual scroll container */}
        <div
          style={{
            height: `${virtualizer.getTotalSize()}px`,
            width: '100%',
            position: 'relative',
          }}
        >
          {virtualizer.getVirtualItems().map((virtualRow) => {
            const rowFiles = rows[virtualRow.index];
            return (
              <div
                key={virtualRow.key}
                style={{
                  position: 'absolute',
                  top: 0,
                  left: 0,
                  width: '100%',
                  transform: `translateY(${virtualRow.start}px)`,
                }}
              >
                <div
                  className="grid gap-2 file-grid"
                  style={{
                    gridTemplateColumns:
                      columnCount > 0
                        ? `repeat(${columnCount}, 1fr)`
                        : `repeat(auto-fill, minmax(${tile + 24}px, 1fr))`,
                  }}
                >
                  {rowFiles.map((file) => renderFileItem(file))}
                </div>
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
