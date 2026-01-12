# Type-to-Filter Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable instant filtering of the current directory by typing, with a small input appearing in the bottom-right corner and matching text highlighted in filenames.

**Architecture:** Filter state lives in Zustand store. Keyboard events in App.tsx detect typing and update filter. FileGrid/FileList apply filter before rendering and pass highlight text to FileNameDisplay. A FilterInput component (similar to ZoomSlider) shows the current filter with clear/escape support.

**Tech Stack:** React, Zustand, Tailwind CSS, existing component patterns

---

## Task 1: Add Filter State to Store

**Files:**

- Modify: `src/store/useAppStore.ts`
- Test: `src/store/__tests__/useAppStore.filter.test.ts`

**Step 1: Write the failing test**

Create `src/store/__tests__/useAppStore.filter.test.ts`:

```typescript
import { describe, it, expect, beforeEach } from 'vitest';
import { useAppStore } from '../useAppStore';

describe('useAppStore filter state', () => {
  beforeEach(() => {
    useAppStore.setState({
      filterText: '',
      showFilterInput: false,
    });
  });

  it('should have empty filter text by default', () => {
    const state = useAppStore.getState();
    expect(state.filterText).toBe('');
  });

  it('should have showFilterInput false by default', () => {
    const state = useAppStore.getState();
    expect(state.showFilterInput).toBe(false);
  });

  it('should update filter text via setFilterText', () => {
    const { setFilterText } = useAppStore.getState();
    setFilterText('test');
    expect(useAppStore.getState().filterText).toBe('test');
  });

  it('should show filter input when filter text is set', () => {
    const { setFilterText } = useAppStore.getState();
    setFilterText('abc');
    expect(useAppStore.getState().showFilterInput).toBe(true);
  });

  it('should hide filter input when filter text is cleared', () => {
    const { setFilterText, clearFilter } = useAppStore.getState();
    setFilterText('test');
    clearFilter();
    expect(useAppStore.getState().filterText).toBe('');
    expect(useAppStore.getState().showFilterInput).toBe(false);
  });

  it('should clear filter when navigating to new directory', () => {
    useAppStore.setState({ filterText: 'test', showFilterInput: true });
    const { navigateTo } = useAppStore.getState();
    navigateTo('/some/new/path');
    expect(useAppStore.getState().filterText).toBe('');
    expect(useAppStore.getState().showFilterInput).toBe(false);
  });
});
```

**Step 2: Run test to verify it fails**

Run: `npm test -- src/store/__tests__/useAppStore.filter.test.ts`
Expected: FAIL with "filterText" property not found

**Step 3: Add filter state and actions to store**

In `src/store/useAppStore.ts`, add to the state interface (around line 160):

```typescript
// Add to AppState interface
filterText: string;
showFilterInput: boolean;
```

Add to initial state (around line 262):

```typescript
filterText: '',
showFilterInput: false,
```

Add actions (after the zoom slider actions, around line 380):

```typescript
setFilterText: (text: string) =>
  set({
    filterText: text,
    showFilterInput: text.length > 0,
  }),

clearFilter: () =>
  set({
    filterText: '',
    showFilterInput: false,
  }),
```

Modify `navigateTo` action to clear filter (find the navigateTo action and add to its set call):

```typescript
filterText: '',
showFilterInput: false,
```

**Step 4: Run test to verify it passes**

Run: `npm test -- src/store/__tests__/useAppStore.filter.test.ts`
Expected: PASS

**Step 5: Commit**

```bash
git add src/store/useAppStore.ts src/store/__tests__/useAppStore.filter.test.ts
git commit -m "feat: add filter state to store"
```

---

## Task 2: Create FilterInput Component

**Files:**

- Create: `src/components/FilterInput.tsx`
- Test: `src/components/__tests__/FilterInput.test.tsx`

**Step 1: Write the failing test**

Create `src/components/__tests__/FilterInput.test.tsx`:

