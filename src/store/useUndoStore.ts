import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { useToastStore } from './useToastStore';
import type { UndoTrashResponse } from '../types';

// Types for undo records
export type UndoOperationType = 'move' | 'copy' | 'rename' | 'trash';

export interface UndoMoveRecord {
  type: 'move';
  files: Array<{ originalPath: string; currentPath: string }>;
}

export interface UndoCopyRecord {
  type: 'copy';
  copiedPaths: string[]; // Will be trashed on undo
}

export interface UndoRenameRecord {
  type: 'rename';
  originalPath: string;
  newPath: string;
}

export interface UndoTrashRecord {
  type: 'trash';
  undoToken: string;
  trashedPaths: string[];
}

export type UndoRecord = UndoMoveRecord | UndoCopyRecord | UndoRenameRecord | UndoTrashRecord;

export interface UndoStackEntry {
  id: string;
  record: UndoRecord;
  createdAt: number;
  description: string;
}

const MAX_STACK_SIZE = 10;
const UNDO_TTL_MS = 5 * 60 * 1000; // 5 minutes

interface UndoState {
  stack: UndoStackEntry[];
  undoInProgress: boolean;
  pushUndo: (record: UndoRecord, description: string) => string;
  popUndo: () => UndoStackEntry | undefined;
  removeById: (id: string) => void;
  executeUndo: () => Promise<void>;
  executeUndoRecord: (record: UndoRecord) => Promise<void>;
  getValidStack: () => UndoStackEntry[];
}

// Helper to generate unique IDs
const generateId = () => crypto.randomUUID();

// Execute the undo for a specific record (internal helper)
// Returns paths that were restored/affected for potential selection
async function executeUndoRecordInternal(record: UndoRecord): Promise<string[]> {
  switch (record.type) {
    case 'move': {
      // Move files back to their original locations
      const restoredPaths: string[] = [];
      for (const file of record.files) {
        await invoke('rename_file', {
          fromPath: file.currentPath,
          toPath: file.originalPath,
        });
        restoredPaths.push(file.originalPath);
      }
      return restoredPaths;
    }
    case 'copy': {
      // Send copied files to trash
      await invoke('trash_paths', { paths: record.copiedPaths });
      return [];
    }
    case 'rename': {
      // Rename back to original name
      await invoke('rename_file', {
        fromPath: record.newPath,
        toPath: record.originalPath,
      });
      return [record.originalPath];
    }
    case 'trash': {
      // Restore from trash using the undo token
      const result = await invoke<UndoTrashResponse>('undo_trash', { token: record.undoToken });
      return result.restored;
    }
  }
}

export const useUndoStore = create<UndoState>((set, get) => ({
  stack: [],
  undoInProgress: false,

  pushUndo: (record, description) => {
    const id = generateId();
    const entry: UndoStackEntry = {
      id,
      record,
      createdAt: Date.now(),
      description,
    };

    set((state) => {
      // Filter out expired entries and add new one
      const now = Date.now();
      const validEntries = state.stack.filter((e) => now - e.createdAt < UNDO_TTL_MS);

      // Keep only the most recent entries if we're at max
      const trimmed =
        validEntries.length >= MAX_STACK_SIZE
          ? validEntries.slice(-(MAX_STACK_SIZE - 1))
          : validEntries;

      return { stack: [...trimmed, entry] };
    });

    return id;
  },

  popUndo: () => {
    const state = get();
    const now = Date.now();

    // Find the most recent valid entry (not expired)
    const validStack = state.stack.filter((e) => now - e.createdAt < UNDO_TTL_MS);

    if (validStack.length === 0) {
      // Clean up expired entries
      set({ stack: [] });
      return undefined;
    }

    const entry = validStack[validStack.length - 1];

    // Remove the entry from the stack
    set({ stack: validStack.slice(0, -1) });

    return entry;
  },

  removeById: (id) => {
    set((state) => ({
      stack: state.stack.filter((e) => e.id !== id),
    }));
  },

  getValidStack: () => {
    const now = Date.now();
    return get().stack.filter((e) => now - e.createdAt < UNDO_TTL_MS);
  },

  executeUndo: async () => {
    const state = get();
    const toastStore = useToastStore.getState();

    // Prevent double-undo
    if (state.undoInProgress) {
      return;
    }

    const entry = state.popUndo();
    if (!entry) {
      toastStore.addToast({
        type: 'info',
        message: 'Nothing to undo.',
        duration: 3000,
      });
      return;
    }

    set({ undoInProgress: true });

    try {
      const restoredPaths = await executeUndoRecordInternal(entry.record);

      // Dynamically import useAppStore to avoid circular dependency
      const { useAppStore } = await import('./useAppStore');
      const appStore = useAppStore.getState();

      // Refresh the current directory to show changes
      if (appStore.currentPath?.includes('://')) {
        await appStore.refreshCurrentDirectory();
      } else {
        await appStore.refreshCurrentDirectoryStreaming();
      }

      // Select restored files if any
      if (restoredPaths.length > 0) {
        appStore.setSelectedFiles(restoredPaths);
      }

      toastStore.addToast({
        type: 'success',
        message: `Undone: ${entry.description}`,
        duration: 4000,
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      toastStore.addToast({
        type: 'error',
        message: `Undo failed: ${message}`,
        duration: 6000,
      });
    } finally {
      set({ undoInProgress: false });
    }
  },

  // Execute a specific undo record (called from toast action buttons)
  executeUndoRecord: async (record: UndoRecord) => {
    const state = get();
    const toastStore = useToastStore.getState();

    // Prevent double-undo
    if (state.undoInProgress) {
      return;
    }

    set({ undoInProgress: true });

    try {
      const restoredPaths = await executeUndoRecordInternal(record);

      // Dynamically import useAppStore to avoid circular dependency
      const { useAppStore } = await import('./useAppStore');
      const appStore = useAppStore.getState();

      // Refresh the current directory to show changes
      if (appStore.currentPath?.includes('://')) {
        await appStore.refreshCurrentDirectory();
      } else {
        await appStore.refreshCurrentDirectoryStreaming();
      }

      // Select restored files if any
      if (restoredPaths.length > 0) {
        appStore.setSelectedFiles(restoredPaths);
      }

      toastStore.addToast({
        type: 'success',
        message: 'Undone',
        duration: 4000,
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      toastStore.addToast({
        type: 'error',
        message: `Undo failed: ${message}`,
        duration: 6000,
      });
    } finally {
      set({ undoInProgress: false });
    }
  },
}));

// Expose store for e2e testing (dev only)
if (typeof window !== 'undefined' && import.meta.env.DEV) {
  (window as unknown as { __UNDO_STORE__: typeof useUndoStore }).__UNDO_STORE__ = useUndoStore;
}
