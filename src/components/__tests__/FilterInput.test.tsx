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

  // Helper to create mock that handles both bare calls and selector calls
  const setupMock = (state: {
    filterText: string;
    showFilterInput: boolean;
    files?: { name: string }[];
  }) => {
    const fullState = {
      ...state,
      files: state.files ?? [],
      setFilterText: mockSetFilterText,
      clearFilter: mockClearFilter,
    };
    (useAppStore as unknown as ReturnType<typeof vi.fn>).mockImplementation(
      (selector?: (s: typeof fullState) => unknown) => {
        if (selector) return selector(fullState);
        return fullState;
      }
    );
  };

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('should not render when showFilterInput is false', () => {
    setupMock({ filterText: '', showFilterInput: false });
    const { container } = render(<FilterInput />);
    expect(container.firstChild).toBeNull();
  });

  it('should render when showFilterInput is true', () => {
    setupMock({ filterText: 'test', showFilterInput: true });
    render(<FilterInput />);
    expect(screen.getByTestId('filter-input')).toBeInTheDocument();
  });

  it('should display the current filter text', () => {
    setupMock({ filterText: 'hello', showFilterInput: true });
    render(<FilterInput />);
    const input = screen.getByTestId('filter-input') as HTMLInputElement;
    expect(input.value).toBe('hello');
  });

  it('should call setFilterText when typing', () => {
    setupMock({ filterText: '', showFilterInput: true });
    render(<FilterInput />);
    const input = screen.getByTestId('filter-input');
    fireEvent.change(input, { target: { value: 'new' } });
    expect(mockSetFilterText).toHaveBeenCalledWith('new');
  });

  it('should call clearFilter when clear button is clicked', () => {
    setupMock({ filterText: 'test', showFilterInput: true });
    render(<FilterInput />);
    const clearButton = screen.getByLabelText('Clear filter');
    fireEvent.click(clearButton);
    expect(mockClearFilter).toHaveBeenCalled();
  });

  it('should call clearFilter when Escape is pressed', () => {
    setupMock({ filterText: 'test', showFilterInput: true });
    render(<FilterInput />);
    const input = screen.getByTestId('filter-input');
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(mockClearFilter).toHaveBeenCalled();
  });
});
