import { create } from 'zustand'

interface DraggedDirectory {
  path: string
  name: string
}

interface DragStore {
  // Native drag tracking (for directories being dragged to external apps or sidebar)
  nativeDragDirectory: DraggedDirectory | null
  
  // Start tracking a native drag of a directory
  startNativeDrag: (directory: DraggedDirectory) => void
  // End tracking of native drag
  endNativeDrag: () => void
  // Check if a specific path is being dragged
  isDraggedDirectory: (path: string) => boolean
}

export const useDragStore = create<DragStore>((set, get) => ({
  nativeDragDirectory: null,

  startNativeDrag: (directory: DraggedDirectory) => {
    console.log('🚀 Native drag started for directory:', directory)
    set({
      nativeDragDirectory: directory
    })
  },

  endNativeDrag: () => {
    console.log('🏁 Native drag ended')
    set({
      nativeDragDirectory: null
    })
  },

  isDraggedDirectory: (path: string) => {
    const state = get()
    return state.nativeDragDirectory?.path === path
  }
}))