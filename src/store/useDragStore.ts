import { create } from 'zustand';

interface DraggedDirectory {
  path: string;
  name: string;
}

interface DragStore {
  // Native drag tracking (for directories being dragged to external apps or sidebar)
  nativeDragDirectory: DraggedDirectory | null;
  // Paths placed on the native pasteboard by this app while a native drag is active.
  nativeDragPaths: string[];
  // In-app hover target for non-native drags
  inAppDropTargetId: string | null;
  dropTargetPath: string | null;
  pendingDropOperation: 'move' | 'copy' | null;
  isDraggingOver: boolean;

  // Start tracking a native drag of a directory
  startNativeDrag: (directory: DraggedDirectory) => void;
  setNativeDragPaths: (paths: string[]) => void;
  clearNativeDragPaths: () => void;
  // End tracking of native drag
  endNativeDrag: () => void;
  // Check if a specific path is being dragged
  isDraggedDirectory: (path: string) => boolean;
  // Update in-app drop target for hover feedback
  setInAppDropTargetId: (targetId: string | null) => void;
  setDropTargetPath: (path: string | null) => void;
  setPendingDropOperation: (op: 'move' | 'copy' | null) => void;
  setIsDraggingOver: (isDragging: boolean) => void;
}

export const useDragStore = create<DragStore>((set, get) => ({
  nativeDragDirectory: null,
  nativeDragPaths: [],
  inAppDropTargetId: null,
  dropTargetPath: null,
  pendingDropOperation: null,
  isDraggingOver: false,

  startNativeDrag: (directory: DraggedDirectory) => {
    set({
      nativeDragDirectory: directory,
    });
  },

  setNativeDragPaths: (paths: string[]) => {
    set({ nativeDragPaths: paths });
  },

  clearNativeDragPaths: () => {
    set({ nativeDragPaths: [] });
  },

  endNativeDrag: () => {
    set({
      nativeDragDirectory: null,
      inAppDropTargetId: null,
    });
  },

  isDraggedDirectory: (path: string) => {
    const state = get();
    return state.nativeDragDirectory?.path === path;
  },

  setInAppDropTargetId: (targetId: string | null) => {
    if (get().inAppDropTargetId === targetId) return;
    set({ inAppDropTargetId: targetId });
  },

  setDropTargetPath: (path) => set({ dropTargetPath: path }),
  setPendingDropOperation: (op) => set({ pendingDropOperation: op }),
  setIsDraggingOver: (isDragging) => set({ isDraggingOver: isDragging }),
}));
