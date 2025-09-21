import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { FolderSizeProgressPayload } from '@/types';

export interface FolderSizeTarget {
  path: string;
  name: string;
  isDirectory: boolean;
}

interface FolderSizeState {
  requestId?: string;
  targets: FolderSizeTarget[];
  totalBytes: number;
  totalApparentBytes: number;
  totalItems: number;
  startedAt?: number;
  updatedAt?: number;
  completedAt?: number;
  cancelled: boolean;
  cancelRequested: boolean;
  isRunning: boolean;
  error?: string;
  lastPath?: string;
  initializeAndStart: (
    requestId: string,
    targets: FolderSizeTarget[],
    options?: {
      invokeBackend?: boolean;
      markRunning?: boolean;
    }
  ) => Promise<void>;
  applyProgress: (payload: FolderSizeProgressPayload) => void;
  cancel: () => Promise<void>;
  reset: () => void;
}

const INITIAL_STATE: Omit<
  FolderSizeState,
  'initializeAndStart' | 'applyProgress' | 'cancel' | 'reset'
> = {
  requestId: undefined,
  targets: [],
  totalBytes: 0,
  totalApparentBytes: 0,
  totalItems: 0,
  startedAt: undefined,
  updatedAt: undefined,
  completedAt: undefined,
  cancelled: false,
  cancelRequested: false,
  isRunning: false,
  error: undefined,
  lastPath: undefined,
};

const extractErrorMessage = (error: unknown): string => {
  if (typeof error === 'string') return error;
  if (error && typeof error === 'object' && 'message' in error) {
    const { message } = error as { message?: unknown };
    if (typeof message === 'string') {
      return message;
    }
  }
  try {
    return JSON.stringify(error);
  } catch {
    return 'Unknown error';
  }
};

export const useFolderSizeStore = create<FolderSizeState>((set, get) => ({
  ...INITIAL_STATE,

  initializeAndStart: async (requestId, targets, options) => {
    if (!requestId || !targets || targets.length === 0) {
      return;
    }

    const shouldInvoke = options?.invokeBackend ?? true;
    const shouldMarkRunning = options?.markRunning ?? true;
    const now = Date.now();

    set({
      ...INITIAL_STATE,
      requestId,
      targets,
      isRunning: shouldMarkRunning,
      startedAt: now,
      updatedAt: now,
    });

    if (!shouldInvoke) {
      return;
    }

    try {
      await invoke('calculate_folder_size', {
        requestId,
        paths: targets.map((target) => target.path),
      });
    } catch (error) {
      console.warn('Failed to start folder size calculation:', error);
      const message = extractErrorMessage(error);
      set({
        isRunning: false,
        cancelRequested: false,
        error: message,
        updatedAt: Date.now(),
        completedAt: Date.now(),
      });
    }
  },

  applyProgress: (payload) => {
    const { requestId } = get();
    if (!requestId || payload.requestId !== requestId) {
      return;
    }

    set((state) => {
      const finished = Boolean(payload.finished);
      const cancelled = Boolean(payload.cancelled);
      const hasError = Boolean(payload.error);
      const running = !finished && !cancelled && !hasError;

      const next: Partial<FolderSizeState> = {
        totalBytes: payload.totalBytes,
        totalApparentBytes: payload.totalApparentBytes ?? payload.totalBytes,
        totalItems: payload.totalItems,
        updatedAt: Date.now(),
        lastPath: payload.currentPath ?? state.lastPath,
        isRunning: running,
      };

      if (payload.error) {
        next.error = payload.error;
      }

      if (finished || cancelled || hasError) {
        next.cancelled = cancelled;
        next.cancelRequested = false;
        next.completedAt = Date.now();
      }
      return { ...state, ...next };
    });
  },

  cancel: async () => {
    const { requestId, cancelRequested, isRunning } = get();
    if (!requestId || cancelRequested || !isRunning) {
      return;
    }

    set({ cancelRequested: true });
    try {
      await invoke('cancel_folder_size_calculation', { requestId });
    } catch (error) {
      console.warn('Failed to cancel folder size calculation:', error);
      const message = extractErrorMessage(error);
      set({ cancelRequested: false, error: message });
    }
  },

  reset: () => {
    set({ ...INITIAL_STATE });
  },
}));

export const openFolderSizeWindow = async (targets: FolderSizeTarget[]) => {
  if (!targets || targets.length === 0) {
    return;
  }

  const hasDirectory = targets.some((target) => target.isDirectory);
  if (!hasDirectory) {
    return;
  }

  const dedup = new Map<string, FolderSizeTarget>();
  for (const target of targets) {
    dedup.set(target.path, target);
  }

  const payload = Array.from(dedup.values()).map((target) => ({
    path: target.path,
    name: target.name,
    isDirectory: target.isDirectory,
  }));

  try {
    await invoke('open_folder_size_window', { targets: payload });
  } catch (error) {
    console.warn('Failed to open folder size window:', error);
    // TODO: Add toast notification when toast store is implemented
    // For now, just log the error
    console.error('Unable to open folder size window:', error);
  }
};
