import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { listen, type Event } from '@tauri-apps/api/event';
import { useDirectoryWatcher } from '@/hooks/useDirectoryWatcher';
import { useAppStore } from '@/store/useAppStore';
import type { DirectoryChangeEventPayload, DirectoryListingResponse, FileItem } from '@/types';

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
vi.mock('@/hooks/useThumbnail', () => ({ invalidateThumbnailsForPaths: vi.fn() }));

const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);

const downloadedFile: FileItem = {
  name: 'download.zip',
  path: '/test/download.zip',
  size: 20,
  modified: '2024-01-01T00:00:00.000Z',
  is_directory: false,
  is_hidden: false,
  is_symlink: false,
  is_git_repo: false,
  extension: 'zip',
};

const listing: DirectoryListingResponse = {
  entries: [downloadedFile],
  location: {
    raw: 'file:///test',
    scheme: 'file',
    authority: null,
    path: '/test',
    displayPath: '/test',
  },
  capabilities: {
    scheme: 'file',
    displayName: 'Local Filesystem',
    canRead: true,
    canWrite: true,
    canCreateDirectories: true,
    canDelete: true,
    canRename: true,
    canCopy: true,
    canMove: true,
    supportsWatching: true,
    requiresExplicitRefresh: false,
  },
};

describe('useDirectoryWatcher', () => {
  let directoryChanged: ((event: Event<DirectoryChangeEventPayload>) => void) | undefined;
  const unlisten = vi.fn();

  beforeEach(() => {
    vi.useFakeTimers();
    directoryChanged = undefined;
    useAppStore.setState({
      currentPath: '/test',
      files: [],
      selectedFiles: [],
      streamingSessionId: null,
      isStreamingComplete: true,
    });

    mockListen.mockImplementation(async (_event, handler) => {
      directoryChanged = handler;
      return unlisten;
    });
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'read_directory') return listing;
      return undefined;
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it('coalesces event bursts into one authoritative reconciliation', async () => {
    const { unmount } = renderHook(() => useDirectoryWatcher('/test'));
    await act(async () => Promise.resolve());

    const event: Event<DirectoryChangeEventPayload> = {
      event: 'directory-changed',
      id: 1,
      payload: {
        path: '/test',
        changeType: 'modified',
        affectedFiles: ['download.zip'],
        affectedPaths: ['/test/download.zip'],
      },
    };

    act(() => {
      directoryChanged?.(event);
      directoryChanged?.(event);
      directoryChanged?.(event);
    });
    await act(async () => vi.advanceTimersByTimeAsync(400));

    expect(mockInvoke).toHaveBeenCalledTimes(2);
    expect(mockInvoke).toHaveBeenNthCalledWith(1, 'start_watching_directory', { path: '/test' });
    expect(mockInvoke).toHaveBeenNthCalledWith(2, 'read_directory', { path: '/test' });
    expect(useAppStore.getState().files).toEqual([downloadedFile]);

    unmount();
    expect(unlisten).toHaveBeenCalledOnce();
  });
});
