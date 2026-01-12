import { useRef, useEffect } from 'react';
import { X, MagnifyingGlass } from 'phosphor-react';
import { useAppStore } from '@/store/useAppStore';
import type { FileItem } from '@/types';

// Stable empty array to avoid infinite re-render loop in Zustand selector
const EMPTY_FILES: FileItem[] = [];

export default function FilterInput() {
  const {
    filterText,
    showFilterInput,
    setFilterText,
    clearFilter,
    setSelectedFiles,
    setSelectionAnchor,
  } = useAppStore();
  // Only subscribe to files when we have a filter to avoid re-renders during streaming
  const files = useAppStore((state) => (state.filterText ? state.files : EMPTY_FILES));
  const inputRef = useRef<HTMLInputElement>(null);

  const filteredFiles = files.filter((f) =>
    f.name.toLowerCase().includes(filterText.toLowerCase())
  );
  const matchCount = filteredFiles.length;

  useEffect(() => {
    if (showFilterInput && inputRef.current) {
      inputRef.current.focus();
      // Don't select - user is typing char by char via appendToFilter
    }
  }, [showFilterInput]);

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
          className="bg-transparent border-none outline-none text-sm w-40 text-white placeholder-app-muted"
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
