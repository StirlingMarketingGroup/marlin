import { create } from 'zustand';
import type { ArchiveProgressPayload, ArchiveProgressUpdatePayload } from '@/types';

interface ArchiveProgressState {
  archiveName?: string;
  destinationDir?: string;
  format?: string;
  entries: string[];
  currentEntry?: string;
  finished: boolean;
  setContext: (payload: ArchiveProgressPayload) => void;
  pushUpdate: (payload: ArchiveProgressUpdatePayload) => void;
  reset: () => void;
}

const INITIAL_STATE: Omit<ArchiveProgressState, 'setContext' | 'pushUpdate' | 'reset'> = {
  archiveName: undefined,
  destinationDir: undefined,
  format: undefined,
  entries: [],
  currentEntry: undefined,
  finished: false,
};

export const useArchiveProgressStore = create<ArchiveProgressState>((set) => ({
  ...INITIAL_STATE,
  setContext: (payload) =>
    set({
      archiveName: payload.fileName,
      destinationDir: payload.destinationDir,
      format: payload.format,
      entries: [],
      currentEntry: undefined,
      finished: false,
    }),
  pushUpdate: (payload) =>
    set((state) => ({
      archiveName: state.archiveName ?? payload.archiveName,
      format: payload.format ?? state.format,
      entries: payload.entryName ? [...state.entries, payload.entryName] : state.entries,
      currentEntry: payload.entryName ?? state.currentEntry,
      finished: payload.finished ?? state.finished,
    })),
  reset: () => set({ ...INITIAL_STATE }),
}));
