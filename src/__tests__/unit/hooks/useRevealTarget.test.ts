import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Virtualizer } from '@tanstack/react-virtual';
import { useRevealTarget } from '@/hooks/useRevealTarget';
import { useAppStore } from '@/store/useAppStore';
import type { FileItem } from '@/types';

function fileItem(path: string): FileItem {
  const name = path.split(/[/\\]/).pop() ?? path;
  return {
    name,
    path,
    size: 1,
    modified: '2026-01-01T00:00:00.000Z',
    is_directory: true,
    is_hidden: false,
    is_symlink: false,
    is_git_repo: false,
  };
}

describe('useRevealTarget', () => {
  const scrollToIndex = vi.fn();
  const virtualizer = { scrollToIndex } as unknown as Virtualizer<HTMLDivElement, Element>;
  let container: HTMLDivElement;

  beforeEach(() => {
    scrollToIndex.mockClear();
    container = document.createElement('div');
    document.body.appendChild(container);
    useAppStore.setState({
      pendingRevealTarget: undefined,
      isMetadataComplete: true,
      selectedFiles: [],
      selectionAnchor: undefined,
      selectionLead: undefined,
    });
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
      callback(0);
      return 0;
    });
  });

  afterEach(() => {
    container.remove();
    vi.unstubAllGlobals();
  });

  function render(files: FileItem[], itemsPerRow = 1) {
    return renderHook(() =>
      useRevealTarget(files, { current: container }, virtualizer, itemsPerRow)
    );
  }

  it('selects the file and clears the target when the listing is complete', () => {
    const file = fileItem('/a/b');
    useAppStore.setState({ pendingRevealTarget: '/a/b' });

    render([file]);

    expect(useAppStore.getState().selectedFiles).toEqual(['/a/b']);
    expect(useAppStore.getState().selectionAnchor).toBe('/a/b');
    expect(useAppStore.getState().selectionLead).toBe('/a/b');
    expect(useAppStore.getState().pendingRevealTarget).toBeUndefined();
  });

  it('waits for sizes and dates, then scrolls to where the item ends up', () => {
    const target = fileItem('/dir/b');
    useAppStore.setState({ pendingRevealTarget: '/dir/b', isMetadataComplete: false });

    const { rerender } = renderHook(
      ({ files }) => useRevealTarget(files, { current: container }, virtualizer, 1),
      { initialProps: { files: [target, fileItem('/dir/a')] } }
    );

    expect(useAppStore.getState().selectedFiles).toEqual([]);
    expect(useAppStore.getState().pendingRevealTarget).toBe('/dir/b');

    // The dates arrive and the sort moves the target from the first row to the third.
    rerender({ files: [fileItem('/dir/a'), fileItem('/dir/c'), target] });
    act(() => {
      useAppStore.setState({ isMetadataComplete: true });
    });

    expect(useAppStore.getState().selectedFiles).toEqual(['/dir/b']);
    expect(useAppStore.getState().pendingRevealTarget).toBeUndefined();
    expect(scrollToIndex).toHaveBeenCalledTimes(1);
    expect(scrollToIndex).toHaveBeenCalledWith(2, { align: 'center' });
  });

  it('leaves the target pending when no file matches', () => {
    useAppStore.setState({ pendingRevealTarget: '/a/missing' });

    render([fileItem('/a/b')]);

    expect(useAppStore.getState().selectedFiles).toEqual([]);
    expect(useAppStore.getState().pendingRevealTarget).toBe('/a/missing');
    expect(scrollToIndex).not.toHaveBeenCalled();
  });

  it('matches a Windows path and selects the file path', () => {
    const file = fileItem('C:\\Users\\me\\Docs');
    useAppStore.setState({ pendingRevealTarget: 'C:/Users/me/Docs' });

    render([file]);

    expect(useAppStore.getState().selectedFiles).toEqual(['C:\\Users\\me\\Docs']);
    expect(useAppStore.getState().pendingRevealTarget).toBeUndefined();
  });

  it('scrolls the virtualizer when the row is not rendered', () => {
    const files = Array.from({ length: 6 }, (_, index) => fileItem(`/dir/item-${index}`));
    useAppStore.setState({ pendingRevealTarget: '/dir/item-5' });

    render(files, 3);

    expect(scrollToIndex).toHaveBeenCalledWith(Math.floor(5 / 3), { align: 'center' });
  });

  it('does not scroll the virtualizer when the row is rendered', () => {
    const file = fileItem('/a/b');
    const node = document.createElement('div');
    node.setAttribute('data-file-item', 'true');
    node.setAttribute('data-file-path', file.path);
    node.scrollIntoView = vi.fn();
    container.appendChild(node);
    useAppStore.setState({ pendingRevealTarget: file.path });

    render([file], 3);

    expect(node.scrollIntoView).toHaveBeenCalledWith({ block: 'nearest', inline: 'nearest' });
    expect(scrollToIndex).not.toHaveBeenCalled();
  });
});
