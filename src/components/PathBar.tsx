import {
  useCallback,
  useEffect,
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
  ArrowUp,
  ArrowClockwise,
  Minus,
  Square,
  CopySimple,
  X,
} from 'phosphor-react';
import { invoke } from '@tauri-apps/api/core';
import type { DirectoryPreferencesMap, DirectoryListingResponse, FileItem } from '@/types';
import { useAppStore } from '@/store/useAppStore';
import { getCurrentWindow } from '@tauri-apps/api/window';
import ZoomSlider from './ZoomSlider';
import UpdateNotice from '@/components/UpdateNotice';

const MAX_SUGGESTIONS = 8;

interface PathSuggestion {
  value: string;
  name: string;
  display: string;
}

interface AutocompleteInfo {
  basePrefix: string;
  partial: string;
  separator: string;
  showHidden: boolean;
  triggerIndex: number;
}

const normalizeForCompare = (value: string): string => {
  if (!value) return '/';
  const replaced = value.replace(/\\/g, '/');
  const trimmed = replaced.length > 1 && replaced.endsWith('/') ? replaced.slice(0, -1) : replaced;
  return trimmed || '/';
};

interface DeriveAutocompleteParams {
  value: string;
  cursor: number;
  previousInfo: AutocompleteInfo | null;
  directoryPreferences: DirectoryPreferencesMap;
  globalShowHidden: boolean;
}

const deriveAutocompleteInfo = ({
  value,
  cursor,
  previousInfo,
  directoryPreferences,
  globalShowHidden,
}: DeriveAutocompleteParams): AutocompleteInfo | null => {
  const uptoCursor = value.slice(0, cursor);
  const lastSlash = Math.max(uptoCursor.lastIndexOf('/'), uptoCursor.lastIndexOf('\\'));

  if (lastSlash < 0) return null;

  const separator = uptoCursor[lastSlash];
  const basePrefix = value.slice(0, lastSlash + 1);
  const partial = uptoCursor.slice(lastSlash + 1);
  const hasTrailingSeparator = uptoCursor.endsWith('/') || uptoCursor.endsWith('\\');
  const requestingHidden = partial.startsWith('.');
  const triggeredIndex = hasTrailingSeparator
    ? lastSlash
    : previousInfo && previousInfo.triggerIndex === lastSlash
      ? previousInfo.triggerIndex
      : undefined;

  const shouldSuggest =
    hasTrailingSeparator || typeof triggeredIndex === 'number' || requestingHidden;
  if (!shouldSuggest) return null;

  const normalizedDir = normalizeForCompare(basePrefix);
  const showHiddenPref = directoryPreferences[normalizedDir]?.showHidden ?? globalShowHidden;

  return {
    basePrefix,
    partial,
    separator,
    showHidden: showHiddenPref,
    triggerIndex: hasTrailingSeparator
      ? lastSlash
      : typeof triggeredIndex === 'number'
        ? triggeredIndex
        : lastSlash,
  };
};

interface FetchSuggestionsParams {
  info: AutocompleteInfo;
  currentPath: string;
  files: FileItem[];
}

const fetchSuggestionsForInfo = async ({
  info,
  currentPath,
  files,
}: FetchSuggestionsParams): Promise<PathSuggestion[]> => {
  const { basePrefix: prefix, partial: partialSegment, separator: sep, showHidden } = info;

  let entries: FileItem[] = [];
  if (normalizeForCompare(prefix) === normalizeForCompare(currentPath)) {
    entries = files;
  } else {
    const fetchPath = prefix.length === 0 ? sep : prefix;
    const response = await invoke<DirectoryListingResponse>('read_directory', {
      path: fetchPath || '/',
    });
    entries = response.entries;
  }

  const partialLower = partialSegment.toLocaleLowerCase();
  const allowHidden = showHidden || partialSegment.startsWith('.');
  const filtered = entries.filter((entry) => {
    if (!entry.is_directory) return false;
    if (!allowHidden && entry.is_hidden) return false;
    if (!partialSegment) return true;
    return entry.name.toLocaleLowerCase().startsWith(partialLower);
  });

  return filtered
    .sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }))
    .slice(0, MAX_SUGGESTIONS)
    .map<PathSuggestion>((entry) => ({
      value: `${prefix}${entry.name}${sep}`,
      name: entry.name,
      display: `${entry.name}${sep}`,
    }));
};

