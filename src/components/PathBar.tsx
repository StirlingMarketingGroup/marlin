import { useEffect, useState, KeyboardEvent, MouseEvent } from 'react'
import { CaretLeft, CaretRight, SquaresFour, List, ArrowUp, ArrowClockwise } from 'phosphor-react'
import { useAppStore } from '../store/useAppStore'

export default function PathBar() {
  const {
    currentPath,
    homeDir,
    navigateTo,
  } = useAppStore()

  const [editPath, setEditPath] = useState(currentPath)

  const handleKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') {
      navigateTo(editPath)
    } else if (e.key === 'Escape') {
      setEditPath(currentPath)
    }
  }

  // Keep the input in sync if navigation occurs elsewhere
  useEffect(() => {
    setEditPath(currentPath)
  }, [currentPath])

  return (
    <div className="toolbar gap-3 select-none" data-tauri-drag-region>
      {/* Back/Forward */}
      <div className="flex items-center">
        {(() => { const isMac = navigator.platform.toUpperCase().includes('MAC');
          const backTitle = isMac ? 'Back (⌘[)' : 'Back (Alt+←)'
          const fwdTitle = isMac ? 'Forward (⌘])' : 'Forward (Alt+→)'
          const upTitle = isMac ? 'Up (⌘↑)' : 'Up (Alt+↑)'
          const refreshTitle = isMac ? 'Refresh (⌘R)' : 'Refresh (F5/Ctrl+R)'
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
          )})()}
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

      {/* View toggles */}
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
    </div>
  )
}
