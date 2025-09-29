import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { useAppStore } from '../../../store/useAppStore';
import { invoke } from '@tauri-apps/api/core';

vi.mock('@tauri-apps/api/core');
vi.mock('@tauri-apps/plugin-dialog');

const mockInvoke = vi.mocked(invoke);

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

    // Mock the read_directory command that refreshCurrentDirectory calls
    mockInvoke.mockImplementation((cmd) => {
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

    it('should call refreshCurrentDirectory after toggling', async () => {
      await useAppStore.getState().toggleHiddenFiles();

      expect(mockInvoke).toHaveBeenCalledWith('read_directory', { path: '/test' });
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
});
