import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { FileNameDisplay } from '../FileNameDisplay';
import type { FileItem } from '@/types';

const mockFile: FileItem = {
  name: 'TestDocument.pdf',
  path: '/test/TestDocument.pdf',
  is_directory: false,
  is_hidden: false,
  is_symlink: false,
  is_git_repo: false,
  size: 1024,
  modified: new Date().toISOString(),
};

describe('FileNameDisplay highlight', () => {
  it('should highlight matching text case-insensitively', () => {
    render(<FileNameDisplay file={mockFile} variant="list" highlightText="doc" />);

    const highlight = screen.getByTestId('highlight-match');
    expect(highlight).toBeInTheDocument();
    expect(highlight.textContent).toBe('Doc');
  });

  it('should not render highlight when no match', () => {
    render(<FileNameDisplay file={mockFile} variant="list" highlightText="xyz" />);

    expect(screen.queryByTestId('highlight-match')).not.toBeInTheDocument();
  });

  it('should not render highlight when highlightText is empty', () => {
    render(<FileNameDisplay file={mockFile} variant="list" highlightText="" />);

    expect(screen.queryByTestId('highlight-match')).not.toBeInTheDocument();
  });

  it('highlights the suffix for *.pdf', () => {
    render(<FileNameDisplay file={mockFile} variant="list" highlightText="*.pdf" />);

    const highlights = screen.getAllByTestId('highlight-match');
    expect(highlights).toHaveLength(1);
    expect(highlights[0].textContent).toBe('.pdf');
  });

  it('highlights both parts of test*.pdf', () => {
    render(<FileNameDisplay file={mockFile} variant="list" highlightText="test*.pdf" />);

    const highlights = screen.getAllByTestId('highlight-match');
    expect(highlights).toHaveLength(2);
    expect(highlights[0].textContent).toBe('Test');
    expect(highlights[1].textContent).toBe('.pdf');
  });

  it('renders no highlight for *', () => {
    render(<FileNameDisplay file={mockFile} variant="list" highlightText="*" />);

    expect(screen.queryByTestId('highlight-match')).not.toBeInTheDocument();
  });

  it('highlights report when the name starts with İ', () => {
    const file: FileItem = {
      ...mockFile,
      name: 'İstanbul report.pdf',
      path: '/test/İstanbul report.pdf',
    };
    render(<FileNameDisplay file={file} variant="list" highlightText="report" />);

    expect(screen.getByTestId('highlight-match').textContent).toBe('report');
  });

  it('renders no highlight when the name as shown does not match the filter', () => {
    const userAgent = vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue('Macintosh');
    const file: FileItem = {
      ...mockFile,
      name: 'Safari.app',
      path: '/test/Safari.app',
      is_directory: true,
    };
    render(<FileNameDisplay file={file} variant="list" highlightText="*.app" />);
    userAgent.mockRestore();

    expect(screen.getByText('Safari')).toBeInTheDocument();
    expect(screen.queryByTestId('highlight-match')).not.toBeInTheDocument();
  });
});
