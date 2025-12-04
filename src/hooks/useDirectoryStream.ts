import { useEffect } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useAppStore } from '@/store/useAppStore';
import type { DirectoryBatch } from '@/types';

/**
 * Hook that sets up the event listener for directory batch streaming.
 * Should be called once at the app root level.
 */
export function useDirectoryStream() {
  const appendStreamingBatch = useAppStore((state) => state.appendStreamingBatch);

  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;

    const setupListener = async () => {
      const fn = await listen<DirectoryBatch>('directory-batch', (event) => {
        if (!cancelled) {
          appendStreamingBatch(event.payload);
        }
      });

      if (cancelled) {
        // Effect was cleaned up while we were setting up - unlisten immediately
        fn();
      } else {
        unlisten = fn;
      }
    };

    setupListener().catch((err) => {
      if (!cancelled) {
        console.error('Failed to set up directory stream listener:', err);
      }
    });

    return () => {
      cancelled = true;
      if (unlisten) {
        unlisten();
      }
    };
  }, [appendStreamingBatch]);
}
