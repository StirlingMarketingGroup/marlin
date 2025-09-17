import { useEffect, useRef, useState, KeyboardEvent, MouseEvent } from 'react';
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
import { useAppStore } from '../store/useAppStore';
import { getCurrentWindow } from '@tauri-apps/api/window';
import ZoomSlider from './ZoomSlider';
import UpdateNotice from '@/components/UpdateNotice';

export default function PathBar() {
  const { currentPath, navigateTo, showZoomSliderNow, scheduleHideZoomSlider, showZoomSlider } =
    useAppStore();

  const [editPath, setEditPath] = useState(currentPath);
  const [isMaximized, setIsMaximized] = useState(false);
  const userAgent = typeof navigator !== 'undefined' ? navigator.userAgent.toUpperCase() : '';
  const platform = typeof navigator !== 'undefined' ? navigator.platform.toUpperCase() : '';
  const isLinux = userAgent.includes('LINUX');
  const isMacPlatform = platform.includes('MAC');

  const handleKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') {
      navigateTo(editPath);
    } else if (e.key === 'Escape') {
      setEditPath(currentPath);
    }
  };

  // Keep the input in sync if navigation occurs elsewhere
  useEffect(() => {
    setEditPath(currentPath);
  }, [currentPath]);

  const windowRef = useRef(getCurrentWindow());

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
  const handleManualDrag = async (e: MouseEvent<HTMLDivElement>) => {
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
      <div className="flex-1 flex items-center gap-2">
        <input
          type="text"
          value={editPath}
          onChange={(e) => setEditPath(e.target.value)}
          onKeyDown={handleKeyDown}
          className="flex-1 input-field"
          placeholder="Enter path..."
          data-tauri-drag-region={false}
        />
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
