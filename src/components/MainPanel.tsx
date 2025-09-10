import { useEffect, useRef, useState } from 'react'
import { useAppStore } from '../store/useAppStore'
import FileGrid from './FileGrid'
import FileList from './FileList'
import ContextMenu from './ContextMenu'

export default function MainPanel() {
  const {
    files,
    error,
    globalPreferences,
    currentPath,
    directoryPreferences,
  } = useAppStore()

  const [contextMenu, setContextMenu] = useState<{ x: number; y: number } | null>(null)
  const scrollRef = useRef<HTMLDivElement>(null)

  const currentPrefs = {
    ...globalPreferences,
    ...directoryPreferences[currentPath],
  }

  const handleContextMenu = (e: React.MouseEvent) => {
    e.preventDefault()
    setContextMenu({ x: e.clientX, y: e.clientY })
  }

  const closeContextMenu = () => {
    setContextMenu(null)
  }

  // Reset scroll when navigating to a new path
  useEffect(() => {
    const el = scrollRef.current
    if (el) el.scrollTo({ top: 0, left: 0, behavior: 'auto' })
  }, [currentPath])

  // View and sort are now controlled via the system menu and keyboard shortcuts

  if (error) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <div className="text-app-red">Error: {error}</div>
      </div>
    )
  }

  

  return (
    <div className="flex-1 flex flex-col select-none min-h-0">
      {/* File content only */}
      <div ref={scrollRef} className="flex-1 min-h-0 overflow-auto" onContextMenu={handleContextMenu}>
        {currentPrefs.viewMode === 'grid' ? (
          <FileGrid files={files} preferences={currentPrefs} />
        ) : (
          <FileList files={files} preferences={currentPrefs} />
        )}
      </div>
      
      {/* Context Menu */}
      {contextMenu && (
        <ContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          onClose={closeContextMenu}
        />
      )}
    </div>
  )
}
