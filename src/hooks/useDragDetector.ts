import { useEffect, useCallback, useMemo } from 'react'
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
      await invoke('enable_drag_detection')
    } catch (error) {
      console.error('Failed to enable drag detection:', error)
    }
  }, [])

  const setDropZone = useCallback(async (zoneId: string, enabled: boolean, config?: { width?: number }) => {
    try {
      await invoke('set_drop_zone', {
        zoneId,
        enabled,
        config: config ?? null
      })
    } catch (error) {
      console.error('Failed to set drop zone:', error)
    }
  }, [])

  useEffect(() => {
    let unlisten: UnlistenFn | null = null
    let lastTargetId: string | null = null
    let lastLogTime = 0

    const setupListener = async () => {
      // Enable drag detection when component mounts
      await enableDragDetection()

      // Listen for drag-drop events from the plugin
      unlisten = await listen<DragDropEvent>('drag-drop-event', (event) => {
        const viewportHeight = window.innerHeight
        const deviceScale = window.devicePixelRatio || 1

        let zoneId: string | null = event.payload.location.targetId ?? null
        const domPoint = {
          x: event.payload.location.x,
          y: viewportHeight - event.payload.location.y
        }

        if (!zoneId) {
          if (Number.isFinite(domPoint.x) && Number.isFinite(domPoint.y)) {
            if (domPoint.x >= 0 && domPoint.x <= window.innerWidth && domPoint.y >= 0 && domPoint.y <= viewportHeight) {
              const element = document.elementFromPoint(domPoint.x, domPoint.y)
              const zoneEl = element?.closest<HTMLElement>('[data-drop-zone-id]')
              if (zoneEl) {
                zoneId = zoneEl.dataset.dropZoneId ?? null
              }
            }
          }
        }

        const normalizedEvent: DragDropEvent = {
          ...event.payload,
          location: {
            ...event.payload.location,
            x: domPoint.x,
            y: domPoint.y,
            targetId: zoneId
          }
        }

        const now = performance.now()
        if (
          import.meta.env.DEV &&
          normalizedEvent.eventType !== 'dragOver' &&
          (normalizedEvent.location.targetId !== lastTargetId || now - lastLogTime > 250)
        ) {
          lastTargetId = normalizedEvent.location.targetId ?? null
          lastLogTime = now
          console.debug('[drag-detector]', normalizedEvent)
        }

        switch (normalizedEvent.eventType) {
          case 'dragEnter':
            handlers.onDragEnter?.(normalizedEvent)
            break
          case 'dragOver':
            if (normalizedEvent.location.targetId) {
              handlers.onDragOver?.(normalizedEvent)
            } else {
              handlers.onDragLeave?.({
                ...normalizedEvent,
                eventType: 'dragLeave',
                location: {
                  ...normalizedEvent.location,
                  targetId: null
                }
              })
            }
            break
          case 'dragLeave':
            handlers.onDragLeave?.(normalizedEvent)
            break
          case 'drop':
            handlers.onDrop?.(normalizedEvent)
            break
        }
      })
    }

    setupListener()

    return () => {
      if (unlisten) {
        unlisten()
      }
      lastTargetId = null
      lastLogTime = 0
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
interface SidebarDropZoneHandlers {
  onDragEnter?: () => void
  onDragOver?: () => void
  onDragLeave?: () => void
}

interface SidebarDropZoneOptions extends SidebarDropZoneHandlers {
  width?: number
}

export function useSidebarDropZone(onDrop: (paths: string[]) => void, options?: SidebarDropZoneOptions) {
  const handleDrop = useCallback((event: DragDropEvent) => {
    if (event.location.targetId === 'sidebar' && event.paths.length > 0) {
      onDrop(event.paths)
    }
  }, [onDrop])

  const detectorHandlers = useMemo<DragDropHandlers>(() => ({
    onDrop: handleDrop,
    onDragEnter: (event) => {
      if (event.location.targetId === 'sidebar') {
        options?.onDragEnter?.()
      }
    },
    onDragOver: (event) => {
      if (event.location.targetId === 'sidebar') {
        options?.onDragOver?.()
      } else {
        options?.onDragLeave?.()
      }
    },
    onDragLeave: () => {
      options?.onDragLeave?.()
    }
  }), [handleDrop, options?.onDragEnter, options?.onDragOver, options?.onDragLeave])

  const { setDropZone } = useDragDetector(detectorHandlers)

  useEffect(() => {
    void setDropZone('sidebar', true)
    return () => {
      void setDropZone('sidebar', false)
    }
  }, [setDropZone])

  useEffect(() => {
    if (options?.width == null) return
    void setDropZone('sidebar', true, { width: options.width })
  }, [setDropZone, options?.width])
}
