import { create } from 'zustand';
import type { ClipboardProgressPayload, ClipboardProgressUpdatePayload } from '@/types';

interface ClipboardProgressState {
  operation?: string;
  destination?: string;
  totalItems: number;
  completed: number;
  currentItem?: string;
  recentItems: string[];
  finished: boolean;
  error?: string;
  setContext: (payload: ClipboardProgressPayload) => void;
  pushUpdate: (payload: ClipboardProgressUpdatePayload) => void;
  reset: () => void;
}

const INITIAL_STATE: Omit<ClipboardProgressState, 'setContext' | 'pushUpdate' | 'reset'> = {
  operation: undefined,
  destination: undefined,
  totalItems: 0,
  completed: 0,
  currentItem: undefined,
  recentItems: [],
  finished: false,
  error: undefined,
};

export const useClipboardProgressStore = create<ClipboardProgressState>((set) => ({
  ...INITIAL_STATE,
  setContext: (payload) =>
    set({
      operation: payload.operation,
      destination: payload.destination,
      totalItems: payload.totalItems,
      completed: 0,
      currentItem: undefined,
      recentItems: [],
      finished: false,
      error: undefined,
    }),
  pushUpdate: (payload) =>
    set((state) => ({
      operation: payload.operation ?? state.operation,
      destination: payload.destination ?? state.destination,
      totalItems: payload.total ?? state.totalItems,
      completed: payload.completed,
      currentItem: payload.currentItem ?? state.currentItem,
      recentItems: payload.currentItem
        ? [...state.recentItems, payload.currentItem].slice(-12)
        : state.recentItems,
      finished: payload.finished,
      error: payload.error ?? state.error,
    })),
  reset: () => set({ ...INITIAL_STATE }),
}));
