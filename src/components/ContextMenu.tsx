import { useEffect, useRef } from 'react'
import { Check, Eye, EyeSlash } from 'phosphor-react'
import { useAppStore } from '../store/useAppStore'

interface ContextMenuProps {
  x: number
  y: number
  onClose: () => void
}

export default function ContextMenu({ x, y, onClose }: ContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null)
  const { globalPreferences, toggleHiddenFiles: centralizedToggle } = useAppStore()

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
        >
          <div className="flex items-center justify-center w-4 h-4">
            {globalPreferences.showHidden ? (
              <Check className="w-4 h-4 text-app-accent" weight="bold" />
            ) : null}
          </div>
          <div className="flex items-center gap-2">
            {globalPreferences.showHidden ? (
              <EyeSlash className="w-4 h-4" />
            ) : (
              <Eye className="w-4 h-4" />
            )}
            <span>Show Hidden Files</span>
          </div>
        </button>
      </div>
    </div>
  )
}
