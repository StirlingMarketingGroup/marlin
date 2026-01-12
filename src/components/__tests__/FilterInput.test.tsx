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
