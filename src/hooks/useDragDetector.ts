import { useEffect, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen, UnlistenFn } from '@tauri-apps/api/event'

export interface DragDropEvent {
  paths: string[]
  location: {
    x: number
    y: number
    targetId: string | null
  }
  eventType: 'dragEnter' | 'dragOver' | 'dragLeave' | 'drop'
}

export interface DragDropHandlers {
  onDragEnter?: (event: DragDropEvent) => void
  onDragOver?: (event: DragDropEvent) => void
  onDragLeave?: (event: DragDropEvent) => void
  onDrop?: (event: DragDropEvent) => void
}

/**
 * Hook to handle native drag and drop events within the application
 * This uses a custom Tauri plugin to detect drops that the native drag API can't handle
 */
export function useDragDetector(handlers: DragDropHandlers) {
  const enableDragDetection = useCallback(async () => {
    try {
      await invoke('plugin:drag-detector|enable_drag_detection')
    } catch (error) {
      console.error('Failed to enable drag detection:', error)
    }
  }, [])

  const setDropZone = useCallback(async (zoneId: string, enabled: boolean) => {
    try {
      await invoke('plugin:drag-detector|set_drop_zone', { 
        zoneId, 
        enabled 
      })
    } catch (error) {
      console.error('Failed to set drop zone:', error)
    }
  }, [])

  useEffect(() => {
    let unlisten: UnlistenFn | null = null

    const setupListener = async () => {
      // Enable drag detection when component mounts
      await enableDragDetection()

      // Listen for drag-drop events from the plugin
      unlisten = await listen<DragDropEvent>('drag-drop-event', (event) => {
        const { eventType } = event.payload

        switch (eventType) {
          case 'dragEnter':
            handlers.onDragEnter?.(event.payload)
            break
          case 'dragOver':
            handlers.onDragOver?.(event.payload)
            break
          case 'dragLeave':
            handlers.onDragLeave?.(event.payload)
            break
          case 'drop':
            handlers.onDrop?.(event.payload)
            break
        }
      })
    }

    setupListener()

    return () => {
      if (unlisten) {
        unlisten()
      }
    }
  }, [handlers, enableDragDetection])

  return {
    setDropZone,
    enableDragDetection
  }
}

/**
 * Hook specifically for sidebar drop zone
 */
export function useSidebarDropZone(onDrop: (paths: string[]) => void) {
  const handleDrop = useCallback((event: DragDropEvent) => {
    if (event.location.targetId === 'sidebar' && event.paths.length > 0) {
      onDrop(event.paths)
    }
  }, [onDrop])

  const { setDropZone } = useDragDetector({
    onDrop: handleDrop,
    onDragEnter: (event) => {
      if (event.location.targetId === 'sidebar') {
        // Add visual feedback
        const sidebar = document.querySelector('[data-sidebar="true"]')
        if (sidebar) {
          sidebar.classList.add('drag-over')
        }
      }
    },
    onDragLeave: (event) => {
      if (event.location.targetId === 'sidebar') {
        // Remove visual feedback
        const sidebar = document.querySelector('[data-sidebar="true"]')
        if (sidebar) {
          sidebar.classList.remove('drag-over')
        }
      }
    }
  })

  useEffect(() => {
    // Enable sidebar as a drop zone
    setDropZone('sidebar', true)

    return () => {
      // Disable sidebar as a drop zone on unmount
      setDropZone('sidebar', false)
    }
  }, [setDropZone])
}