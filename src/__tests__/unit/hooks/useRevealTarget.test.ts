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
      pendingReveal: undefined,
      currentPath: '/',
      isMetadataComplete: true,
      selectedFiles: [],
      selectionAnchor: undefined,
      selectionLead: undefined,
      files: [],
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
    useAppStore.setState({ files });
    return renderHook(() =>
      useRevealTarget(files, { current: container }, virtualizer, itemsPerRow)
    );
  }

  it('selects the file and clears the target when the listing is complete', () => {
    const file = fileItem('/a/b');
    useAppStore.setState({ pendingReveal: { paths: ['/a/b'] } });

    render([file]);

    expect(useAppStore.getState().selectedFiles).toEqual(['/a/b']);
    expect(useAppStore.getState().selectionAnchor).toBe('/a/b');
    expect(useAppStore.getState().selectionLead).toBe('/a/b');
    expect(useAppStore.getState().pendingReveal).toBeUndefined();
  });

  it('waits for sizes and dates, then scrolls to where the item ends up', () => {
    const target = fileItem('/dir/b');
    const initialFiles = [target, fileItem('/dir/a')];
    useAppStore.setState({
      pendingReveal: { paths: ['/dir/b'] },
      isMetadataComplete: false,
      files: initialFiles,
    });

    const { rerender } = renderHook(
      ({ files }) => useRevealTarget(files, { current: container }, virtualizer, 1),
      { initialProps: { files: initialFiles } }
    );

    expect(useAppStore.getState().selectedFiles).toEqual([]);
    expect(useAppStore.getState().pendingReveal).toEqual({ paths: ['/dir/b'] });

    // The dates arrive and the sort moves the target from the first row to the third.
    const nextFiles = [fileItem('/dir/a'), fileItem('/dir/c'), target];
    rerender({ files: nextFiles });
    act(() => {
      useAppStore.setState({ isMetadataComplete: true, files: nextFiles });
    });

    expect(useAppStore.getState().selectedFiles).toEqual(['/dir/b']);
    expect(useAppStore.getState().pendingReveal).toBeUndefined();
    expect(scrollToIndex).toHaveBeenCalledTimes(1);
    expect(scrollToIndex).toHaveBeenCalledWith(2, { align: 'center' });
  });

  it('leaves the target pending when no file matches', () => {
    useAppStore.setState({ pendingReveal: { paths: ['/a/missing'] } });

    render([fileItem('/a/b')]);

    expect(useAppStore.getState().selectedFiles).toEqual([]);
    expect(useAppStore.getState().pendingReveal).toEqual({ paths: ['/a/missing'] });
    expect(scrollToIndex).not.toHaveBeenCalled();
  });

  it('matches a Windows path and selects the file path', () => {
    const file = fileItem('C:\\Users\\me\\Docs');
    useAppStore.setState({ pendingReveal: { paths: ['C:/Users/me/Docs'] } });

    render([file]);

    expect(useAppStore.getState().selectedFiles).toEqual(['C:\\Users\\me\\Docs']);
    expect(useAppStore.getState().pendingReveal).toBeUndefined();
  });

  it('scrolls the virtualizer when the row is not rendered', () => {
    const files = Array.from({ length: 6 }, (_, index) => fileItem(`/dir/item-${index}`));
    useAppStore.setState({ pendingReveal: { paths: ['/dir/item-5'] } });

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
    useAppStore.setState({ pendingReveal: { paths: [file.path] } });

    render([file], 3);

    expect(node.scrollIntoView).toHaveBeenCalledWith({ block: 'nearest', inline: 'nearest' });
    expect(scrollToIndex).not.toHaveBeenCalled();
  });

  it('selects every target and scrolls to the first one the view shows', () => {
    const files = ['/dir/a', '/dir/b', '/dir/c', '/dir/d'].map(fileItem);
    useAppStore.setState({ pendingReveal: { paths: ['/dir/d', '/dir/b'] } });

    render(files);

    expect(useAppStore.getState().selectedFiles).toEqual(['/dir/b', '/dir/d']);
    expect(useAppStore.getState().selectionAnchor).toBe('/dir/b');
    expect(useAppStore.getState().selectionLead).toBe('/dir/d');
    expect(scrollToIndex).toHaveBeenCalledTimes(1);
    expect(scrollToIndex).toHaveBeenCalledWith(1, { align: 'center' });
    expect(useAppStore.getState().pendingReveal).toBeUndefined();
  });

  it('waits until the folder lists every target', () => {
    const first = fileItem('/dir/a');
    const second = fileItem('/dir/b');
    useAppStore.setState({ pendingReveal: { paths: ['/dir/a', '/dir/b'] }, files: [first] });

    const { rerender } = renderHook(
      ({ files }) => useRevealTarget(files, { current: container }, virtualizer, 1),
      { initialProps: { files: [first] } }
    );

    expect(useAppStore.getState().selectedFiles).toEqual([]);
    expect(useAppStore.getState().pendingReveal).toEqual({ paths: ['/dir/a', '/dir/b'] });

    act(() => {
      useAppStore.setState({ files: [first, second] });
      rerender({ files: [first, second] });
    });

    expect(useAppStore.getState().selectedFiles).toEqual(['/dir/a', '/dir/b']);
    expect(useAppStore.getState().pendingReveal).toBeUndefined();
  });

  it('selects only the targets the view shows', () => {
    const shown = [fileItem('/dir/a'), fileItem('/dir/b')];
    useAppStore.setState({
      pendingReveal: { paths: ['/dir/b', '/dir/.hidden'] },
      files: [...shown, fileItem('/dir/.hidden')],
    });

    renderHook(() => useRevealTarget(shown, { current: container }, virtualizer, 1));

    expect(useAppStore.getState().selectedFiles).toEqual(['/dir/b']);
    expect(useAppStore.getState().pendingReveal).toBeUndefined();
  });

  it('waits while the view shows none of the targets, then selects them once shown', () => {
    const visible = fileItem('/dir/a');
    const hidden = fileItem('/dir/.hidden');
    useAppStore.setState({
      selectedFiles: ['/dir/a'],
      pendingReveal: { paths: ['/dir/.hidden'] },
      files: [visible, hidden],
    });

    const { rerender } = renderHook(
      ({ files }) => useRevealTarget(files, { current: container }, virtualizer, 1),
      { initialProps: { files: [visible] } }
    );

    expect(useAppStore.getState().selectedFiles).toEqual(['/dir/a']);
    expect(scrollToIndex).not.toHaveBeenCalled();
    expect(useAppStore.getState().pendingReveal).toEqual({ paths: ['/dir/.hidden'] });

    // Hidden files turned on.
    rerender({ files: [hidden, visible] });

    expect(useAppStore.getState().selectedFiles).toEqual(['/dir/.hidden']);
    expect(useAppStore.getState().pendingReveal).toBeUndefined();
  });

  it('finds a file by name in the folder the listing names', () => {
    useAppStore.setState({
      currentPath: 'sftp://u@h:22/dir',
      pendingReveal: { name: 'report.txt' },
    });

    render([fileItem('sftp://u@h:22/dir/report.txt')]);

    expect(useAppStore.getState().selectedFiles).toEqual(['sftp://u@h:22/dir/report.txt']);
    expect(useAppStore.getState().pendingReveal).toBeUndefined();
  });

  it('does not match the name in the folder that was on screen before', () => {
    useAppStore.setState({
      currentPath: '/new',
      pendingReveal: { name: 'a.txt' },
    });

    render([fileItem('/old/a.txt')]);

    expect(useAppStore.getState().selectedFiles).toEqual([]);
    expect(useAppStore.getState().pendingReveal).toEqual({ name: 'a.txt' });
  });

  it('matches a name that differs only in case or Unicode form once the listing is complete', () => {
    // The typed é is one code point; the listed one is an e and a combining accent.
    useAppStore.setState({
      currentPath: '/dir',
      pendingReveal: { name: 'caf\u00e9.pdf' },
      isMetadataComplete: false,
    });

    render([fileItem('/dir/Cafe\u0301.pdf')]);

    expect(useAppStore.getState().selectedFiles).toEqual([]);
    expect(useAppStore.getState().pendingReveal).toEqual({ name: 'caf\u00e9.pdf' });

    act(() => {
      useAppStore.setState({ isMetadataComplete: true });
    });

    expect(useAppStore.getState().selectedFiles).toEqual(['/dir/Cafe\u0301.pdf']);
    expect(useAppStore.getState().pendingReveal).toBeUndefined();
  });

  it('prefers the exact name over one that differs in case', () => {
    useAppStore.setState({
      currentPath: '/dir',
      pendingReveal: { name: 'file.txt' },
    });

    render([fileItem('/dir/File.txt'), fileItem('/dir/file.txt')]);

    expect(useAppStore.getState().selectedFiles).toEqual(['/dir/file.txt']);
  });
});
