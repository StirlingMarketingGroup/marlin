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
