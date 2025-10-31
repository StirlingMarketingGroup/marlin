import { create } from 'zustand';
import type {
  DeleteItemPayload,
  DeleteProgressPayload,
  DeleteProgressUpdatePayload,
} from '@/types';

interface DeleteProgressState {
  requestId?: string;
  items: DeleteItemPayload[];
  totalItems: number;
  completed: number;
  currentPath?: string;
  finished: boolean;
  error?: string;
  history: string[];
  setContext: (payload: DeleteProgressPayload) => void;
  applyUpdate: (payload: DeleteProgressUpdatePayload) => void;
  reset: () => void;
}

const INITIAL_STATE: Omit<DeleteProgressState, 'setContext' | 'applyUpdate' | 'reset'> = {
  requestId: undefined,
  items: [],
  totalItems: 0,
  completed: 0,
  currentPath: undefined,
  finished: false,
  error: undefined,
  history: [],
};

export const useDeleteProgressStore = create<DeleteProgressState>((set) => ({
  ...INITIAL_STATE,
  setContext: (payload) =>
    set({
      requestId: payload.requestId,
      items: payload.items,
      totalItems: payload.totalItems,
      completed: 0,
      currentPath: undefined,
      finished: false,
      error: undefined,
      history: [],
    }),
  applyUpdate: (payload) =>
    set((state) => {
      if (state.requestId && payload.requestId !== state.requestId) {
        return state;
      }

      const total = typeof payload.total === 'number' ? payload.total : state.totalItems;
      const completedTarget =
        typeof payload.completed === 'number' ? payload.completed : state.completed;
      const nextCompleted = Math.min(completedTarget, total);
      const shouldRecord = payload.currentPath && nextCompleted > state.completed;

      return {
        requestId: payload.requestId,
        completed: nextCompleted,
        totalItems: total,
        currentPath: payload.currentPath ?? state.currentPath,
        finished: payload.finished ?? state.finished,
        error: payload.error ?? (payload.finished ? undefined : state.error),
        history: shouldRecord
          ? [...state.history.slice(-15), payload.currentPath as string]
          : state.history,
        items: state.items,
      };
    }),
  reset: () => set({ ...INITIAL_STATE }),
}));
