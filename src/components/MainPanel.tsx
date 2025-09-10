import { useEffect, useRef, useState, MouseEvent } from 'react'
import { useAppStore } from '../store/useAppStore'
import FileGrid from './FileGrid'
import FileList from './FileList'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { invoke } from '@tauri-apps/api/core'

export default function MainPanel() {
  const {
    files,
    error,
    globalPreferences,
    currentPath,
    directoryPreferences,
    setSelectedFiles,
  } = useAppStore()

  // We rely solely on the native OS context menu now
  const scrollRef = useRef<HTMLDivElement>(null)

  const currentPrefs = {
    ...globalPreferences,
    ...directoryPreferences[currentPath],
  }

  const handleContextMenu = async (e: React.MouseEvent) => {
    e.preventDefault()
    try {
      const win = getCurrentWindow()
      // Prefer native OS context menu
      // Use the same merged prefs driving the UI header so checks match exactly
      const sortBy = currentPrefs.sortBy
      const sortOrder = currentPrefs.sortOrder
      // Debug in DevTools to verify values we pass to backend
      console.debug('[ContextMenu] passing', { path: currentPath, sortBy, sortOrder })

      await invoke('show_native_context_menu', {
        window_label: win.label,
        // Tauri expects position relative to window's top-left (logical coords)
        x: e.clientX,
        y: e.clientY,
        sort_by: sortBy,
        sort_order: sortOrder,
        path: currentPath,
      })
      return
    } catch (_) {
      // If native menu fails for some reason, silently ignore
    }
  }

  // No custom context menu fallback

  // Reset scroll when navigating to a new path
  useEffect(() => {
    const el = scrollRef.current
    if (el) el.scrollTo({ top: 0, left: 0, behavior: 'auto' })
  }, [currentPath])

  // Clear selection when clicking anywhere that's not an interactive control
  const handleContainerBackgroundClick = (e: React.MouseEvent<HTMLDivElement>) => {
    const target = e.target as HTMLElement
    // Ignore clicks on obvious controls
    if (target.closest('button, a, input, select, textarea, [role="button"], [data-prevent-deselect]')) return
    setSelectedFiles([])
  }

  // Manual dragging fallback function for MainPanel
  const handleManualDrag = async (e: MouseEvent<HTMLDivElement>) => {
    // Only start drag on primary button (left click)
    if (e.button !== 0) return
    
    // Check if clicked element is interactive or a file item
    const target = e.target as HTMLElement
    if (target.closest('[data-tauri-drag-region="false"], button, input, select, textarea, [role="button"]')) return
    
    try {
      const window = getCurrentWindow()
      await window.startDragging()
    } catch (error) {
      console.error('Failed to start window dragging from MainPanel:', error)
    }
  }

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
      <div
        ref={scrollRef}
        className="flex-1 min-h-0 overflow-auto"
        onContextMenu={handleContextMenu}
        onClick={handleContainerBackgroundClick}
      >
        {currentPrefs.viewMode === 'grid' ? (
          <FileGrid files={files} preferences={currentPrefs} />
        ) : (
          <FileList files={files} preferences={currentPrefs} />
        )}
      </div>
      
      {/* No React context menu fallback */}
    </div>
  )
}
