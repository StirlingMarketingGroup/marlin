import { useRef, useEffect, useMemo } from 'react';
import { X, MagnifyingGlass } from 'phosphor-react';
import { useAppStore } from '@/store/useAppStore';
import { usePlatform } from '@/hooks/usePlatform';
import { useSortedFiles } from '@/hooks/useSortedFiles';
import type { FileItem } from '@/types';
import { matchesFilter } from '@/utils/filterMatch';

// Stable empty array to avoid infinite re-render loop in Zustand selector
const EMPTY_FILES: FileItem[] = [];

export default function FilterInput() {
  const {
    filterText,
    showFilterInput,
    setFilterText,
    clearFilter,
    openFilter,
    setSelectedFiles,
    setSelectionAnchor,
    setSelectionLead,
  } = useAppStore();
  // Read the files only while the box is open, so a closed box sorts nothing.
  const files = useAppStore((state) => (state.showFilterInput ? state.files : EMPTY_FILES));
  const globalPreferences = useAppStore((state) => state.globalPreferences);
  const directoryPreferences = useAppStore(
    (state) => state.directoryPreferences[state.currentPath]
  );
  const inputRef = useRef<HTMLInputElement>(null);
  const { isMac } = usePlatform();

  // The same files in the same order as the view, so the count and the arrow keys agree with it.
  const preferences = { ...globalPreferences, ...directoryPreferences };
  const sortedFiles = useSortedFiles(files, preferences);
  const filteredFiles = useMemo(() => {
    const hiddenFiltered = preferences.showHidden
      ? sortedFiles
      : sortedFiles.filter((file) => !file.is_hidden);
    return filterText
      ? hiddenFiltered.filter((file) => matchesFilter(file.name, filterText))
      : hiddenFiltered;
  }, [sortedFiles, preferences.showHidden, filterText]);
  const matchCount = filteredFiles.length;

  useEffect(() => {
    if (showFilterInput && inputRef.current) {
      inputRef.current.focus();
      // No select: the box opens with one typed character, or empty from the shortcut.
    }
  }, [showFilterInput]);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (
        (isMac ? e.metaKey : e.ctrlKey) &&
        !e.altKey &&
        !e.shiftKey &&
        e.key.toLowerCase() === 'f'
      ) {
        e.preventDefault();
        // Closed: openFilter mounts the input, and the effect above focuses it.
        // Open: the input is mounted, so focus it and select its text here.
        openFilter();
        inputRef.current?.focus();
        inputRef.current?.select();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [isMac, openFilter]);

  if (!showFilterInput) return null;

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      clearFilter();
      return;
    }

    // Arrow keys jump to file list
    if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && filteredFiles.length > 0) {
      e.preventDefault();
      e.stopPropagation();

      // Select first or last filtered file
      const targetFile =
        e.key === 'ArrowDown' ? filteredFiles[0] : filteredFiles[filteredFiles.length - 1];
      setSelectedFiles([targetFile.path]);
      setSelectionAnchor(targetFile.path);
      setSelectionLead(targetFile.path);

      // Blur input and focus the file container so arrow keys continue working
      inputRef.current?.blur();
      const fileContainer = document.querySelector('.file-grid, .file-list') as HTMLElement;
      fileContainer?.focus();
    }
  };

  return (
    <div className="fixed bottom-4 right-4 z-50 select-none" data-tauri-drag-region={false}>
      <div className="flex items-center gap-2 bg-app-gray/95 border border-app-border rounded-md px-2 py-1.5 shadow-lg backdrop-blur-sm">
        <MagnifyingGlass className="w-4 h-4 text-app-muted flex-shrink-0" />
        <input
          ref={inputRef}
          type="text"
          value={filterText}
          onChange={(e) => setFilterText(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="Filter..."
          data-testid="filter-input"
          className="bg-transparent border-none outline-none text-sm w-40 text-app-text placeholder-app-muted"
        />
        {filterText && (
          <span className="text-xs text-app-muted whitespace-nowrap w-20 text-right">
            {matchCount} {matchCount === 1 ? 'match' : 'matches'}
          </span>
        )}
        <button
          className="p-0.5 rounded hover:bg-app-light"
          onClick={clearFilter}
          aria-label="Clear filter"
        >
          <X className="w-4 h-4" />
        </button>
      </div>
    </div>
  );
}