export default function PathBar() {
  const {
    currentPath,
    navigateTo,
    showZoomSliderNow,
    scheduleHideZoomSlider,
    showZoomSlider,
    files,
    globalPreferences,
    directoryPreferences,
  } = useAppStore();

  const [editPath, setEditPath] = useState(currentPath);
  const [suggestions, setSuggestions] = useState<PathSuggestion[]>([]);
  const [showSuggestions, setShowSuggestions] = useState(false);
  const [activeSuggestion, setActiveSuggestion] = useState(-1);
  const [isFocused, setIsFocused] = useState(false);
  const [isMaximized, setIsMaximized] = useState(false);
  const userAgent = typeof navigator !== 'undefined' ? navigator.userAgent.toUpperCase() : '';
  const platform = typeof navigator !== 'undefined' ? navigator.platform.toUpperCase() : '';
  const isLinux = userAgent.includes('LINUX');
  const isMacPlatform = platform.includes('MAC');

  const inputRef = useRef<HTMLInputElement>(null);
  const windowRef = useRef(getCurrentWindow());
  const autocompleteInfoRef = useRef<AutocompleteInfo | null>(null);
  const originalValueRef = useRef<string>('');
  const skipNextFetchRef = useRef(false);
  const suggestionRequestIdRef = useRef(0);

  const clearSuggestions = useCallback(() => {
    setSuggestions([]);
    setShowSuggestions(false);
    setActiveSuggestion(-1);
  }, []);

  const scheduleSelection = (start: number, end: number) => {
    requestAnimationFrame(() => {
      const input = inputRef.current;
      if (!input) return;
      input.setSelectionRange(start, end);
    });
  };

  const previewSuggestion = (index: number) => {
    const info = autocompleteInfoRef.current;
    const suggestion = suggestions[index];
    if (!info || !suggestion) return;
    skipNextFetchRef.current = true;
    setActiveSuggestion(index);
    setEditPath(suggestion.value);
    const start = Math.min(info.basePrefix.length + info.partial.length, suggestion.value.length);
    scheduleSelection(start, suggestion.value.length);
  };

  const restoreOriginal = () => {
    const info = autocompleteInfoRef.current;
    if (!info) return;
    skipNextFetchRef.current = true;
    const original = originalValueRef.current || info.basePrefix + info.partial;
    setEditPath(original);
    setActiveSuggestion(-1);
    const caret = info.basePrefix.length + info.partial.length;
    scheduleSelection(caret, caret);
  };

  const handleFocus = () => {
    setIsFocused(true);
    skipNextFetchRef.current = false;
    originalValueRef.current = editPath;
  };

  const handleBlur = () => {
    setIsFocused(false);
    clearSuggestions();
  };

  const commitSuggestion = (index?: number) => {
    const targetIndex = typeof index === 'number' ? index : activeSuggestion;
    const suggestion = suggestions[targetIndex];
    if (!suggestion) return false;
    clearSuggestions();
    skipNextFetchRef.current = true;
    autocompleteInfoRef.current = null;
    setEditPath(suggestion.value);
    scheduleSelection(suggestion.value.length, suggestion.value.length);
    originalValueRef.current = suggestion.value;
    return true;
  };

  const handleInputChange = (e: ChangeEvent<HTMLInputElement>) => {
    skipNextFetchRef.current = false;
    setActiveSuggestion(-1);
    setEditPath(e.target.value);
  };

  const handleKeyDown = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    const hasSuggestions = showSuggestions && suggestions.length > 0;

    if (!hasSuggestions && e.key === 'ArrowDown') {
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

    if (hasSuggestions) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        const next = activeSuggestion + 1;
        const index = next >= suggestions.length ? 0 : next;
        previewSuggestion(index);
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        if (activeSuggestion <= 0) {
          restoreOriginal();
        } else {
          previewSuggestion(activeSuggestion - 1);
        }
        return;
      }
      if (e.key === 'Tab' && !e.shiftKey) {
        const index = activeSuggestion === -1 ? 0 : activeSuggestion;
        if (suggestions[index]) {
          e.preventDefault();
          commitSuggestion(index);
        }
        return;
      }
    }

    if (e.key === 'Enter') {
      if (hasSuggestions && activeSuggestion >= 0) {
        e.preventDefault();
        const suggestion = suggestions[activeSuggestion];
        if (commitSuggestion(activeSuggestion)) {
          navigateTo(suggestion.value);
        }
        return;
      }
      clearSuggestions();
      navigateTo(editPath);
    } else if (e.key === 'Escape') {
      setEditPath(currentPath);
      clearSuggestions();
      originalValueRef.current = currentPath;
      skipNextFetchRef.current = false;
      scheduleSelection(currentPath.length, currentPath.length);
    }
  };

  // Keep the input in sync if navigation occurs elsewhere
  useEffect(() => {
    setEditPath(currentPath);
    clearSuggestions();
    originalValueRef.current = currentPath;
  }, [currentPath, clearSuggestions]);

  useEffect(() => {
    if (!isFocused) {
      clearSuggestions();
      return;
    }

    if (skipNextFetchRef.current) {
      skipNextFetchRef.current = false;
      return;
    }

    const input = inputRef.current;
    const cursor = input ? (input.selectionStart ?? editPath.length) : editPath.length;

    const nextInfo = deriveAutocompleteInfo({
      value: editPath,
      cursor,
      previousInfo: autocompleteInfoRef.current,
      directoryPreferences,
      globalShowHidden: globalPreferences.showHidden,
    });

    if (!nextInfo) {
      autocompleteInfoRef.current = null;
      clearSuggestions();
      return;
    }

    autocompleteInfoRef.current = nextInfo;
    originalValueRef.current = editPath;

    let cancelled = false;
    const requestId = ++suggestionRequestIdRef.current;

    const run = async () => {
      const activeInfo = autocompleteInfoRef.current;
      if (!activeInfo) return;
      try {
        const limited = await fetchSuggestionsForInfo({
          info: activeInfo,
          currentPath,
          files,
        });
        if (cancelled || suggestionRequestIdRef.current !== requestId) return;
        setSuggestions(limited);
        setShowSuggestions(limited.length > 0);
        setActiveSuggestion(-1);
      } catch (error) {
        if (cancelled || suggestionRequestIdRef.current !== requestId) return;
        console.warn('Failed to fetch path suggestions:', error);
        clearSuggestions();
      }
    };

    const timeoutId = window.setTimeout(() => {
      void run();
    }, 120);

    return () => {
      cancelled = true;
      window.clearTimeout(timeoutId);
    };
  }, [
    editPath,
    files,
    currentPath,
    directoryPreferences,
    globalPreferences.showHidden,
    isFocused,
    clearSuggestions,
  ]);

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
                className="btn-icon rounded-full"
                title={upTitle}
                data-tauri-drag-region={false}
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
      <div className="flex-1 flex items-center gap-2 relative">
        <input
          ref={inputRef}
          type="text"
          value={editPath}
          onChange={handleInputChange}
          onKeyDown={handleKeyDown}
          onFocus={handleFocus}
          onBlur={handleBlur}
          className="flex-1 input-field"
          placeholder="Enter path..."
          data-tauri-drag-region={false}
          autoComplete="off"
        />
        {showSuggestions && suggestions.length > 0 && (
          <div className="absolute left-0 right-0 top-full mt-1 rounded-md border border-app-border bg-app-gray shadow-lg z-30 overflow-hidden">
            {suggestions.map((suggestion, index) => {
              const partial = autocompleteInfoRef.current?.partial ?? '';
              const highlightLength = Math.min(partial.length, suggestion.display.length);
              const typedPortion = suggestion.display.slice(0, highlightLength);
              const remainder = suggestion.display.slice(highlightLength);
              const isActive = index === activeSuggestion;

              return (
                <button
                  key={suggestion.value}
                  type="button"
                  className={`w-full text-left px-3 py-1.5 text-sm flex items-center gap-2 transition-colors ${
                    isActive ? 'bg-app-light text-accent' : 'hover:bg-app-light'
                  }`}
                  onMouseEnter={() => previewSuggestion(index)}
                  onMouseDown={(event) => {
                    event.preventDefault();
                    commitSuggestion(index);
                  }}
                >
                  <span className="font-mono tracking-tight">
                    <span>{typedPortion}</span>
                    <span className="text-app-muted">{remainder}</span>
                  </span>
                </button>
              );
            })}
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
