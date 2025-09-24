import { create } from 'zustand';

export type ToastType = 'success' | 'error' | 'info';

export interface Toast {
  id: string;
  message: string;
  type: ToastType;
  duration?: number;
  action?: {
    label: string;
    onClick: () => void;
  };
  closing?: boolean;
}

interface ToastStore {
  toasts: Toast[];
  addToast: (toast: Omit<Toast, 'id'>) => string;
  removeToast: (id: string) => void;
}

const FADE_DURATION = 250;

export const useToastStore = create<ToastStore>((set, get) => ({
  toasts: [],

  addToast: (toast) => {
    const id = Math.random().toString(36).substring(7);
    const newToast = { ...toast, id, closing: false };

    set((state) => ({
      toasts: [...state.toasts, newToast],
    }));

    // Auto-remove after duration (default 5 seconds)
    const duration = toast.duration ?? 5000;
    if (duration > 0) {
      setTimeout(() => {
        // Abort if toast already removed
        if (!get().toasts.some((t) => t.id === id)) return;

        set((state) => ({
          toasts: state.toasts.map((t) => (t.id === id ? { ...t, closing: true } : t)),
        }));

        setTimeout(() => {
          set((state) => ({
            toasts: state.toasts.filter((t) => t.id !== id),
          }));
        }, FADE_DURATION);
      }, duration);
    }

    return id;
  },

  removeToast: (id) => {
    if (!get().toasts.some((t) => t.id === id)) return;

    set((state) => ({
      toasts: state.toasts.map((t) => (t.id === id ? { ...t, closing: true } : t)),
    }));

    setTimeout(() => {
      set((state) => ({
        toasts: state.toasts.filter((t) => t.id !== id),
      }));
    }, FADE_DURATION);
  },
}));