```typescript
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import FilterInput from '../FilterInput';
import { useAppStore } from '@/store/useAppStore';

// Mock the store
vi.mock('@/store/useAppStore', () => ({
  useAppStore: vi.fn(),
}));

describe('FilterInput', () => {
  const mockSetFilterText = vi.fn();
  const mockClearFilter = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('should not render when showFilterInput is false', () => {
    (useAppStore as unknown as ReturnType<typeof vi.fn>).mockReturnValue({
      filterText: '',
      showFilterInput: false,
      setFilterText: mockSetFilterText,
      clearFilter: mockClearFilter,
    });

    const { container } = render(<FilterInput />);
    expect(container.firstChild).toBeNull();
  });

  it('should render when showFilterInput is true', () => {
    (useAppStore as unknown as ReturnType<typeof vi.fn>).mockReturnValue({
      filterText: 'test',
      showFilterInput: true,
      setFilterText: mockSetFilterText,
      clearFilter: mockClearFilter,
    });

    render(<FilterInput />);
    expect(screen.getByTestId('filter-input')).toBeInTheDocument();
  });

  it('should display the current filter text', () => {
    (useAppStore as unknown as ReturnType<typeof vi.fn>).mockReturnValue({
      filterText: 'hello',
      showFilterInput: true,
      setFilterText: mockSetFilterText,
      clearFilter: mockClearFilter,
    });

    render(<FilterInput />);
    const input = screen.getByTestId('filter-input') as HTMLInputElement;
    expect(input.value).toBe('hello');
  });

  it('should call setFilterText when typing', () => {
    (useAppStore as unknown as ReturnType<typeof vi.fn>).mockReturnValue({
      filterText: '',
      showFilterInput: true,
      setFilterText: mockSetFilterText,
      clearFilter: mockClearFilter,
    });

    render(<FilterInput />);
    const input = screen.getByTestId('filter-input');
    fireEvent.change(input, { target: { value: 'new' } });
    expect(mockSetFilterText).toHaveBeenCalledWith('new');
  });

  it('should call clearFilter when clear button is clicked', () => {
    (useAppStore as unknown as ReturnType<typeof vi.fn>).mockReturnValue({
      filterText: 'test',
      showFilterInput: true,
      setFilterText: mockSetFilterText,
      clearFilter: mockClearFilter,
    });

    render(<FilterInput />);
    const clearButton = screen.getByLabelText('Clear filter');
    fireEvent.click(clearButton);
    expect(mockClearFilter).toHaveBeenCalled();
  });

  it('should call clearFilter when Escape is pressed', () => {
    (useAppStore as unknown as ReturnType<typeof vi.fn>).mockReturnValue({
      filterText: 'test',
      showFilterInput: true,
      setFilterText: mockSetFilterText,
      clearFilter: mockClearFilter,
    });

    render(<FilterInput />);
    const input = screen.getByTestId('filter-input');
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(mockClearFilter).toHaveBeenCalled();
  });
});
```

**Step 2: Run test to verify it fails**

Run: `npm test -- src/components/__tests__/FilterInput.test.tsx`
Expected: FAIL with module not found

**Step 3: Create FilterInput component**

Create `src/components/FilterInput.tsx`:

```typescript
import { useRef, useEffect } from 'react';
import { X, MagnifyingGlass } from 'phosphor-react';
import { useAppStore } from '@/store/useAppStore';

export default function FilterInput() {
  const { filterText, showFilterInput, setFilterText, clearFilter } = useAppStore();
  const inputRef = useRef<HTMLInputElement>(null);

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
    <div
      className="fixed bottom-4 right-4 z-50 select-none"
      data-tauri-drag-region={false}
    >
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
```

**Step 4: Run test to verify it passes**

Run: `npm test -- src/components/__tests__/FilterInput.test.tsx`
Expected: PASS

**Step 5: Commit**

```bash
git add src/components/FilterInput.tsx src/components/__tests__/FilterInput.test.tsx
git commit -m "feat: add FilterInput component"
```

---

## Task 3: Add FilterInput to App Layout

**Files:**

- Modify: `src/App.tsx`

**Step 1: Import and render FilterInput**

In `src/App.tsx`, add import near the top with other component imports:

```typescript
import FilterInput from './components/FilterInput';
```

Find where ZoomSlider is rendered (around line 1280) and add FilterInput nearby:

