import { create } from 'zustand';
import type { ConflictPayload, ConflictAction } from '@/types';

interface ConflictState {
  conflict: ConflictPayload | undefined;
  customName: string;
  applyToAll: boolean;
  selectedAction: ConflictAction | undefined;

  setConflict: (payload: ConflictPayload) => void;
  setCustomName: (name: string) => void;
  setApplyToAll: (value: boolean) => void;
  setSelectedAction: (action: ConflictAction) => void;
  reset: () => void;
}

export const useConflictStore = create<ConflictState>((set) => ({
  conflict: undefined,
  customName: '',
  applyToAll: false,
  selectedAction: undefined,

  setConflict: (payload) =>
    set({
      conflict: payload,
      customName: payload.source.name,
      // Pre-select merge when both are directories, replace for files
      selectedAction:
        payload.source.isDirectory && payload.destination.isDirectory ? 'merge' : 'replace',
    }),

  setCustomName: (name) => set({ customName: name }),

  setApplyToAll: (value) =>
    set((state) => ({
      applyToAll: value,
      // Can't batch-rename, so reset action if it was rename
      selectedAction: value && state.selectedAction === 'rename' ? undefined : state.selectedAction,
    })),

  setSelectedAction: (action) =>
    set((state) => ({
      selectedAction: action,
      // Can't use apply-to-all with rename
      applyToAll: action === 'rename' ? false : state.applyToAll,
    })),

  reset: () =>
    set({
      conflict: undefined,
      customName: '',
      applyToAll: false,
      selectedAction: undefined,
    }),
}));
