import { useEffect, useRef, useState } from 'react'
import { Check } from 'phosphor-react'
import { useAppStore } from '../store/useAppStore'

interface ContextMenuProps {
  x: number
  y: number
  onClose: () => void
}

export default function ContextMenu({ x, y, onClose }: ContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null)
  const { globalPreferences, toggleHiddenFiles: centralizedToggle, toggleFoldersFirst } = useAppStore()
  const { updateDirectoryPreferences } = useAppStore()
  const sortTriggerRef = useRef<HTMLButtonElement>(null)
  const [submenuOpen, setSubmenuOpen] = useState<boolean>(false)
  const [submenuTop, setSubmenuTop] = useState<number>(0)
  const [submenuLeftSide, setSubmenuLeftSide] = useState<boolean>(false)
  const submenuRef = useRef<HTMLDivElement>(null)

  // Position the menu and handle viewport boundaries
  useEffect(() => {
    if (!menuRef.current) return

    const menu = menuRef.current
    const rect = menu.getBoundingClientRect()
    const viewportWidth = window.innerWidth
    const viewportHeight = window.innerHeight

    // Adjust position if menu would go off-screen
    let adjustedX = x
    let adjustedY = y

    if (x + rect.width > viewportWidth) {
      adjustedX = x - rect.width
    }
    if (y + rect.height > viewportHeight) {
      adjustedY = y - rect.height
    }

    menu.style.left = `${Math.max(0, adjustedX)}px`
    menu.style.top = `${Math.max(0, adjustedY)}px`
  }, [x, y])

  // Close menu on outside click or Escape key
  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
        onClose()
      }
    }

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        onClose()
      }
    }

    document.addEventListener('mousedown', handleClickOutside)
    document.addEventListener('keydown', handleKeyDown)

    return () => {
      document.removeEventListener('mousedown', handleClickOutside)
      document.removeEventListener('keydown', handleKeyDown)
    }
  }, [onClose])

  const toggleHiddenFiles = () => {
    centralizedToggle()
    onClose()
  }

  const toggleFoldersOnTop = () => {
    toggleFoldersFirst()
    onClose()
  }

  const setSortBy = (sortBy: 'name' | 'size' | 'type' | 'modified') => {
    updateDirectoryPreferences(useAppStore.getState().currentPath, { sortBy })
    onClose()
  }

  const openSortSubmenu = () => {
    if (!menuRef.current || !sortTriggerRef.current) {
      setSubmenuOpen(true)
      return
    }
    const menuRect = menuRef.current.getBoundingClientRect()
    const btnRect = sortTriggerRef.current.getBoundingClientRect()
    const top = btnRect.top - menuRect.top
    setSubmenuTop(top)

    // Decide left/right placement with a simple estimate
    const estimatedWidth = 220
    const openLeft = menuRect.right + estimatedWidth > window.innerWidth
    setSubmenuLeftSide(openLeft)
    setSubmenuOpen(true)

    // After it renders, adjust to keep within viewport vertically
    requestAnimationFrame(() => {
      if (!menuRef.current || !submenuRef.current) return
      const menuR = menuRef.current.getBoundingClientRect()
      const subR = submenuRef.current.getBoundingClientRect()
      let desiredTop = btnRect.top - menuR.top
      const maxTop = Math.max(0, window.innerHeight - menuR.top - subR.height - 4)
      if (menuR.top + desiredTop + subR.height > window.innerHeight) {
        desiredTop = maxTop
      }
      setSubmenuTop(desiredTop)
    })
  }

  const closeSubmenu = () => setSubmenuOpen(false)

  return (
    <div
      ref={menuRef}
      className="fixed z-50 bg-app-gray border border-app-border rounded-md shadow-lg py-1 min-w-48"
      style={{ left: x, top: y }}
      data-tauri-drag-region={false}
    >
      <div className="px-1">
        <button
          className="w-full flex items-center gap-3 px-3 py-2 text-sm text-app-text hover:bg-app-light rounded transition-colors"
          onClick={toggleHiddenFiles}
          data-tauri-drag-region={false}
        >
          <div className="flex items-center justify-center w-4 h-4">
            {globalPreferences.showHidden ? (
              <Check className="w-4 h-4 text-app-accent" weight="bold" />
            ) : null}
          </div>
          <div className="flex items-center gap-2">
            <span>Show Hidden Files</span>
          </div>
        </button>
        <div className="h-px bg-app-border my-1" />
        <div className="relative">
          <button
            ref={sortTriggerRef}
            className="w-full flex items-center justify-between px-3 py-2 text-sm text-app-text hover:bg-app-light rounded transition-colors"
            onMouseEnter={openSortSubmenu}
            onClick={openSortSubmenu}
            data-tauri-drag-region={false}
          >
            <span>Sort by</span>
            <span className="text-app-muted">▸</span>
          </button>

          {submenuOpen && (
            <div
              className="absolute z-50 bg-app-gray border border-app-border rounded-md shadow-lg py-1 min-w-44"
              style={{
                top: submenuTop,
                left: submenuLeftSide ? 'auto' as any : 'calc(100% + 4px)',
                right: submenuLeftSide ? 'calc(100% + 4px)' : 'auto' as any,
              }}
              ref={submenuRef}
              onMouseLeave={closeSubmenu}
            >
              {(['name','size','type','modified'] as const).map((key) => {
                const dirPrefs = useAppStore.getState().directoryPreferences[useAppStore.getState().currentPath]
                const active = (dirPrefs?.sortBy ?? globalPreferences.sortBy) === key
                const label = key === 'name' ? 'Name' : key === 'size' ? 'Size' : key === 'type' ? 'Type' : 'Date Modified'
                return (
                  <button
                    key={key}
                    className="w-full flex items-center gap-3 px-3 py-2 text-sm text-app-text hover:bg-app-light rounded transition-colors"
                    onClick={() => {
                      const defaultOrder: 'asc' | 'desc' = (key === 'size' || key === 'modified') ? 'desc' : 'asc'
                      updateDirectoryPreferences(useAppStore.getState().currentPath, { sortBy: key, sortOrder: defaultOrder })
                      onClose()
                    }}
                    data-tauri-drag-region={false}
                  >
                    <div className="flex items-center justify-center w-4 h-4">
                      {active ? (
                        <Check className="w-4 h-4 text-app-accent" weight="bold" />
                      ) : null}
                    </div>
                    <div className="flex items-center gap-2"><span>{label}</span></div>
                  </button>
                )
              })}
              <div className="h-px bg-app-border my-1" />
              {(['asc','desc'] as const).map((dir) => {
                const dirPrefs = useAppStore.getState().directoryPreferences[useAppStore.getState().currentPath]
                const active = (dirPrefs?.sortOrder ?? globalPreferences.sortOrder) === dir
                const label = dir === 'asc' ? 'Ascending' : 'Descending'
                return (
                  <button
                    key={dir}
                    className="w-full flex items-center gap-3 px-3 py-2 text-sm text-app-text hover:bg-app-light rounded transition-colors"
                    onClick={() => {
                      updateDirectoryPreferences(useAppStore.getState().currentPath, { sortOrder: dir })
                      onClose()
                    }}
                    data-tauri-drag-region={false}
                  >
                    <div className="flex items-center justify-center w-4 h-4">
                      {active ? (
                        <Check className="w-4 h-4 text-app-accent" weight="bold" />
                      ) : null}
                    </div>
                    <div className="flex items-center gap-2"><span>{label}</span></div>
                  </button>
                )
              })}
              <div className="h-px bg-app-border my-1" />
              <button
                className="w-full flex items-center gap-3 px-3 py-2 text-sm text-app-text hover:bg-app-light rounded transition-colors"
                onClick={() => { toggleFoldersOnTop() }}
                data-tauri-drag-region={false}
              >
                <div className="flex items-center justify-center w-4 h-4">
                  {globalPreferences.foldersFirst ? (
                    <Check className="w-4 h-4 text-app-accent" weight="bold" />
                  ) : null}
                </div>
                <div className="flex items-center gap-2"><span>Folders on Top</span></div>
              </button>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
