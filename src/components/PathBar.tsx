import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ChangeEvent,
} from 'react';
import {
  CaretLeft,
  CaretRight,
  SquaresFour,
  List,
  Folder,
  ArrowUp,
  ArrowClockwise,
  Minus,
  Square,
  CopySimple,
  X,
} from 'phosphor-react';
import { invoke } from '@tauri-apps/api/core';
import type { DirectoryListingResponse, FileItem } from '@/types';
import { useAppStore } from '@/store/useAppStore';
import { useToastStore } from '@/store/useToastStore';
import { getCurrentWindow } from '@tauri-apps/api/window';
import ZoomSlider from './ZoomSlider';
import UpdateNotice from '@/components/UpdateNotice';
import GitRepoBadge from '@/components/GitRepoBadge';
import SymlinkBadge from '@/components/SymlinkBadge';
import { useDragStore } from '@/store/useDragStore';
import { formatArchivePathForDisplay, isArchiveUri } from '@/utils/archiveUri';
import { dirname, resolveTypedPath } from '@/utils/pathUtils';

/** Get parent directory path — returns null for root */
const getParentPath = (path: string): string | null => {
  if (!path || path === '/') return null;
  const parent = dirname(path);
  // dirname returns '/' for root-level files, which is fine;
  // but if the input was already root-level, nothing to go up to.
  return parent === path ? null : parent;
};

const MAX_SUGGESTIONS = 50;

interface PathSuggestion {
  value: string;
  display: string;
}

const normalizeForCompare = (value: string): string => {
  if (!value) return '/';
  const replaced = value.replace(/\\/g, '/');
  const trimmed = replaced.length > 1 && replaced.endsWith('/') ? replaced.slice(0, -1) : replaced;
  return trimmed || '/';
};

