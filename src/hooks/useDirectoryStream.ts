import { useEffect } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useAppStore } from '@/store/useAppStore';
import type { DirectoryBatch, MetadataBatch } from '@/types';

/**
 * Hook that sets up event listeners for directory batch streaming.
 * Listens for both skeleton batches (fast, names only) and metadata updates.
 * Should be called once at the app root level.
 */
export function useDirectoryStream() {
  const appendStreamingBatch = useAppStore((state) => state.appendStreamingBatch);
  const applyMetadataUpdates = useAppStore((state) => state.applyMetadataUpdates);

  useEffect(() => {
    let cancelled = false;
    let unlistenBatch: UnlistenFn | undefined;
    let unlistenMetadata: UnlistenFn | undefined;

    const setupListeners = async () => {
      // Listen for skeleton batches (file names, minimal info)
      const batchFn = await listen<DirectoryBatch>('directory-batch', (event) => {
        if (!cancelled) {
          appendStreamingBatch(event.payload);
        }
      });

      // Listen for metadata updates (size, modified, etc.)
      const metadataFn = await listen<MetadataBatch>('metadata-batch', (event) => {
        if (!cancelled) {
          applyMetadataUpdates(event.payload);
        }
      });

      if (cancelled) {
        // Effect was cleaned up while we were setting up - unlisten immediately
        batchFn();
        metadataFn();
      } else {
        unlistenBatch = batchFn;
        unlistenMetadata = metadataFn;
      }
    };

    setupListeners().catch((err) => {
      if (!cancelled) {
        console.error('Failed to set up directory stream listeners:', err);
      }
    });

    return () => {
      cancelled = true;
      if (unlistenBatch) {
        unlistenBatch();
      }
      if (unlistenMetadata) {
        unlistenMetadata();
      }
    };
  }, [appendStreamingBatch, applyMetadataUpdates]);
}
