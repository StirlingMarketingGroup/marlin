import { create } from 'zustand'

interface DraggedDirectory {
  path: string
  name: string
}

interface DragStore {
  isDragging: boolean
  draggedDirectory: DraggedDirectory | null
  dragPreviewPosition: { x: number; y: number } | null
  
  startDrag: (directory: DraggedDirectory) => void
  updateDragPosition: (x: number, y: number) => void
  endDrag: () => void
  isDraggedDirectory: (path: string) => boolean
}

export const useDragStore = create<DragStore>((set, get) => ({
  isDragging: false,
  draggedDirectory: null,
  dragPreviewPosition: null,

  startDrag: (directory: DraggedDirectory) => {
    console.log('🚀 Manual drag started:', directory)
    // Alert for debugging
    // alert(`Manual drag started for: ${directory.name}`)
    set({
      isDragging: true,
      draggedDirectory: directory,
      dragPreviewPosition: { x: 100, y: 100 } // Start with a visible position
    })
  },

  updateDragPosition: (x: number, y: number) => {
    set({ dragPreviewPosition: { x, y } })
  },

  endDrag: () => {
    console.log('🏁 Manual drag ended')
    set({
      isDragging: false,
      draggedDirectory: null,
      dragPreviewPosition: null
    })
  },

  isDraggedDirectory: (path: string) => {
    const state = get()
    return state.isDragging && state.draggedDirectory?.path === path
  }
}))