```tsx
<ZoomSlider visible={showZoomSlider && effectivePrefs.viewMode === 'grid'} />
<FilterInput />
```

**Step 2: Verify visually**

Run: `npm run tauri dev`
Expected: No visible change yet (filter not active)

**Step 3: Commit**

```bash
git add src/App.tsx
git commit -m "feat: render FilterInput in App layout"
```

---

## Task 4: Add Type-to-Filter Keyboard Handling

**Files:**

- Modify: `src/App.tsx`
- Modify: `src/store/useAppStore.ts`

**Step 1: Add appendToFilter action to store**

In `src/store/useAppStore.ts`, add action after setFilterText:

```typescript
appendToFilter: (char: string) =>
  set((state) => ({
    filterText: state.filterText + char,
    showFilterInput: true,
  })),
```

**Step 2: Add keyboard handler for typing**

In `src/App.tsx`, find the `onKey` function (around line 805). At the beginning of the function, after the `inEditable` check (around line 812), add:

```typescript
// Type-to-filter: single printable characters start/append to filter
if (
  !inEditable &&
  e.key.length === 1 &&
  !e.metaKey &&
  !e.ctrlKey &&
  !e.altKey &&
  /^[a-zA-Z0-9\s._-]$/.test(e.key)
) {
  e.preventDefault();
  useAppStore.getState().appendToFilter(e.key);
  return;
}

// Escape clears filter if active
if (e.key === 'Escape' && useAppStore.getState().showFilterInput) {
  e.preventDefault();
  useAppStore.getState().clearFilter();
  return;
}

// Backspace removes last char from filter if active
if (e.key === 'Backspace' && !inEditable && useAppStore.getState().showFilterInput) {
  e.preventDefault();
  const current = useAppStore.getState().filterText;
  if (current.length > 1) {
    useAppStore.getState().setFilterText(current.slice(0, -1));
  } else {
    useAppStore.getState().clearFilter();
  }
  return;
}
```

**Step 3: Test manually**

Run: `npm run tauri dev`
Expected: Typing letters shows FilterInput in bottom-right, Escape clears it

**Step 4: Commit**

```bash
git add src/App.tsx src/store/useAppStore.ts
git commit -m "feat: add type-to-filter keyboard handling"
```

---

## Task 5: Apply Filter to FileGrid

**Files:**

- Modify: `src/components/FileGrid.tsx`

**Step 1: Import filterText from store**

In `src/components/FileGrid.tsx`, find the useAppStore destructuring (around line 224) and add `filterText`:

```typescript
const {
  selectedFiles,
  setSelectedFiles,
  renameTargetPath,
  setRenameTarget,
  renameFile,
  filterText, // Add this
} = useAppStore();
```

**Step 2: Apply filter to files**

Find where `filteredFiles` is defined (around line 630-632). Replace it with:

```typescript
const hiddenFiltered = preferences.showHidden
  ? sortedFiles
  : sortedFiles.filter((file) => !file.is_hidden);

const filteredFiles = filterText
  ? hiddenFiltered.filter((file) => file.name.toLowerCase().includes(filterText.toLowerCase()))
  : hiddenFiltered;
```

**Step 3: Test manually**

Run: `npm run tauri dev`
Expected: Typing filters visible files in grid view

**Step 4: Commit**

```bash
git add src/components/FileGrid.tsx
git commit -m "feat: apply filter to FileGrid"
```

---

## Task 6: Apply Filter to FileList

**Files:**

- Modify: `src/components/FileList.tsx`

**Step 1: Import filterText from store**

In `src/components/FileList.tsx`, find the useAppStore destructuring and add `filterText`:

```typescript
const {
  selectedFiles,
  setSelectedFiles,
  renameTargetPath,
  setRenameTarget,
  renameFile,
  filterText, // Add this
} = useAppStore();
```

**Step 2: Apply filter to files**

Find where `filteredFiles` is defined (around line 473-475). Replace it with:

```typescript
const hiddenFiltered = preferences.showHidden
  ? sortedFiles
  : sortedFiles.filter((file) => !file.is_hidden);

const filteredFiles = filterText
  ? hiddenFiltered.filter((file) => file.name.toLowerCase().includes(filterText.toLowerCase()))
  : hiddenFiltered;
```

