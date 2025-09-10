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
    selectedFiles,
    loading,
  } = useAppStore()

  // We rely solely on the native OS context menu now
  const scrollRef = useRef<HTMLDivElement>(null)
  const fileCtxCaptureRef = useRef<boolean>(false)
  const fileCtxPathRef = useRef<string | null>(null)

  const currentPrefs = {
    ...globalPreferences,
    ...directoryPreferences[currentPath],
  }

  const handleContextMenuCapture = (e: React.MouseEvent) => {
    const targetEl = e.target as HTMLElement
    const fileEl = targetEl && (targetEl.closest('[data-file-item="true"]') as HTMLElement | null)
    fileCtxCaptureRef.current = !!fileEl
    fileCtxPathRef.current = fileEl ? (fileEl.getAttribute('data-file-path') || null) : null
  }

  const handleContextMenu = async (e: React.MouseEvent) => {
    e.preventDefault()
    try {
      const win = getCurrentWindow()
      const state = useAppStore.getState()
      const path = state.currentPath
      const prefs = { ...state.globalPreferences, ...state.directoryPreferences[path] }
      const sortBy = prefs.sortBy
      const sortOrder = prefs.sortOrder
      let filePaths = state.selectedFiles
      const isFileCtx = fileCtxCaptureRef.current
      const ctxPath = fileCtxPathRef.current
      // If right-clicked a file, ensure it is selected and pass it explicitly
      if (isFileCtx && ctxPath) {
        if (!filePaths.includes(ctxPath)) {
          setSelectedFiles([ctxPath])
        }
        filePaths = [ctxPath]
      }
      fileCtxCaptureRef.current = false
      fileCtxPathRef.current = null

      await invoke('show_native_context_menu', {
        window_label: win.label,
        x: e.clientX,
        y: e.clientY,
        sort_by: sortBy,
        sort_order: sortOrder,
        path,
        has_file_context: isFileCtx || (Array.isArray(filePaths) && filePaths.length > 0),
        file_paths: filePaths,
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
        className="relative flex-1 min-h-0 overflow-auto"
        onContextMenuCapture={handleContextMenuCapture}
        onContextMenu={handleContextMenu}
        onClick={handleContainerBackgroundClick}
      >
        {/* Mask old content while loading to avoid layout flicker during view changes */}
        <div className={`${loading ? 'invisible' : 'visible'}`}>
          {currentPrefs.viewMode === 'grid' ? (
            <FileGrid files={files} preferences={currentPrefs} />
          ) : (
            <FileList files={files} preferences={currentPrefs} />
          )}
        </div>

        {loading && (
          <div className="absolute inset-0 flex items-center justify-center bg-app-dark">
            <div className="animate-spin w-6 h-6 border-2 border-[var(--accent)] border-t-transparent rounded-full" />
          </div>
        )}
      </div>
      
      {/* No React context menu fallback */}
    </div>
  )
}
