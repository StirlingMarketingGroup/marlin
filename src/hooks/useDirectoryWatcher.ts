import { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useAppStore } from '@/store/useAppStore';
import { invalidateThumbnailsForPaths } from '@/hooks/useThumbnail';
import type { DirectoryChangeEventPayload, DirectoryListingResponse } from '@/types';

const RECONCILE_DELAY_MS = 400;
const STREAMING_RETRY_MS = 200;

const comparablePath = (path: string) => path.replace(/[\\/]+$/, '');

/**
 * Treat filesystem notifications only as invalidation signals. The directory
 * listing remains authoritative, and every refresh replaces the store by path.
 */
export function useDirectoryWatcher(currentPath: string) {
  useEffect(() => {
    if (!currentPath || /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(currentPath)) return;

    let cancelled = false;
    let watcherStarted = false;
    let unlisten: UnlistenFn | undefined;
    let reconcileTimer: number | undefined;
    let reconcileInFlight = false;
    let reconcileAgain = false;

    const watchedPath = comparablePath(currentPath);

    const scheduleReconciliation = (delay = RECONCILE_DELAY_MS) => {
      if (cancelled) return;
      if (reconcileTimer !== undefined) window.clearTimeout(reconcileTimer);
      reconcileTimer = window.setTimeout(() => {
        reconcileTimer = undefined;
        void reconcileDirectory();
      }, delay);
    };

    const reconcileDirectory = async () => {
      if (cancelled) return;

      const initialState = useAppStore.getState();
      if (initialState.streamingSessionId && !initialState.isStreamingComplete) {
        scheduleReconciliation(STREAMING_RETRY_MS);
        return;
      }

      if (reconcileInFlight) {
        reconcileAgain = true;
        return;
      }

      reconcileInFlight = true;
      try {
        do {
          reconcileAgain = false;
          const response = await invoke<DirectoryListingResponse>('read_directory', {
            path: currentPath,
          });

          if (cancelled) return;

          const state = useAppStore.getState();
          if (comparablePath(state.currentPath) !== watchedPath) return;

          // This is an authoritative snapshot. setFiles preserves computed metadata
          // while enforcing exactly one item for each filesystem path.
          state.setFiles(response.entries);

          if (state.selectedFiles.length > 0) {
            const existingPaths = new Set(response.entries.map((file) => file.path));
            const selectedFiles = state.selectedFiles.filter((path) => existingPaths.has(path));
            if (selectedFiles.length !== state.selectedFiles.length) {
              state.setSelectedFiles(selectedFiles);
            }
          }
        } while (!cancelled && reconcileAgain);
      } catch (error) {
        if (!cancelled) console.warn('Directory watcher reconciliation failed:', error);
      } finally {
        reconcileInFlight = false;
      }
    };

    const setup = async () => {
      try {
        // Listen first so changes cannot fall into a gap between starting the native
        // watcher and registering the frontend callback.
        const stopListening = await listen<DirectoryChangeEventPayload>(
          'directory-changed',
          (event) => {
            if (cancelled || comparablePath(event.payload.path) !== watchedPath) return;

            const changeType = event.payload.changeType.toLowerCase();
            if (
              (changeType === 'modified' || changeType === 'removed') &&
              event.payload.affectedPaths?.length
            ) {
              invalidateThumbnailsForPaths(event.payload.affectedPaths);
            }

            scheduleReconciliation();
          }
        );

        if (cancelled) {
          stopListening();
          return;
        }
        unlisten = stopListening;

        await invoke('start_watching_directory', { path: currentPath });
        watcherStarted = true;

        if (cancelled) {
          await invoke('stop_watching_directory', { path: currentPath }).catch(() => undefined);
          watcherStarted = false;
        }
      } catch (error) {
        if (!cancelled) console.warn('Failed to set up directory watcher:', error);
      }
    };

    void setup();

    return () => {
      cancelled = true;
      if (reconcileTimer !== undefined) window.clearTimeout(reconcileTimer);
      unlisten?.();
      if (watcherStarted) {
        void invoke('stop_watching_directory', { path: currentPath }).catch(() => undefined);
      }
    };
  }, [currentPath]);
}