export default function PathBar() {
  const {
    currentPath,
    navigateTo,
    showZoomSliderNow,
    scheduleHideZoomSlider,
    showZoomSlider,
    files,
    loading,
    homeDir,
    globalPreferences,
    directoryPreferences,
  } = useAppStore();

  const [editPath, setEditPath] = useState(currentPath);
  const [activeSuggestion, setActiveSuggestion] = useState(-1);
  const [isFocused, setIsFocused] = useState(false);
  // The typed text split at its last separator: the folder to list and the start of a name in it.
  // `folder` is that folder as a location, in the form `normalizeForCompare` gives.
  const [query, setQuery] = useState<{
    folderText: string;
    typedName: string;
    folder: string;
  } | null>(null);
  // The folder listed for suggestions when it is not the current folder.
  // `entries` is null while the request is out. A failed request leaves it empty.
  const [listing, setListing] = useState<{ folder: string; entries: FileItem[] | null } | null>(
    null
  );
  const [isMaximized, setIsMaximized] = useState(false);
  const [remoteDragGhost, setRemoteDragGhost] = useState<{
    x: number;
    y: number;
    label: string;
  } | null>(null);
  const userAgent = typeof navigator !== 'undefined' ? navigator.userAgent.toUpperCase() : '';
  const platform = typeof navigator !== 'undefined' ? navigator.platform.toUpperCase() : '';
  const isLinux = userAgent.includes('LINUX');
  const isMacPlatform = platform.includes('MAC');

  const inputRef = useRef<HTMLInputElement>(null);
  const suggestionListRef = useRef<HTMLDivElement>(null);
  const [currentDirMeta, setCurrentDirMeta] = useState<FileItem | null>(null);
  const windowRef = useRef(getCurrentWindow());
  const startNativeDrag = useDragStore((state) => state.startNativeDrag);
  const endNativeDrag = useDragStore((state) => state.endNativeDrag);
  const setNativeDragPaths = useDragStore((state) => state.setNativeDragPaths);
  const clearNativeDragPaths = useDragStore((state) => state.clearNativeDragPaths);
  const setInAppDropTargetId = useDragStore((state) => state.setInAppDropTargetId);
  const dropTargetPath = useDragStore((state) => state.dropTargetPath);

  const parentPath = getParentPath(currentPath);
  const isUpButtonDropTarget = parentPath && dropTargetPath === parentPath;

  const displayPath = isArchiveUri(currentPath)
    ? formatArchivePathForDisplay(currentPath)
    : currentPath;

  useEffect(() => {
    let cancelled = false;

    (async () => {
      try {
        const metadata = await invoke<FileItem>('get_file_metadata', { path: currentPath });
        if (!cancelled) {
          setCurrentDirMeta(metadata);
        }
      } catch {
        if (!cancelled) {
          setCurrentDirMeta(null);
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [currentPath]);

  const suggestions = useMemo<PathSuggestion[]>(() => {
    if (!query) return [];
    const { folderText, typedName, folder } = query;
    // While the current folder loads, `files` can still hold the folder before it.
    const entries =
      folder === normalizeForCompare(currentPath)
        ? loading
          ? []
          : files
        : listing?.folder === folder
          ? (listing.entries ?? [])
          : [];
    const separator = folderText.slice(-1);
    const typedLower = typedName.toLocaleLowerCase();
    const showHidden =
      typedName.startsWith('.') ||
      (directoryPreferences[folder]?.showHidden ?? globalPreferences.showHidden);
    return entries
      .filter(
        (entry) =>
          entry.is_directory &&
          (showHidden || !entry.is_hidden) &&
          entry.name.toLocaleLowerCase().startsWith(typedLower)
      )
      .sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }))
      .slice(0, MAX_SUGGESTIONS)
      .map((entry) => ({
        value: `${folderText}${entry.name}${separator}`,
        display: `${entry.name}${separator}`,
      }));
  }, [
    query,
    currentPath,
    loading,
    files,
    listing,
    directoryPreferences,
    globalPreferences.showHidden,
  ]);

  const closeSuggestions = useCallback(() => {
    setQuery(null);
    setActiveSuggestion(-1);
  }, []);

  // The one place that turns text into a query. The caret is at the end of `text`.
  const suggestFor = (text: string) => {
    setActiveSuggestion(-1);
    const lastSlash = Math.max(text.lastIndexOf('/'), text.lastIndexOf('\\'));
    // An archive address keeps its inner path in a query parameter, so its text has no folder to list.
    if (lastSlash < 0 || isArchiveUri(text)) {
      setQuery(null);
      return;
    }
    const folderText = text.slice(0, lastSlash + 1);
    // The folder is listed as the location Enter would open, so `~/` and a Windows share path list too.
    const folderLocation = resolveTypedPath(folderText, homeDir);
    const folder = normalizeForCompare(folderLocation);
    setQuery({ folderText, typedName: text.slice(lastSlash + 1), folder });

    if (folder === normalizeForCompare(currentPath) || folder === listing?.folder) return;
    // One listing per folder: later edits in the same folder wait on the same request.
    setListing({ folder, entries: null });
    void invoke<DirectoryListingResponse>('read_directory', { path: folderLocation })
      .catch((error) => {
        console.warn('Failed to fetch path suggestions:', error);
        return { entries: [] };
      })
      .then(({ entries }) =>
        setListing((current) => (current?.folder === folder ? { folder, entries } : current))
      );
  };

  const completeWith = (suggestion: PathSuggestion) => {
    setEditPath(suggestion.value);
    suggestFor(suggestion.value);
  };

  const handleFocus = () => {
    setIsFocused(true);
    // A folder listed during an earlier edit may have changed since.
    setListing(null);
  };

  const handleBlur = () => {
    closeSuggestions();
    // Switching to another app blurs the input but leaves it the active element.
    // Keep the draft for when the user comes back.
    if (document.activeElement === inputRef.current) return;
    setIsFocused(false);
    setEditPath(currentPath);
  };

  const handleInputChange = (e: ChangeEvent<HTMLInputElement>) => {
    const text = e.target.value;
    setEditPath(text);
    // A suggestion completes the end of the text, so it can never replace what follows the caret.
    if (e.target.selectionStart === text.length) {
      suggestFor(text);
    } else {
      closeSuggestions();
    }
  };

  const handleKeyDown = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    const listOpen = suggestions.length > 0;

    if (!listOpen && e.key === 'ArrowDown') {
      // With a draft in the bar, Down waits for suggestions: moving to the files would drop the draft.
      if (editPath !== currentPath) {
        e.preventDefault();
        return;
      }
      const input = inputRef.current;
      if (input) {
        input.blur();
        e.preventDefault();
        e.stopPropagation();
        requestAnimationFrame(() => {
          const forwardedEvent = new KeyboardEvent('keydown', {
            key: 'ArrowDown',
            code: 'ArrowDown',
            bubbles: true,
          });
          window.dispatchEvent(forwardedEvent);
        });
      }
      return;
    }

    if (listOpen) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setActiveSuggestion(activeSuggestion + 1 >= suggestions.length ? 0 : activeSuggestion + 1);
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        setActiveSuggestion(Math.max(activeSuggestion - 1, -1));
        return;
      }
      if (e.key === 'Escape') {
        closeSuggestions();
        return;
      }
    }

    // Tab completes a name, as in a shell. With nothing to complete it does nothing:
    // moving focus would drop the draft.
    if (e.key === 'Tab' && !e.shiftKey) {
      e.preventDefault();
      if (listOpen) completeWith(suggestions[Math.max(activeSuggestion, 0)]);
      return;
    }

    if (e.key === 'Enter') {
      navigateTo(suggestions[activeSuggestion]?.value ?? editPath);
      // The files the person asked for take the focus. The blur closes the list and shows the real path.
      inputRef.current?.blur();
    } else if (e.key === 'Escape') {
      setEditPath(currentPath);
      // A listing still on its way must not open a list under the restored path.
      closeSuggestions();
    }
  };

  // Keep the input in sync if navigation occurs elsewhere
  useEffect(() => {
    setEditPath(currentPath);
    closeSuggestions();
  }, [currentPath, closeSuggestions]);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (
        (isMacPlatform ? e.metaKey : e.ctrlKey) &&
        !e.altKey &&
        !e.shiftKey &&
        e.key.toLowerCase() === 'l'
      ) {
        e.preventDefault();
        inputRef.current?.focus();
        inputRef.current?.select();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [isMacPlatform]);

  // Typing or pasting right after a click or the shortcut replaces the whole path.
  useEffect(() => {
    if (isFocused) inputRef.current?.select();
  }, [isFocused]);

  useEffect(() => {
    suggestionListRef.current?.children[activeSuggestion]?.scrollIntoView({ block: 'nearest' });
  }, [activeSuggestion]);

  useEffect(() => {
    if (!isLinux) {
      setIsMaximized(false);
      return;
    }

    let cancelled = false;
    let unlisten: (() => void) | undefined;

    const syncMaximizeState = async () => {
      try {
        const maximized = await windowRef.current.isMaximized();
        if (!cancelled) setIsMaximized(maximized);
      } catch (error) {
        console.warn('Failed to read maximize state:', error);
      }
    };

    const setup = async () => {
      await syncMaximizeState();
      try {
        unlisten = await windowRef.current.onResized(async () => {
          await syncMaximizeState();
        });
      } catch (error) {
        console.warn('Failed to subscribe to resize events:', error);
      }
    };

    void setup();

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [isLinux]);

  // Manual dragging fallback function
  const handleManualDrag = async (e: ReactMouseEvent<HTMLDivElement>) => {
    // Only start drag on primary button (left click)
    if (e.button !== 0) return;

    // Check if clicked element is interactive (has data-tauri-drag-region=false)
    const target = e.target as HTMLElement;
    if (target.closest('[data-tauri-drag-region="false"], button, input, select, textarea')) return;

    try {
      await windowRef.current.startDragging();
    } catch (error) {
      console.error('Failed to start window dragging:', error);
    }
  };

  const handleCurrentDirMouseDown = useCallback(
    (event: ReactMouseEvent<HTMLButtonElement>) => {
      event.stopPropagation();

      if (event.button !== 0) return;
      if (!currentDirMeta?.is_directory) return;

      const startX = event.clientX;
      const startY = event.clientY;
      const dragThreshold = 5;
      const isRemote = currentPath.includes('://');
      let dragStarted = false;
      const dragLabel =
        currentDirMeta?.name || currentPath.replace(/\/$/, '').split('/').pop() || currentPath;

      const cleanup = () => {
        window.removeEventListener('mousemove', onMouseMove);
        window.removeEventListener('mouseup', onMouseUp);
        setRemoteDragGhost(null);
        setInAppDropTargetId(null);
      };

      const onMouseUp = (upEvent: MouseEvent) => {
        cleanup();

        if (!dragStarted) {
          inputRef.current?.focus();
          return;
        }

        if (!isRemote) {
          return;
        }

        const el = document.elementFromPoint(upEvent.clientX, upEvent.clientY);
        const isOverSidebar = Boolean(el?.closest('[data-drop-zone-id="sidebar"]'));
        if (isOverSidebar) {
          const { addPinnedDirectory } = useAppStore.getState();
          void addPinnedDirectory(currentPath)
            .then(() => {
              useToastStore.getState().addToast({
                type: 'success',
                message: 'Pinned folder to sidebar',
              });
            })
            .catch((error) => {
              const msg = error instanceof Error ? error.message : String(error);
              if (!msg.toLowerCase().includes('already pinned')) {
                useToastStore.getState().addToast({
                  type: 'error',
                  message: `Failed to pin: ${msg}`,
                });
              }
            });
        }
        endNativeDrag();
      };

      const onMouseMove = (moveEvent: MouseEvent) => {
        const dx = moveEvent.clientX - startX;
        const dy = moveEvent.clientY - startY;
        const distance = Math.sqrt(dx * dx + dy * dy);
        if (!dragStarted) {
          if (distance < dragThreshold) return;
          dragStarted = true;

          startNativeDrag({ path: currentPath, name: currentDirMeta.name });

          if (isRemote) {
            setRemoteDragGhost({
              x: moveEvent.clientX,
              y: moveEvent.clientY,
              label: dragLabel,
            });
            const el = document.elementFromPoint(moveEvent.clientX, moveEvent.clientY);
            const targetId = el?.closest<HTMLElement>('[data-drop-zone-id]')?.dataset.dropZoneId;
            setInAppDropTargetId(targetId ?? null);
            return;
          }

          cleanup();
          setNativeDragPaths([currentPath]);
          void invoke('start_native_drag', {
            paths: [currentPath],
            previewImage: null,
            dragOffsetY: 0,
          })
            .catch((error) => {
              console.warn('Native drag failed for current directory:', error);
            })
            .finally(() => {
              window.setTimeout(clearNativeDragPaths, 500);
              endNativeDrag();
            });
          return;
        }

        if (isRemote) {
          setRemoteDragGhost((prev) =>
            prev ? { ...prev, x: moveEvent.clientX, y: moveEvent.clientY } : prev
          );
          const el = document.elementFromPoint(moveEvent.clientX, moveEvent.clientY);
          const targetId = el?.closest<HTMLElement>('[data-drop-zone-id]')?.dataset.dropZoneId;
          setInAppDropTargetId(targetId ?? null);
        }
      };

      window.addEventListener('mousemove', onMouseMove);
      window.addEventListener('mouseup', onMouseUp);
    },
    [
      clearNativeDragPaths,
      currentDirMeta,
      currentPath,
      endNativeDrag,
      setInAppDropTargetId,
      setNativeDragPaths,
      startNativeDrag,
    ]
  );

  return (
    <div
      className="toolbar gap-3 select-none relative"
      data-tauri-drag-region
      onMouseDown={handleManualDrag}
    >
      {/* Back/Forward */}
      <div className="flex items-center">
        {(() => {
          const isMac = isMacPlatform;
          const backTitle = isMac ? 'Back (⌘[)' : 'Back (Alt+←)';
          const fwdTitle = isMac ? 'Forward (⌘])' : 'Forward (Alt+→)';
          const upTitle = isMac ? 'Up (⌘↑)' : 'Up (Alt+↑)';
          const refreshTitle = isMac ? 'Refresh (⌘R)' : 'Refresh (F5/Ctrl+R)';
          return (
            <>
              <button
                onClick={() => useAppStore.getState().goBack()}
                disabled={!useAppStore.getState().canGoBack()}
                className="btn-icon rounded-full"
                title={backTitle}
                data-tauri-drag-region={false}
              >
                <CaretLeft className="w-4 h-4" />
              </button>
              <button
                onClick={() => useAppStore.getState().goForward()}
                disabled={!useAppStore.getState().canGoForward()}
                className="btn-icon rounded-full"
                title={fwdTitle}
                data-tauri-drag-region={false}
              >
                <CaretRight className="w-4 h-4" />
              </button>
              <button
                onClick={() => useAppStore.getState().goUp()}
                disabled={!useAppStore.getState().canGoUp()}
                className={`btn-icon rounded-full transition-all ${
                  isUpButtonDropTarget ? 'ring-2 ring-accent bg-accent-soft scale-110' : ''
                }`}
                title={upTitle}
                data-tauri-drag-region={false}
                data-folder-path={parentPath ?? undefined}
              >
                <ArrowUp className="w-4 h-4" />
              </button>
              <button
                onClick={() => useAppStore.getState().refreshCurrentDirectory()}
                className="btn-icon rounded-full"
                title={refreshTitle}
                data-tauri-drag-region={false}
              >
                <ArrowClockwise className="w-4 h-4" />
              </button>
            </>
          );
        })()}
      </div>

      {/* Path input */}
      <div className="flex-1 relative">
        <button
          type="button"
          aria-label="Drag to pin folder to sidebar"
          className="absolute inset-y-0 left-0 flex items-center pl-2.5 z-10 cursor-grab active:cursor-grabbing text-accent hover:text-accent/80 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
          data-tauri-drag-region={false}
          onMouseDown={handleCurrentDirMouseDown}
        >
          <span className="relative w-5 h-5">
            <Folder className="w-5 h-5" weight="fill" />
            {currentDirMeta?.is_git_repo && (
              <GitRepoBadge size="sm" style={{ bottom: -2, left: -2 }} />
            )}
            {currentDirMeta?.is_symlink && (
              <SymlinkBadge size="sm" style={{ bottom: -2, right: -2 }} />
            )}
          </span>
        </button>
        <input
          ref={inputRef}
          type="text"
          value={isFocused ? editPath : displayPath}
          onChange={handleInputChange}
          onKeyDown={handleKeyDown}
          onFocus={handleFocus}
          onBlur={handleBlur}
          onMouseDown={(e) => {
            // The first click selects the whole path (see the focus effect). A later click places the caret.
            if (document.activeElement !== e.currentTarget) {
              e.preventDefault();
              e.currentTarget.focus();
            }
          }}
          className="w-full input-field pl-10"
          placeholder="Enter path..."
          data-testid="path-input"
          data-tauri-drag-region={false}
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
          spellCheck={false}
        />
        {suggestions.length > 0 && (
          <div
            ref={suggestionListRef}
            role="listbox"
            // A mouse press in the list, its scrollbar included, must not take focus from the input.
            onMouseDown={(event) => event.preventDefault()}
            // Enter opens the highlighted row, so a row the pointer only passed over must not stay highlighted.
            onMouseLeave={() => setActiveSuggestion(-1)}
            className="absolute left-0 right-0 top-full mt-1 max-h-64 overflow-y-auto rounded-md border border-app-border bg-app-gray shadow-lg z-30"
          >
            {suggestions.map((suggestion, index) => (
              <button
                key={suggestion.value}
                type="button"
                role="option"
                aria-selected={index === activeSuggestion}
                className={`w-full text-left pl-10 pr-3 py-1 text-sm ${
                  index === activeSuggestion ? 'bg-app-light' : ''
                }`}
                onMouseMove={() => setActiveSuggestion(index)}
                onMouseDown={() => completeWith(suggestion)}
              >
                {suggestion.display}
              </button>
            ))}
          </div>
        )}
      </div>

      {/* View toggles + update notice */}
      <div className="flex items-center gap-2">
        <div className="flex items-center gap-1">
          <button
            className={`btn-icon ${
              (useAppStore.getState().directoryPreferences[currentPath]?.viewMode ||
                useAppStore.getState().globalPreferences.viewMode) === 'grid'
                ? 'bg-accent-soft text-accent'
                : ''
            }`}
            onClick={() =>
              useAppStore.getState().updateDirectoryPreferences(currentPath, { viewMode: 'grid' })
            }
            title="Icons"
            data-tauri-drag-region={false}
            onMouseEnter={() => showZoomSliderNow()}
            onMouseLeave={() => scheduleHideZoomSlider(400)}
            onFocus={() => showZoomSliderNow()}
            onBlur={() => scheduleHideZoomSlider(400)}
          >
            <SquaresFour className="w-4 h-4 text-accent" />
          </button>
          <button
            className={`btn-icon ${
              (useAppStore.getState().directoryPreferences[currentPath]?.viewMode ||
                useAppStore.getState().globalPreferences.viewMode) === 'list'
                ? 'bg-accent-soft text-accent'
                : ''
            }`}
            onClick={() =>
              useAppStore.getState().updateDirectoryPreferences(currentPath, { viewMode: 'list' })
            }
            title="List"
            data-tauri-drag-region={false}
          >
            <List className="w-4 h-4 text-accent" />
          </button>
        </div>

        <UpdateNotice />
      </div>

      {isLinux && (
        <div className="flex items-center gap-2 ml-3 -mt-1.5" data-tauri-drag-region={false}>
          <button
            className="flex items-center justify-center w-[26px] h-[26px] rounded-full bg-app-light/40 hover:bg-app-light text-app-muted transition-colors"
            onClick={() => {
              void windowRef.current
                .minimize()
                .catch((error) => console.error('Failed to minimize window:', error));
            }}
            aria-label="Minimize window"
          >
            <Minus className="w-3 h-3 translate-y-[2px]" />
          </button>
          <button
            className="flex items-center justify-center w-[26px] h-[26px] rounded-full bg-app-light/40 hover:bg-app-light text-app-muted transition-colors"
            onClick={() => {
              void (async () => {
                try {
                  await windowRef.current.toggleMaximize();
                  setIsMaximized(await windowRef.current.isMaximized());
                } catch (error) {
                  console.error('Failed to toggle maximize state:', error);
                }
              })();
            }}
            aria-label={isMaximized ? 'Restore window' : 'Maximize window'}
          >
            {isMaximized ? (
              <CopySimple className="w-3.5 h-3.5" style={{ transform: 'scale(0.9)' }} />
            ) : (
              <Square className="w-3.5 h-3.5" style={{ transform: 'scale(0.85)' }} />
            )}
          </button>
          <button
            className="flex items-center justify-center w-[26px] h-[26px] rounded-full bg-app-light/40 hover:bg-app-red/70 hover:text-white text-app-muted transition-colors"
            onClick={() => {
              void windowRef.current
                .close()
                .catch((error) => console.error('Failed to close window:', error));
            }}
            aria-label="Close window"
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>
      )}

      {remoteDragGhost && (
        <div
          className="fixed z-50 pointer-events-none"
          style={{
            left: remoteDragGhost.x,
            top: remoteDragGhost.y,
            transform: 'translate(12px, 12px)',
          }}
          data-tauri-drag-region={false}
        >
          <div className="flex items-center gap-2 rounded-md border border-app-border bg-app-gray/95 px-2 py-1.5 shadow-lg backdrop-blur-sm">
            <Folder className="w-4 h-4 text-accent" weight="fill" />
            <span className="max-w-[240px] truncate text-xs text-app-muted">
              {remoteDragGhost.label}
            </span>
          </div>
        </div>
      )}

      {/* Sticky Zoom slider at top right */}
      {(() => {
        const viewMode =
          useAppStore.getState().directoryPreferences[currentPath]?.viewMode ||
          useAppStore.getState().globalPreferences.viewMode;
        const isGrid = viewMode === 'grid';
        return <ZoomSlider visible={isGrid && showZoomSlider} />;
      })()}
    </div>
  );
}
