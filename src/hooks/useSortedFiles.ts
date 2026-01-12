import { useMemo } from 'react';
import type { FileItem, ViewPreferences } from '../types';
import { useAppStore } from '../store/useAppStore';
import { isMacOSBundle } from '../utils/fileTypes';

/**
 * Hook to sort files with streaming-aware logic.
 * During streaming: sorts by name only (stable order as files arrive)
 * After streaming: applies user's full sort preferences
 */
export function useSortedFiles(files: FileItem[], preferences: ViewPreferences): FileItem[] {
  const isStreamingComplete = useAppStore((state) => state.isStreamingComplete);

  const nameCollator = useMemo(
    () => new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' }),
    []
  );

  return useMemo(() => {
    // During streaming: sort by name only (stable order as files arrive)
    // After streaming: apply user's full sort preferences
    const effectiveSortBy = isStreamingComplete ? preferences.sortBy : 'name';

    return [...files].sort((a, b) => {
      // Treat macOS bundles (.app, .photoslibrary, etc.) as files for sorting purposes
      const aIsBundle = isMacOSBundle(a);
      const bIsBundle = isMacOSBundle(b);
      const aIsFolder = a.is_directory && !aIsBundle;
      const bIsFolder = b.is_directory && !bIsBundle;

      // Optionally sort directories first (but not bundles)
      if (preferences.foldersFirst) {
        if (aIsFolder && !bIsFolder) return -1;
        if (!aIsFolder && bIsFolder) return 1;
      }

      let compareValue = 0;
      switch (effectiveSortBy) {
        case 'name':
          compareValue = nameCollator.compare(a.name, b.name);
          break;
        case 'size':
          compareValue = a.size - b.size;
          break;
        case 'modified':
          compareValue = new Date(a.modified).getTime() - new Date(b.modified).getTime();
          break;
        case 'type':
          compareValue = (a.extension || '').localeCompare(b.extension || '');
          break;
      }

      return preferences.sortOrder === 'asc' ? compareValue : -compareValue;
    });
  }, [
    files,
    preferences.sortBy,
    preferences.sortOrder,
    preferences.foldersFirst,
    isStreamingComplete,
    nameCollator,
  ]);
}
