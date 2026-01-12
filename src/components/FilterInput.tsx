import { useRef, useEffect } from 'react';
import { X, MagnifyingGlass } from 'phosphor-react';
import { useAppStore } from '@/store/useAppStore';

export default function FilterInput() {
  const { filterText, showFilterInput, setFilterText, clearFilter } = useAppStore();
  // Only subscribe to files when we have a filter to avoid re-renders during streaming
  const files = useAppStore((state) => (state.filterText ? state.files : []));
  const inputRef = useRef<HTMLInputElement>(null);

  const matchCount = files.filter((f) =>
    f.name.toLowerCase().includes(filterText.toLowerCase())
  ).length;

  useEffect(() => {
    if (showFilterInput && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [showFilterInput]);

  if (!showFilterInput) return null;

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      clearFilter();
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
          <span className="text-xs text-app-muted whitespace-nowrap">
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
