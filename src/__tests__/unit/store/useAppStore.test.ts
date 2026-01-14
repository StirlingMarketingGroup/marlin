import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { useAppStore } from '../../../store/useAppStore';
import { invoke } from '@tauri-apps/api/core';
import { ask } from '@tauri-apps/plugin-dialog';

vi.mock('@tauri-apps/api/core');
vi.mock('@tauri-apps/plugin-dialog');

const mockInvoke = vi.mocked(invoke);
const mockAsk = vi.mocked(ask);

describe('useAppStore', () => {
  beforeEach(() => {
    // Reset store to initial state
    useAppStore.setState({
      currentPath: '/test',
      currentLocationRaw: 'file:///test',
      currentProviderCapabilities: undefined,
      globalPreferences: {
        viewMode: 'grid' as const,
        sortBy: 'name' as const,
        sortOrder: 'asc' as const,
        showHidden: false,
        foldersFirst: false,
      },
      directoryPreferences: {},
      files: [],
      selectedFiles: [],
      loading: false,
      pathHistory: ['/test'],
      historyIndex: 0,
      theme: 'system' as const,
      appIconCache: {},
      pinnedDirectories: [],
      sidebarWidth: 200,
      showSidebar: true,
      showPreviewPanel: false,
      showZoomSlider: false,
    });

    vi.clearAllMocks();

    // Mock the directory commands
    mockInvoke.mockImplementation((cmd, payload) => {
      if (cmd === 'read_directory') {
        return Promise.resolve({
          entries: [],
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
        });
      }
      // Mock streaming command - use the sessionId passed from frontend
      if (cmd === 'read_directory_streaming_command') {
        const args = payload as { path: string; sessionId: string };
        return Promise.resolve({
          sessionId: args.sessionId, // Return the same sessionId passed by frontend
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
        });
      }
      // Mock git status
      if (cmd === 'get_git_status') {
        return Promise.resolve(null);
      }
      return Promise.resolve(undefined);
    });
  });

  afterEach(() => {
    vi.clearAllTimers();
  });

  describe('toggleHiddenFiles', () => {
    it('should toggle global hidden files preference from false to true', async () => {
      const store = useAppStore.getState();
      expect(store.globalPreferences.showHidden).toBe(false);

      await store.toggleHiddenFiles();

      const updatedStore = useAppStore.getState();
      expect(updatedStore.globalPreferences.showHidden).toBe(true);
    });

    it('should toggle global hidden files preference from true to false', async () => {
      useAppStore.setState({
        globalPreferences: {
          viewMode: 'grid' as const,
          sortBy: 'name' as const,
          sortOrder: 'asc' as const,
          showHidden: true,
          foldersFirst: false,
        },
      });
      const store = useAppStore.getState();
      expect(store.globalPreferences.showHidden).toBe(true);

      await store.toggleHiddenFiles();

      const updatedStore = useAppStore.getState();
      expect(updatedStore.globalPreferences.showHidden).toBe(false);
    });

    it('should save directory-specific preference', async () => {
      await useAppStore.getState().toggleHiddenFiles();

      expect(mockInvoke).toHaveBeenCalledWith('set_dir_prefs', {
        path: '/test',
        prefs: JSON.stringify({ showHidden: true }),
      });
    });

    it('should sync native menu state', async () => {
      await useAppStore.getState().toggleHiddenFiles();

      expect(mockInvoke).toHaveBeenCalledWith('update_hidden_files_menu', {
        checked: true,
        source: 'frontend',
      });
    });

    it('should call refreshCurrentDirectoryStreaming after toggling', async () => {
      await useAppStore.getState().toggleHiddenFiles();

      expect(mockInvoke).toHaveBeenCalledWith(
        'read_directory_streaming_command',
        expect.objectContaining({ path: '/test', sessionId: expect.any(String) })
      );
    });

    it('should use directory-specific preference over global', async () => {
      useAppStore.setState({
        directoryPreferences: {
          '/test': { showHidden: true },
        },
        globalPreferences: {
          viewMode: 'grid' as const,
          sortBy: 'name' as const,
          sortOrder: 'asc' as const,
          showHidden: false,
          foldersFirst: false,
        },
      });
      await useAppStore.getState().toggleHiddenFiles();

      const updatedStore = useAppStore.getState();
      expect(updatedStore.directoryPreferences['/test']?.showHidden).toBe(false);
    });
  });

  describe('updateDirectoryPreferences', () => {
    it('should update directory-specific preferences', () => {
      const store = useAppStore.getState();

      store.updateDirectoryPreferences('/test', { showHidden: true, viewMode: 'list' });

      const updated = useAppStore.getState();
      expect(updated.directoryPreferences['/test']).toEqual({
        showHidden: true,
        viewMode: 'list',
      });
    });

    it('should merge with existing directory preferences', () => {
      useAppStore.setState({
        directoryPreferences: {
          '/test': { showHidden: true },
        },
      });

      const store = useAppStore.getState();
      store.updateDirectoryPreferences('/test', { viewMode: 'list' });

      const updated = useAppStore.getState();
      expect(updated.directoryPreferences['/test']).toEqual({
        showHidden: true,
        viewMode: 'list',
      });
    });
  });

  describe('setFiles', () => {
    it('should preserve existing image dimensions when new listing lacks them', () => {
      useAppStore.setState({
        files: [
          {
            name: 'photo.jpg',
            path: 'smb://server/share/photo.jpg',
            size: 10,
            modified: new Date().toISOString(),
            is_directory: false,
            is_hidden: false,
            is_symlink: false,
            is_git_repo: false,
            extension: 'jpg',
            image_width: 1920,
            image_height: 1080,
          },
        ],
      });

      useAppStore.getState().setFiles([
        {
          name: 'photo.jpg',
          path: 'smb://server/share/photo.jpg',
          size: 12,
          modified: new Date().toISOString(),
          is_directory: false,
          is_hidden: false,
          is_symlink: false,
          is_git_repo: false,
          extension: 'jpg',
          // No image_width/height in the refreshed listing
        },
      ]);

      const file = useAppStore
        .getState()
        .files.find((f) => f.path === 'smb://server/share/photo.jpg');
      expect(file?.image_width).toBe(1920);
      expect(file?.image_height).toBe(1080);
    });
  });

  describe('refreshCurrentDirectory', () => {
    it('should preserve existing image dimensions on refresh when backend does not return them', async () => {
      const smbPath = 'smb://server/share/folder';
      useAppStore.setState({
        currentPath: smbPath,
        currentLocationRaw: smbPath,
        files: [
          {
            name: 'photo.jpg',
            path: 'smb://server/share/folder/photo.jpg',
            size: 10,
            modified: new Date().toISOString(),
            is_directory: false,
            is_hidden: false,
            is_symlink: false,
            is_git_repo: false,
            extension: 'jpg',
            image_width: 1920,
            image_height: 1080,
          },
        ],
      });

      mockInvoke.mockImplementationOnce((cmd) => {
        if (cmd === 'read_directory') {
          return Promise.resolve({
            entries: [
              {
                name: 'photo.jpg',
                path: 'smb://server/share/folder/photo.jpg',
                size: 12,
                modified: new Date().toISOString(),
                is_directory: false,
                is_hidden: false,
                is_symlink: false,
                is_git_repo: false,
                extension: 'jpg',
                // No image_width/height
              },
            ],
            location: {
              raw: smbPath,
              scheme: 'smb',
              authority: 'server',
              path: '/share/folder',
              displayPath: smbPath,
            },
            capabilities: {
              scheme: 'smb',
              displayName: 'SMB',
              canRead: true,
              canWrite: true,
              canCreateDirectories: true,
              canDelete: true,
              canRename: true,
              canCopy: true,
              canMove: true,
              supportsWatching: false,
              requiresExplicitRefresh: false,
            },
          });
        }
        return Promise.resolve(undefined);
      });

      await useAppStore.getState().refreshCurrentDirectory();

      const file = useAppStore
        .getState()
        .files.find((f) => f.path === 'smb://server/share/folder/photo.jpg');
      expect(file?.image_width).toBe(1920);
      expect(file?.image_height).toBe(1080);
    });
  });

  describe('navigation', () => {
    it('should navigate to new path and update history', () => {
      const store = useAppStore.getState();

      store.navigateTo('/new-path');

      const updated = useAppStore.getState();
      expect(updated.currentPath).toBe('/new-path');
      expect(updated.pathHistory).toEqual(['/test', '/new-path']);
      expect(updated.historyIndex).toBe(1);
    });

    it('should add paths to history even if duplicates', () => {
      const store = useAppStore.getState();

      store.navigateTo('/test'); // Same as current path

      const updated = useAppStore.getState();
      expect(updated.pathHistory).toEqual(['/test', '/test']);
      expect(updated.historyIndex).toBe(1);
    });

    it('should truncate history when navigating from middle', () => {
      useAppStore.setState({
        pathHistory: ['/a', '/b', '/c'],
        historyIndex: 1,
        currentPath: '/b',
      });

      const store = useAppStore.getState();
      store.navigateTo('/d');

      const updated = useAppStore.getState();
      expect(updated.pathHistory).toEqual(['/a', '/b', '/d']);
      expect(updated.historyIndex).toBe(2);
    });
  });

  describe('canGoBack/canGoForward', () => {
    it('should return true when can go back', () => {
      useAppStore.setState({
        pathHistory: ['/a', '/b'],
        historyIndex: 1,
      });

      expect(useAppStore.getState().canGoBack()).toBe(true);
    });

    it('should return false when cannot go back', () => {
      useAppStore.setState({
        pathHistory: ['/a'],
        historyIndex: 0,
      });

      expect(useAppStore.getState().canGoBack()).toBe(false);
    });

    it('should return true when can go forward', () => {
      useAppStore.setState({
        pathHistory: ['/a', '/b'],
        historyIndex: 0,
      });

      expect(useAppStore.getState().canGoForward()).toBe(true);
    });

    it('should return false when cannot go forward', () => {
      useAppStore.setState({
        pathHistory: ['/a', '/b'],
        historyIndex: 1,
      });

      expect(useAppStore.getState().canGoForward()).toBe(false);
    });
  });

  describe('persistence regression tests', () => {
    it('should persist directory preferences when toggling hidden files', async () => {
      const testPath = '/specific/directory';
      useAppStore.setState({ currentPath: testPath });

      await useAppStore.getState().toggleHiddenFiles();

      expect(mockInvoke).toHaveBeenCalledWith('set_dir_prefs', {
        path: testPath,
        prefs: JSON.stringify({ showHidden: true }),
      });

      const state = useAppStore.getState();
      expect(state.directoryPreferences[testPath]?.showHidden).toBe(true);
      expect(state.globalPreferences.showHidden).toBe(true);
    });

    it('should maintain directory-specific preferences across navigation', () => {
      const store = useAppStore.getState();

      store.updateDirectoryPreferences('/dir1', { showHidden: true });
      store.updateDirectoryPreferences('/dir2', { showHidden: false });

      store.navigateTo('/dir1');
      const state1 = useAppStore.getState();
      expect(state1.directoryPreferences['/dir1']?.showHidden).toBe(true);

      store.navigateTo('/dir2');
      const state2 = useAppStore.getState();
      expect(state2.directoryPreferences['/dir1']?.showHidden).toBe(true);
      expect(state2.directoryPreferences['/dir2']?.showHidden).toBe(false);
    });

    it('should reset to clean state', () => {
      useAppStore.setState({
        directoryPreferences: {
          '/test1': { showHidden: true },
          '/test2': { showHidden: false },
        },
      });

      useAppStore.getState().resetDirectoryPreferences();

      const state = useAppStore.getState();
      expect(state.directoryPreferences).toEqual({});
    });
  });

  describe('pinned directories', () => {
    const mockPinnedDir = {
      name: 'Test Directory',
      path: '/test/directory',
      pinned_at: '2024-01-01T00:00:00.000Z',
      is_git_repo: false,
      is_symlink: false,
    };

    describe('loadPinnedDirectories', () => {
      it('should load pinned directories from backend', async () => {
        const mockPinnedDirs = [mockPinnedDir];
        mockInvoke.mockResolvedValueOnce(mockPinnedDirs);

        await useAppStore.getState().loadPinnedDirectories();

        expect(mockInvoke).toHaveBeenCalledWith('get_pinned_directories');
        expect(useAppStore.getState().pinnedDirectories).toEqual(mockPinnedDirs);
      });

      it('should handle loading errors gracefully', async () => {
        mockInvoke.mockRejectedValueOnce(new Error('Backend error'));

        await useAppStore.getState().loadPinnedDirectories();

        expect(useAppStore.getState().pinnedDirectories).toEqual([]);
      });
    });

    describe('addPinnedDirectory', () => {
      it('should add a new pinned directory', async () => {
        mockInvoke.mockResolvedValueOnce(mockPinnedDir);

        const result = await useAppStore.getState().addPinnedDirectory('/test/directory');

        expect(mockInvoke).toHaveBeenCalledWith('add_pinned_directory', {
          path: '/test/directory',
          name: undefined,
        });
        expect(result).toEqual(mockPinnedDir);
        expect(useAppStore.getState().pinnedDirectories).toContain(mockPinnedDir);
      });

      it('should add a pinned directory with custom name', async () => {
        const customPinnedDir = { ...mockPinnedDir, name: 'Custom Name' };
        mockInvoke.mockResolvedValueOnce(customPinnedDir);

        const result = await useAppStore
          .getState()
          .addPinnedDirectory('/test/directory', 'Custom Name');

        expect(mockInvoke).toHaveBeenCalledWith('add_pinned_directory', {
          path: '/test/directory',
          name: 'Custom Name',
        });
        expect(result).toEqual(customPinnedDir);
      });

      it('should throw error when backend fails', async () => {
        mockInvoke.mockRejectedValueOnce(new Error('Directory already pinned'));

        await expect(useAppStore.getState().addPinnedDirectory('/test/directory')).rejects.toThrow(
          'Directory already pinned'
        );
      });
    });

    describe('removePinnedDirectory', () => {
      it('should remove a pinned directory', async () => {
        useAppStore.setState({
          pinnedDirectories: [mockPinnedDir],
        });
        mockInvoke.mockResolvedValueOnce(true);

        const result = await useAppStore.getState().removePinnedDirectory('/test/directory');

        expect(mockInvoke).toHaveBeenCalledWith('remove_pinned_directory', {
          path: '/test/directory',
        });
        expect(result).toBe(true);
        expect(useAppStore.getState().pinnedDirectories).not.toContain(mockPinnedDir);
      });

      it('should return false when directory was not pinned', async () => {
        mockInvoke.mockResolvedValueOnce(false);

        const result = await useAppStore.getState().removePinnedDirectory('/nonexistent');

        expect(result).toBe(false);
      });

      it('should handle backend errors gracefully', async () => {
        mockInvoke.mockRejectedValueOnce(new Error('Backend error'));

        const result = await useAppStore.getState().removePinnedDirectory('/test/directory');

        expect(result).toBe(false);
      });
    });

    describe('reorderPinnedDirectories', () => {
      const mockPinnedDir2 = {
        name: 'Another Directory',
        path: '/another/directory',
        pinned_at: '2024-01-02T00:00:00.000Z',
        is_git_repo: false,
        is_symlink: false,
      };

      it('should reorder pinned directories', async () => {
        useAppStore.setState({
          pinnedDirectories: [mockPinnedDir, mockPinnedDir2],
        });
        mockInvoke.mockResolvedValueOnce(undefined);

        await useAppStore
          .getState()
          .reorderPinnedDirectories(['/another/directory', '/test/directory']);

        expect(mockInvoke).toHaveBeenCalledWith('reorder_pinned_directories', {
          paths: ['/another/directory', '/test/directory'],
        });
        expect(useAppStore.getState().pinnedDirectories).toEqual([mockPinnedDir2, mockPinnedDir]);
      });

      it('should handle missing directories in reorder list', async () => {
        useAppStore.setState({
          pinnedDirectories: [mockPinnedDir, mockPinnedDir2],
        });
        mockInvoke.mockResolvedValueOnce(undefined);

        await useAppStore.getState().reorderPinnedDirectories(['/another/directory']);

        expect(useAppStore.getState().pinnedDirectories).toEqual([mockPinnedDir2, mockPinnedDir]);
      });

      it('should throw error when backend fails', async () => {
        mockInvoke.mockRejectedValueOnce(new Error('Reorder failed'));

        await expect(
          useAppStore.getState().reorderPinnedDirectories(['/test/path'])
        ).rejects.toThrow('Reorder failed');
      });
    });
  });

  describe('trashSelected', () => {
    const mockFile = {
      path: '/test/file.txt',
      name: 'file.txt',
      is_directory: false,
      size: 100,
      modified: '2024-01-01T00:00:00.000Z',
      extension: 'txt',
      is_hidden: false,
      is_symlink: false,
      is_git_repo: false,
    };

    beforeEach(() => {
      useAppStore.setState({
        files: [mockFile],
        selectedFiles: ['/test/file.txt'],
      });
    });

    it('should trash selected files and clear selection', async () => {
      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'trash_paths') {
          return Promise.resolve({
            trashed: ['/test/file.txt'],
            undoToken: 'test-token',
            fallbackToPermanent: false,
          });
        }
        if (cmd === 'read_directory') {
          return Promise.resolve([]);
        }
        return Promise.resolve(undefined);
      });

      await useAppStore.getState().trashSelected();

      expect(mockInvoke).toHaveBeenCalledWith('trash_paths', {
        paths: ['/test/file.txt'],
      });
      expect(useAppStore.getState().selectedFiles).toEqual([]);
    });

    it('should do nothing when no files selected', async () => {
      useAppStore.setState({ selectedFiles: [] });

      await useAppStore.getState().trashSelected();

      expect(mockInvoke).not.toHaveBeenCalledWith('trash_paths', expect.anything());
    });

    it('should prompt for permanent delete when trash fails with permission error', async () => {
      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'trash_paths') {
          return Promise.resolve({
            trashed: [],
            undoToken: null,
            fallbackToPermanent: true,
          });
        }
        if (cmd === 'delete_paths_permanently') {
          return Promise.resolve({ deleted: ['/test/file.txt'] });
        }
        if (cmd === 'read_directory') {
          return Promise.resolve([]);
        }
        return Promise.resolve(undefined);
      });
      mockAsk.mockResolvedValue(true);

      await useAppStore.getState().trashSelected();

      expect(mockAsk).toHaveBeenCalled();
      expect(mockInvoke).toHaveBeenCalledWith('delete_paths_permanently', expect.anything());
    });
  });

  describe('deleteSelectedPermanently', () => {
    const mockFile = {
      path: '/test/file.txt',
      name: 'file.txt',
      is_directory: false,
      size: 100,
      modified: '2024-01-01T00:00:00.000Z',
      extension: 'txt',
      is_hidden: false,
      is_symlink: false,
      is_git_repo: false,
    };

    beforeEach(() => {
      useAppStore.setState({
        files: [mockFile],
        selectedFiles: ['/test/file.txt'],
      });
      mockAsk.mockResolvedValue(true);
    });

    it('should delete selected files permanently after confirmation', async () => {
      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'delete_paths_permanently') {
          return Promise.resolve({ deleted: ['/test/file.txt'] });
        }
        if (cmd === 'read_directory') {
          return Promise.resolve([]);
        }
        return Promise.resolve(undefined);
      });

      await useAppStore.getState().deleteSelectedPermanently();

      expect(mockAsk).toHaveBeenCalledWith(
        'Permanently delete file.txt? This action cannot be undone.',
        expect.objectContaining({
          title: 'Delete Permanently',
          kind: 'warning',
        })
      );
      expect(mockInvoke).toHaveBeenCalledWith('delete_paths_permanently', expect.anything());
      expect(useAppStore.getState().selectedFiles).toEqual([]);
    });

    it('should not delete when user cancels confirmation', async () => {
      mockAsk.mockResolvedValue(false);

      await useAppStore.getState().deleteSelectedPermanently();

      expect(mockInvoke).not.toHaveBeenCalledWith('delete_paths_permanently', expect.anything());
      expect(useAppStore.getState().selectedFiles).toEqual(['/test/file.txt']);
    });

    it('should do nothing when no files selected', async () => {
      useAppStore.setState({ selectedFiles: [] });

      await useAppStore.getState().deleteSelectedPermanently();

      expect(mockAsk).not.toHaveBeenCalled();
      expect(mockInvoke).not.toHaveBeenCalledWith('delete_paths_permanently', expect.anything());
    });

    it('should handle multiple files in confirmation message', async () => {
      const mockFile2 = { ...mockFile, path: '/test/file2.txt', name: 'file2.txt' };
      useAppStore.setState({
        files: [mockFile, mockFile2],
        selectedFiles: ['/test/file.txt', '/test/file2.txt'],
      });
      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'delete_paths_permanently') {
          return Promise.resolve({ deleted: ['/test/file.txt', '/test/file2.txt'] });
        }
        if (cmd === 'read_directory') {
          return Promise.resolve([]);
        }
        return Promise.resolve(undefined);
      });

      await useAppStore.getState().deleteSelectedPermanently();

      expect(mockAsk).toHaveBeenCalledWith(
        'Permanently delete 2 items? This action cannot be undone.',
        expect.anything()
      );
    });
  });

  describe('streaming', () => {
    const mockFile1 = {
      path: '/test/file1.txt',
      name: 'file1.txt',
      is_directory: false,
      size: 100,
      modified: '2024-01-01T00:00:00.000Z',
      extension: 'txt',
      is_hidden: false,
      is_symlink: false,
      is_git_repo: false,
    };

    const mockFile2 = {
      path: '/test/file2.txt',
      name: 'file2.txt',
      is_directory: false,
      size: 200,
      modified: '2024-01-02T00:00:00.000Z',
      extension: 'txt',
      is_hidden: false,
      is_symlink: false,
      is_git_repo: false,
    };

    beforeEach(() => {
      useAppStore.setState({
        files: [],
        streamingSessionId: null,
        streamingTotalCount: null,
        isStreamingComplete: true,
        loading: false,
      });
    });

    describe('appendStreamingBatch', () => {
      it('should append files from a batch with matching session ID', () => {
        useAppStore.setState({
          streamingSessionId: 'session-1',
          files: [mockFile1],
          isStreamingComplete: false,
        });

        useAppStore.getState().appendStreamingBatch({
          sessionId: 'session-1',
          batchIndex: 1,
          entries: [mockFile2],
          isFinal: false,
          totalCount: 100,
        });

        const state = useAppStore.getState();
        expect(state.files).toHaveLength(2);
        expect(state.files[1]).toEqual(mockFile2);
        expect(state.streamingTotalCount).toBe(100);
        expect(state.isStreamingComplete).toBe(false);
      });

      it('should ignore batches from different session IDs', () => {
        useAppStore.setState({
          streamingSessionId: 'session-1',
          files: [mockFile1],
          isStreamingComplete: false,
        });

        useAppStore.getState().appendStreamingBatch({
          sessionId: 'session-2',
          batchIndex: 0,
          entries: [mockFile2],
          isFinal: false,
          totalCount: 50,
        });

        const state = useAppStore.getState();
        expect(state.files).toHaveLength(1);
        expect(state.files[0]).toEqual(mockFile1);
      });

      it('should set isStreamingComplete to true on final batch', () => {
        useAppStore.setState({
          streamingSessionId: 'session-1',
          files: [mockFile1],
          isStreamingComplete: false,
          loading: true,
        });

        useAppStore.getState().appendStreamingBatch({
          sessionId: 'session-1',
          batchIndex: 1,
          entries: [mockFile2],
          isFinal: true,
          totalCount: 2,
        });

        const state = useAppStore.getState();
        expect(state.isStreamingComplete).toBe(true);
        expect(state.loading).toBe(false);
      });

      it('should preserve existing totalCount if batch does not provide one', () => {
        useAppStore.setState({
          streamingSessionId: 'session-1',
          files: [],
          streamingTotalCount: 100,
          isStreamingComplete: false,
        });

        useAppStore.getState().appendStreamingBatch({
          sessionId: 'session-1',
          batchIndex: 0,
          entries: [mockFile1],
          isFinal: false,
          totalCount: null,
        });

        const state = useAppStore.getState();
        expect(state.streamingTotalCount).toBe(100);
      });
    });

    describe('cancelDirectoryStream', () => {
      it('should cancel active streaming session', async () => {
        useAppStore.setState({
          streamingSessionId: 'session-1',
          isStreamingComplete: false,
        });

        mockInvoke.mockResolvedValueOnce(undefined);

        await useAppStore.getState().cancelDirectoryStream();

        expect(mockInvoke).toHaveBeenCalledWith('cancel_directory_stream', {
          sessionId: 'session-1',
        });
        expect(useAppStore.getState().streamingSessionId).toBeNull();
        expect(useAppStore.getState().isStreamingComplete).toBe(true);
      });

      it('should do nothing when no active streaming session', async () => {
        useAppStore.setState({
          streamingSessionId: null,
          isStreamingComplete: true,
        });

        await useAppStore.getState().cancelDirectoryStream();

        expect(mockInvoke).not.toHaveBeenCalledWith('cancel_directory_stream', expect.anything());
      });
    });

    describe('refreshCurrentDirectoryStreaming', () => {
      it('should start streaming and set initial state', async () => {
        mockInvoke.mockImplementation((cmd, payload) => {
          if (cmd === 'read_directory_streaming_command') {
            const args = payload as { path: string; sessionId: string };
            return Promise.resolve({
              sessionId: args.sessionId, // Return the frontend-generated sessionId
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
            });
          }
          // Mock git status refresh
          if (cmd === 'get_git_status') {
            return Promise.resolve(null);
          }
          return Promise.resolve(undefined);
        });

        await useAppStore.getState().refreshCurrentDirectoryStreaming();

        const state = useAppStore.getState();
        expect(state.streamingSessionId).toBeTruthy(); // Frontend generates the sessionId
        expect(state.files).toEqual([]);
        expect(state.isStreamingComplete).toBe(false);
      });

      it('should cancel previous streaming session before starting new one', async () => {
        useAppStore.setState({
          streamingSessionId: 'old-session',
          isStreamingComplete: false,
        });

        mockInvoke.mockImplementation((cmd, payload) => {
          if (cmd === 'cancel_directory_stream') {
            return Promise.resolve(undefined);
          }
          if (cmd === 'read_directory_streaming_command') {
            const args = payload as { path: string; sessionId: string };
            return Promise.resolve({
              sessionId: args.sessionId, // Return the frontend-generated sessionId
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
            });
          }
          // Mock git status refresh
          if (cmd === 'get_git_status') {
            return Promise.resolve(null);
          }
          return Promise.resolve(undefined);
        });

        await useAppStore.getState().refreshCurrentDirectoryStreaming();

        expect(mockInvoke).toHaveBeenCalledWith('cancel_directory_stream', {
          sessionId: 'old-session',
        });
        // Session ID is generated by frontend, just verify it changed
        const newSessionId = useAppStore.getState().streamingSessionId;
        expect(newSessionId).toBeTruthy();
        expect(newSessionId).not.toBe('old-session');
      });
    });
  });
});
