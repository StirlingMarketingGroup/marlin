import { useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { useAppStore } from '@/store/useAppStore'

type SortBy = 'name' | 'size' | 'type' | 'modified'
type SortOrder = 'asc' | 'desc'

interface ContextMenuProps {
  x: number
  y: number
  isFileContext: boolean
  onRequestClose: () => void
}

export default function ContextMenu({ x, y, isFileContext, onRequestClose }: ContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null)
  const [pos, setPos] = useState<{ x: number; y: number }>({ x, y })

  const state = useAppStore()
  const {
    selectedFiles,
    globalPreferences,
    currentPath,
    directoryPreferences,
    updateDirectoryPreferences,
    toggleHiddenFiles,
    toggleFoldersFirst,
    beginRenameSelected,
  } = state

  const prefs = useMemo(() => ({ ...globalPreferences, ...directoryPreferences[currentPath] }), [globalPreferences, directoryPreferences, currentPath])

  // Clamp into viewport once mounted
  useEffect(() => {
    const vw = window.innerWidth
    const vh = window.innerHeight
    const maxX = vw - 260
    const maxY = vh - 280
    setPos({ x: Math.max(8, Math.min(x, maxX)), y: Math.max(8, Math.min(y, maxY)) })
  }, [x, y])

  useEffect(() => {
    const onDocClick = (e: MouseEvent) => {
      const target = e.target as Node | null
      if (menuRef.current && target && menuRef.current.contains(target)) return
      onRequestClose()
    }
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onRequestClose() }
    document.addEventListener('mousedown', onDocClick)
    document.addEventListener('contextmenu', onDocClick)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDocClick)
      document.removeEventListener('contextmenu', onDocClick)
      document.removeEventListener('keydown', onKey)
    }
  }, [onRequestClose])

  const copyToClipboard = async (text: string) => {
    try {
      await navigator.clipboard?.writeText(text)
    } catch (_) {
      // ignore
    }
  }

  const onSortBy = (field: SortBy) => {
    updateDirectoryPreferences(currentPath, { sortBy: field })
    onRequestClose()
  }
  const onSortOrder = (order: SortOrder) => {
    updateDirectoryPreferences(currentPath, { sortOrder: order })
    onRequestClose()
  }

  const fileSpecific = isFileContext && selectedFiles.length > 0

  const menu = (
    <div
      ref={menuRef}
      className="fixed z-50 min-w-[220px] rounded-md border bg-[rgb(var(--bg-rgb))] border-app-border shadow-lg"
      style={{ left: pos.x, top: pos.y }}
      role="menu"
      aria-label="Context menu"
    >
      <div className="py-1 text-sm">
        {fileSpecific && (
          <>
            <button
              className="w-full text-left px-3 py-2 hover:bg-app-light"
              onClick={() => { beginRenameSelected(); onRequestClose() }}
            >
              Rename
            </button>
            <button
              className="w-full text-left px-3 py-2 hover:bg-app-light"
              onClick={() => {
                const first = selectedFiles[0]
                const name = first?.split(/[\\/]/).pop() || ''
                void copyToClipboard(name)
                onRequestClose()
              }}
            >
              Copy File Name
            </button>
            <button
              className="w-full text-left px-3 py-2 hover:bg-app-light"
              onClick={() => { void copyToClipboard(selectedFiles.join('\n')); onRequestClose() }}
            >
              Copy Full Path
            </button>
            <div className="my-1 h-px bg-app-border" />
          </>
        )}

        <button
          className="w-full text-left px-3 py-2 hover:bg-app-light"
          onClick={() => { void toggleHiddenFiles(); onRequestClose() }}
        >
          {prefs.showHidden ? 'Hide Hidden Files' : 'Show Hidden Files'}
        </button>

        <div className="my-1 h-px bg-app-border" />

        <div className="px-3 py-1 text-app-muted uppercase text-[11px]">Sort By</div>
        <button className={`w-full text-left px-3 py-2 hover:bg-app-light ${prefs.sortBy === 'name' ? 'text-accent' : ''}`} onClick={() => onSortBy('name')}>Name</button>
        <button className={`w-full text-left px-3 py-2 hover:bg-app-light ${prefs.sortBy === 'size' ? 'text-accent' : ''}`} onClick={() => onSortBy('size')}>Size</button>
        <button className={`w-full text-left px-3 py-2 hover:bg-app-light ${prefs.sortBy === 'type' ? 'text-accent' : ''}`} onClick={() => onSortBy('type')}>Type</button>
        <button className={`w-full text-left px-3 py-2 hover:bg-app-light ${prefs.sortBy === 'modified' ? 'text-accent' : ''}`} onClick={() => onSortBy('modified')}>Date Modified</button>

        <div className="my-1 h-px bg-app-border" />

        <div className="px-3 py-1 text-app-muted uppercase text-[11px]">Sort Order</div>
        <button className={`w-full text-left px-3 py-2 hover:bg-app-light ${prefs.sortOrder === 'asc' ? 'text-accent' : ''}`} onClick={() => onSortOrder('asc')}>Ascending</button>
        <button className={`w-full text-left px-3 py-2 hover:bg-app-light ${prefs.sortOrder === 'desc' ? 'text-accent' : ''}`} onClick={() => onSortOrder('desc')}>Descending</button>

        <div className="my-1 h-px bg-app-border" />
        <button
          className={`w-full text-left px-3 py-2 hover:bg-app-light ${prefs.foldersFirst ? 'text-accent' : ''}`}
          onClick={() => { void toggleFoldersFirst(); onRequestClose() }}
        >
          Folders on Top
        </button>
      </div>
    </div>
  )

  return createPortal(menu, document.body)
}

