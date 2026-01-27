import { create } from 'zustand';
import type { CompressProgressPayload } from '@/types';

// Cap entries array to prevent unbounded memory growth (UI only shows last 8)
const MAX_ENTRIES = 200;

interface CompressProgressState {
  archiveName?: string;
  entries: string[];
  currentEntry?: string;
  completed: number;
  total: number;
  finished: boolean;
  error?: string;
  setContext: (payload: CompressProgressPayload) => void;
  pushUpdate: (payload: CompressProgressPayload) => void;
  reset: () => void;
}

const INITIAL_STATE: Omit<CompressProgressState, 'setContext' | 'pushUpdate' | 'reset'> = {
  archiveName: undefined,
  entries: [],
  currentEntry: undefined,
  completed: 0,
  total: 0,
  finished: false,
  error: undefined,
};

export const useCompressProgressStore = create<CompressProgressState>((set) => ({
  ...INITIAL_STATE,
  setContext: (payload) =>
    set({
      archiveName: payload.archiveName,
      entries: payload.entryName ? [payload.entryName] : [],
      currentEntry: payload.entryName,
      completed: payload.completed,
      total: payload.total,
      finished: payload.finished,
      error: payload.error,
    }),
  pushUpdate: (payload) =>
    set((state) => {
      let newEntries = state.entries;
      if (payload.entryName) {
        newEntries = [...state.entries, payload.entryName];
        if (newEntries.length > MAX_ENTRIES) {
          newEntries = newEntries.slice(-MAX_ENTRIES);
        }
      }
      return {
        archiveName: payload.archiveName ?? state.archiveName,
        entries: newEntries,
        currentEntry: payload.entryName ?? state.currentEntry,
        completed: payload.completed ?? state.completed,
        total: payload.total ?? state.total,
        finished: payload.finished ?? state.finished,
        error: payload.error ?? state.error,
      };
    }),
  reset: () => set({ ...INITIAL_STATE }),
}));