**Step 3: Test manually**

Run: `npm run tauri dev`
Expected: Typing filters visible files in list view

**Step 4: Commit**

```bash
git add src/components/FileList.tsx
git commit -m "feat: apply filter to FileList"
```

---

## Task 7: Add Highlight Support to FileNameDisplay

**Files:**

- Modify: `src/components/FileNameDisplay.tsx`
- Test: `src/components/__tests__/FileNameDisplay.highlight.test.tsx`

**Step 1: Write the failing test**

Create `src/components/__tests__/FileNameDisplay.highlight.test.tsx`:

```typescript
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { FileNameDisplay } from '../FileNameDisplay';
import type { FileItem } from '@/types';

const mockFile: FileItem = {
  name: 'TestDocument.pdf',
  path: '/test/TestDocument.pdf',
  is_directory: false,
  is_hidden: false,
  size: 1024,
  modified: Date.now(),
};

describe('FileNameDisplay highlight', () => {
  it('should highlight matching text case-insensitively', () => {
    render(
      <FileNameDisplay
        file={mockFile}
        variant="list"
        highlightText="doc"
      />
    );

    const highlight = screen.getByTestId('highlight-match');
    expect(highlight).toBeInTheDocument();
    expect(highlight.textContent).toBe('Doc');
  });

  it('should not render highlight when no match', () => {
    render(
      <FileNameDisplay
        file={mockFile}
        variant="list"
        highlightText="xyz"
      />
    );

    expect(screen.queryByTestId('highlight-match')).not.toBeInTheDocument();
  });

  it('should not render highlight when highlightText is empty', () => {
    render(
      <FileNameDisplay
        file={mockFile}
        variant="list"
        highlightText=""
      />
    );

    expect(screen.queryByTestId('highlight-match')).not.toBeInTheDocument();
  });
});
```

**Step 2: Run test to verify it fails**

Run: `npm test -- src/components/__tests__/FileNameDisplay.highlight.test.tsx`
Expected: FAIL with highlight-match not found

**Step 3: Add highlightText prop and rendering**

In `src/components/FileNameDisplay.tsx`:

Add to interface (around line 8):

```typescript
highlightText?: string;
```

Add to function parameters (around line 18):

```typescript
highlightText = '',
```

Create a helper function before the component (around line 7):

```typescript
function highlightMatch(text: string, highlight: string): React.ReactNode {
  if (!highlight) return text;

  const lowerText = text.toLowerCase();
  const lowerHighlight = highlight.toLowerCase();
  const index = lowerText.indexOf(lowerHighlight);

  if (index === -1) return text;

  const before = text.slice(0, index);
  const match = text.slice(index, index + highlight.length);
  const after = text.slice(index + highlight.length);

  return (
    <>
      {before}
      <span
        data-testid="highlight-match"
        className="bg-yellow-500/40 text-yellow-200 rounded-sm px-0.5 -mx-0.5"
      >
        {match}
      </span>
      {after}
    </>
  );
}
```

Replace all occurrences of `{renderText}` with `{highlightMatch(renderText, highlightText)}`.

There are 4 places to update:

- Line ~213 (grid with tooltip)
- Line ~253 (grid without tooltip)
- Line ~291 (list with tooltip)
- Line ~309 (list without tooltip)

**Step 4: Run test to verify it passes**

Run: `npm test -- src/components/__tests__/FileNameDisplay.highlight.test.tsx`
Expected: PASS

**Step 5: Commit**

```bash
git add src/components/FileNameDisplay.tsx src/components/__tests__/FileNameDisplay.highlight.test.tsx
git commit -m "feat: add highlight support to FileNameDisplay"
```

---

## Task 8: Pass highlightText to FileNameDisplay in FileGrid

**Files:**

- Modify: `src/components/FileGrid.tsx`

**Step 1: Pass filterText as highlightText**

Find where FileNameDisplay is rendered (around line 896-903). Add the highlightText prop:

```tsx
<FileNameDisplay
  file={file}
  maxWidth={tile - 16}
  isSelected={isFileSelected}
  variant="grid"
  showSize={true}
  highlightText={filterText}
/>
```

**Step 2: Test manually**

Run: `npm run tauri dev`
Expected: Matching text is highlighted in grid view

**Step 3: Commit**

```bash
git add src/components/FileGrid.tsx
git commit -m "feat: pass highlightText to FileNameDisplay in FileGrid"
```

---

## Task 9: Pass highlightText to FileNameDisplay in FileList

**Files:**

- Modify: `src/components/FileList.tsx`

**Step 1: Pass filterText as highlightText**

Find where FileNameDisplay is rendered (around line 713). Add the highlightText prop:

```tsx
<FileNameDisplay
  file={file}
  isSelected={isFileSelected}
  variant="list"
  highlightText={filterText}
/>
```

**Step 2: Test manually**

Run: `npm run tauri dev`
Expected: Matching text is highlighted in list view

**Step 3: Commit**

```bash
git add src/components/FileList.tsx
git commit -m "feat: pass highlightText to FileNameDisplay in FileList"
```

---

## Task 10: Add "No matches" Empty State

**Files:**

- Modify: `src/components/FileGrid.tsx`
- Modify: `src/components/FileList.tsx`

**Step 1: Add empty state to FileGrid**

In `src/components/FileGrid.tsx`, find where the empty directory message is rendered (search for "This folder is empty"). Add a check for filter:

```tsx
{
  filteredFiles.length === 0 && filterText && (
    <div className="flex flex-col items-center justify-center h-full text-app-muted">
      <p className="text-sm">No files match "{filterText}"</p>
      <p className="text-xs mt-1">Press Escape to clear filter</p>
    </div>
  );
}
```

**Step 2: Add empty state to FileList**

In `src/components/FileList.tsx`, add the same empty state:

```tsx
{
  filteredFiles.length === 0 && filterText && (
    <div className="flex flex-col items-center justify-center h-full text-app-muted">
      <p className="text-sm">No files match "{filterText}"</p>
      <p className="text-xs mt-1">Press Escape to clear filter</p>
    </div>
  );
}
```

**Step 3: Test manually**

Run: `npm run tauri dev`
Expected: Empty state shown when filter has no matches

**Step 4: Commit**

```bash
git add src/components/FileGrid.tsx src/components/FileList.tsx
git commit -m "feat: add 'no matches' empty state for filter"
```

---

## Task 11: Show Match Count in FilterInput

**Files:**

- Modify: `src/components/FilterInput.tsx`
- Modify: `src/store/useAppStore.ts`

**Step 1: Add files to store selector in FilterInput**

In `src/components/FilterInput.tsx`, update the store usage:

```typescript
const { filterText, showFilterInput, setFilterText, clearFilter, files } = useAppStore();

const matchCount = filterText
  ? files.filter((f) => f.name.toLowerCase().includes(filterText.toLowerCase())).length
  : 0;
```

**Step 2: Display match count**

Update the component JSX to show match count after the input:

```tsx
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
```

**Step 3: Test manually**

Run: `npm run tauri dev`
Expected: FilterInput shows "X matches" count

**Step 4: Commit**

```bash
git add src/components/FilterInput.tsx
git commit -m "feat: show match count in FilterInput"
```

---

## Task 12: Run Full Build Verification

**Step 1: Run frontend build**

Run: `npm run build`
Expected: No errors or warnings

**Step 2: Run backend build**

Run: `cd src-tauri && cargo build`
Expected: Clean compilation

**Step 3: Run all tests**

Run: `npm test -- --run`
Expected: All tests pass

**Step 4: Final commit if any fixes needed**

```bash
git add -A
git commit -m "fix: address build issues"
```

---

## Summary

This plan adds type-to-filter functionality with:

- Filter state in Zustand store
- FilterInput component in bottom-right corner
- Type-to-filter keyboard handling (printable chars, Escape, Backspace)
- Filter applied to both FileGrid and FileList
- Highlighted matching text in filenames
- Match count displayed in FilterInput
- "No matches" empty state
- Filter clears on directory navigation
