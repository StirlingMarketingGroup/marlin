import { useState, useMemo, useEffect, useLayoutEffect, useRef, type ReactNode } from 'react';
import {
  Folder,
  File,
  ImageSquare,
  MusicNote,
  VideoCamera,
  FileText,
  AppWindow,
  Package,
  FilePdf,
  PaintBrush,
  Palette,
  Disc,
  Cube,
} from 'phosphor-react';
import { FileItem, ViewPreferences } from '../types';
import { useAppStore } from '../store/useAppStore';
import { useDragStore } from '../store/useDragStore';
import AppIcon from '@/components/AppIcon';
import { FileTypeIcon, resolveVSCodeIcon } from '@/components/FileTypeIcon';
import { open } from '@tauri-apps/plugin-shell';

import { invoke } from '@tauri-apps/api/core';
import { createDragImageForSelection, createDragImageForSelectionAsync } from '@/utils/dragImage';
// no direct invoke here; background opens the menu
import { useThumbnail } from '@/hooks/useThumbnail';
import { useVisibility } from '@/hooks/useVisibility';
import FileNameDisplay from './FileNameDisplay';
import SymlinkBadge from '@/components/SymlinkBadge';
import GitRepoBadge from '@/components/GitRepoBadge';

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

  const shouldLoadThumbnail = isImage || isPdf || isAi || isPsd || isStl;
  const requestSize = pickBucket(Math.round((box - pad * 2) * dpr));
  const thumbnailPriority = stage === 'visible' ? 'high' : 'medium';
  const { dataUrl, loading, hasTransparency } = useThumbnail(
    shouldLoadThumbnail && stage !== 'far' ? file.path : undefined,
    { size: requestSize, quality: 'medium', priority: thumbnailPriority }
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

  // Non-image fallback icon (reuse the same padding rule)
  const thumb = Math.max(48, Math.min(tile - pad * 2, 320));
  // Base icons ~48px; target ~50% of thumb at default
  const target = Math.max(32, Math.min(thumb * 0.5, 140));
  const scale = Math.max(0.75, Math.min(2.0, target / 48));
  const iconPixels = 48 * scale;
  const gap = Math.max(0, (thumb - iconPixels) / 2);
  const overlayInset = Math.max(0, Math.min(thumb - 4, gap + Math.min(iconPixels * 0.12, 8)));
  const adjust = Math.max(6, iconPixels * 0.12);
  const verticalInset = Math.max(0, overlayInset - adjust);
  const badgeInsetAdjust = badgeSize === 'lg' ? 3 : badgeSize === 'md' ? 2 : 1;
  const horizontalInset = Math.max(0, verticalInset - badgeInsetAdjust);
  return (
    <div
      ref={previewRef}
      className="relative rounded-md flex items-center justify-center"
      style={{ width: thumb, height: thumb }}
    >
      <div style={{ transform: `scale(${scale})`, transformOrigin: 'center' }}>{fallbackIcon}</div>
      {isGitRepo && (
        <GitRepoBadge size={badgeSize} style={{ bottom: verticalInset, left: horizontalInset }} />
      )}
      {isSymlink && (
        <SymlinkBadge size={badgeSize} style={{ bottom: verticalInset, right: horizontalInset }} />
      )}
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
  } = useAppStore();
  const { renameTargetPath, setRenameTarget, renameFile } = useAppStore();
  const { startNativeDrag, endNativeDrag, isDraggedDirectory } = useDragStore();
  const [renameText, setRenameText] = useState<string>('');
  const [draggedFile, setDraggedFile] = useState<string | null>(null);
  const [hoveredFile, setHoveredFile] = useState<string | null>(null);
  const renameInputRef = useRef<HTMLInputElement>(null);
  const gridRef = useRef<HTMLDivElement>(null);
  const [flashPath, setFlashPath] = useState<string | null>(null);
  const flashTimeoutRef = useRef<number | undefined>(undefined);

  // Clean up dragged state when drag ends
  // No longer needed - native drag handles cleanup

  const [renameInputOffset, setRenameInputOffset] = useState<number>(0);
  const [renameInputWidth, setRenameInputWidth] = useState<number | undefined>(undefined);

  // Tile width from preferences (default 120)
  // Allow full range up to 320 to match ZoomSlider
  const tile = Math.max(80, Math.min(320, preferences.gridSize ?? 120));

  const isMac =
    typeof navigator !== 'undefined' && navigator.platform.toUpperCase().includes('MAC');

  const getFileIcon = (file: FileItem) => {
    // Special-case: macOS files with system icons
    if (isMac) {
      const fileName = file.name.toLowerCase();
      if (file.is_directory && fileName.endsWith('.app')) {
        return (
          <AppIcon
            path={file.path}
            size={64}
            className="w-16 h-16"
            priority="high"
            fallback={<AppWindow className="w-14 h-14 text-accent" />}
          />
        );
      }

      // PKG files use a package icon
      if (fileName.endsWith('.pkg')) {
        return <Package className="w-12 h-12 text-blue-500" weight="fill" />;
      }

      // DMG files use a custom icon since they don't have embedded icons
      if (fileName.endsWith('.dmg')) {
        return <Disc className="w-12 h-12 text-app-muted" weight="fill" />;
      }
    }
    if (file.is_directory) {
      return <Folder className="w-12 h-12 text-accent" weight="fill" />;
    }

    const ext = file.extension?.toLowerCase();
    if (!ext) {
      const special = resolveVSCodeIcon(file.name);
      if (special) return <FileTypeIcon name={file.name} size="large" />;
      return <File className="w-12 h-12 text-app-muted" />;
    }

    // Image files
    if (['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg', 'bmp'].includes(ext)) {
      return <ImageSquare className="w-12 h-12 text-app-green" />;
    }

    // PDF files
    if (ext === 'pdf') {
      return <FilePdf className="w-12 h-12 text-red-500" />;
    }

    // Adobe Illustrator files
    if (ext === 'ai' || ext === 'eps') {
      return <PaintBrush className="w-12 h-12 text-orange-500" />;
    }

    // Photoshop files
    if (ext === 'psd' || ext === 'psb') {
      return <Palette className="w-12 h-12 text-blue-500" />;
    }

    // Audio files
    if (['mp3', 'wav', 'flac', 'aac', 'm4a', 'ogg'].includes(ext)) {
      return <MusicNote className="w-12 h-12 text-app-yellow" />;
    }

    // Video files
    if (['mp4', 'mkv', 'avi', 'mov', 'webm', 'flv'].includes(ext)) {
      return <VideoCamera className="w-12 h-12 text-app-red" />;
    }

    // Archive files
    if (
      [
        'zip',
        'rar',
        '7z',
        '7zip',
        'tar',
        'gz',
        'tgz',
        'bz2',
        'tbz2',
        'xz',
        'txz',
        'zst',
        'lz',
        'lzma',
      ].includes(ext)
    ) {
      return <FileTypeIcon name={file.name} ext={ext} size="large" />;
    }

    // 3D model: STL
    if (ext === 'stl') {
      return <Cube className="w-12 h-12 text-app-green" />;
    }

    // VSCode-style file icons for code/config types
    if (resolveVSCodeIcon(file.name, ext)) {
      return <FileTypeIcon name={file.name} ext={ext} size="large" />;
    }

    // Text files
    if (['txt', 'md', 'json', 'xml', 'yml', 'yaml', 'toml', 'ini'].includes(ext)) {
      return <FileText className="w-12 h-12 text-app-text" />;
    }

    return <File className="w-12 h-12 text-app-muted" />;
  };

  // (moved FilePreview to top-level GridFilePreview to avoid remounting)

  const handleDoubleClick = async (file: FileItem) => {
    const isAppBundle = file.is_directory && file.name.toLowerCase().endsWith('.app');
    const shouldNavigate = file.is_directory && (!isAppBundle || file.is_symlink);

    if (shouldNavigate) {
      navigateTo(file.path);
    } else {
      // Open file or app with system default
      try {
        await open(file.path);
      } catch (error) {
        // Fallback to backend command if plugin shell is unavailable/blocked
        try {
          await invoke('open_path', { path: file.path });
        } catch (err2) {
          console.error('Failed to open file:', error, err2);
        }
      }
    }
  };

  // Handle mouse down for drag initiation and right-click selection
  const handleMouseDownForFile = (e: React.MouseEvent, file: FileItem) => {
    console.log('🖱️ FileGrid: handleMouseDownForFile called', {
      fileName: file.name,
      isDirectory: file.is_directory,
      button: e.button,
      ctrlKey: e.ctrlKey,
      metaKey: e.metaKey,
      altKey: e.altKey,
    });

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

        console.log('🚀 FileGrid: Native drag started for', {
          fileName: file.name,
          isDirectory: file.is_directory,
          selectedFiles: selectedFiles.length,
        });

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
            const result = await invoke('start_native_drag', {
              paths: selected.map((f) => f.path),
              previewImage: dragImageDataUrl,
              dragOffsetY: 0,
            });

            console.log('🏁 FileGrid: Native drag completed', result);
          } catch (error) {
            console.warn('Native drag failed:', error);
          } finally {
            // Clear dragging state and hover state
            setDraggedFile(null);
            setHoveredFile(null);
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

  const nameCollator = useMemo(
    () => new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' }),
    []
  );
  const sortedFiles = [...files].sort((a, b) => {
    // Treat .app as files for sorting purposes
    const aIsApp = a.is_directory && a.name.toLowerCase().endsWith('.app');
    const bIsApp = b.is_directory && b.name.toLowerCase().endsWith('.app');
    const aIsFolder = a.is_directory && !aIsApp;
    const bIsFolder = b.is_directory && !bIsApp;

    // Optionally sort directories first (but not .app files)
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

  const filteredFiles = preferences.showHidden
    ? sortedFiles
    : sortedFiles.filter((file) => !file.is_hidden);

  useEffect(() => {
    return () => {
      if (flashTimeoutRef.current) {
        window.clearTimeout(flashTimeoutRef.current);
        flashTimeoutRef.current = undefined;
      }
    };
  }, []);

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

    setFlashPath(targetPath);
    if (flashTimeoutRef.current) window.clearTimeout(flashTimeoutRef.current);
    flashTimeoutRef.current = window.setTimeout(() => {
      setFlashPath((prev) => (prev === targetPath ? null : prev));
      flashTimeoutRef.current = undefined;
    }, 1600);

    setPendingRevealTarget(undefined);
  }, [pendingRevealTarget, files, setPendingRevealTarget, setSelectedFiles, setSelectionAnchor, setSelectionLead]);

  if (filteredFiles.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center text-app-muted">
        <div className="text-center">
          <Folder className="w-16 h-16 mx-auto mb-4 opacity-50" />
          <p>This folder is empty</p>
        </div>
      </div>
    );
  }

  const handleBackgroundClick = (e: React.MouseEvent) => {
    // Only clear when the click is directly on the background container
    if (e.target === e.currentTarget) {
      setSelectedFiles([]);
    }
  };

  // Click selection handling with Shift/Cmd/Ctrl support
  function handleFileClick(e: React.MouseEvent, file: FileItem) {
    const meta = e.ctrlKey || e.metaKey;
    const shift = e.shiftKey;
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

  return (
    <div className="p-2" onClick={handleBackgroundClick} ref={gridRef}>
      <div
        className="grid gap-2 file-grid"
        style={{
          gridTemplateColumns: `repeat(auto-fill, minmax(${tile + 24}px, 1fr))`,
        }}
      >
        {filteredFiles.map((file) => {
          const isSelected = selectedFiles.includes(file.path);
          const isDragged =
            (draggedFile !== null &&
              (draggedFile === file.path || selectedFiles.includes(file.path))) ||
            isDraggedDirectory(file.path);
          const isRenaming = renameTargetPath === file.path;

          return (
            <div
              key={file.path}
              className={`relative flex flex-col items-center px-3 py-2 rounded-md cursor-pointer transition-all duration-75 ${
                isSelected || isRenaming
                  ? 'bg-accent-selected z-20 overflow-visible'
                  : hoveredFile === file.path
                    ? 'bg-app-light/70'
                    : ''
              } ${isDragged ? 'opacity-50' : ''} ${file.is_hidden ? 'opacity-60' : ''}`}
              data-file-item="true"
              data-file-path={file.path}
              data-tauri-drag-region={false}
              style={
                flashPath === file.path
                  ? { outline: '2px solid var(--color-accent)', outlineOffset: '2px' }
                  : undefined
              }
              onClick={(e) => {
                e.stopPropagation();
                handleFileClick(e, file);
              }}
              onDoubleClick={(e) => {
                e.stopPropagation();
                handleDoubleClick(file);
              }}
              onMouseDown={(e) => handleMouseDownForFile(e, file)}
              onMouseEnter={() => setHoveredFile(file.path)}
              onMouseLeave={() => setHoveredFile(null)}
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
                    style={{ margin: '0 auto' }}
                  />
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
