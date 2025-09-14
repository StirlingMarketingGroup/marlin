import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { useAppStore } from '../../../store/useAppStore'
import { invoke } from '@tauri-apps/api/core'

vi.mock('@tauri-apps/api/core')
vi.mock('@tauri-apps/plugin-dialog')

const mockInvoke = vi.mocked(invoke)

describe('useAppStore', () => {
  beforeEach(() => {
    // Reset store to initial state
    useAppStore.setState({
      currentPath: '/test',
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
      sidebarWidth: 200,
      showSidebar: true,
      showPreviewPanel: false,
      showZoomSlider: false,
    })
    
    vi.clearAllMocks()
    
    // Mock the read_directory command that refreshCurrentDirectory calls
    mockInvoke.mockImplementation((cmd) => {
      if (cmd === 'read_directory') {
        return Promise.resolve([])
      }
      return Promise.resolve(undefined)
    })
  })

  afterEach(() => {
    vi.clearAllTimers()
  })

  describe('toggleHiddenFiles', () => {
    it('should toggle global hidden files preference from false to true', async () => {
      mockInvoke.mockResolvedValue(undefined)
      
      const store = useAppStore.getState()
      expect(store.globalPreferences.showHidden).toBe(false)
      
      await store.toggleHiddenFiles()
      
      const updatedStore = useAppStore.getState()
      expect(updatedStore.globalPreferences.showHidden).toBe(true)
    })

    it('should toggle global hidden files preference from true to false', async () => {
      useAppStore.setState({
        globalPreferences: {
          viewMode: 'grid' as const,
          sortBy: 'name' as const,
          sortOrder: 'asc' as const,
          showHidden: true,
          foldersFirst: false,
        }
      })
      mockInvoke.mockResolvedValue(undefined)
      
      const store = useAppStore.getState()
      expect(store.globalPreferences.showHidden).toBe(true)
      
      await store.toggleHiddenFiles()
      
      const updatedStore = useAppStore.getState()
      expect(updatedStore.globalPreferences.showHidden).toBe(false)
    })

    it('should save directory-specific preference', async () => {
      mockInvoke.mockResolvedValue(undefined)
      
      await useAppStore.getState().toggleHiddenFiles()
      
      expect(mockInvoke).toHaveBeenCalledWith('set_dir_prefs', {
        path: '/test',
        prefs: JSON.stringify({ showHidden: true })
      })
    })

    it('should sync native menu state', async () => {
      mockInvoke.mockResolvedValue(undefined)
      
      await useAppStore.getState().toggleHiddenFiles()
      
      expect(mockInvoke).toHaveBeenCalledWith('update_hidden_files_menu', {
        checked: true,
        source: 'frontend'
      })
    })

    it('should call refreshCurrentDirectory after toggling', async () => {
      mockInvoke.mockResolvedValue(undefined)
      
      await useAppStore.getState().toggleHiddenFiles()
      
      expect(mockInvoke).toHaveBeenCalledWith('read_directory', { path: '/test' })
    })

    it('should use directory-specific preference over global', async () => {
      useAppStore.setState({
        directoryPreferences: {
          '/test': { showHidden: true }
        },
        globalPreferences: {
          viewMode: 'grid' as const,
          sortBy: 'name' as const,
          sortOrder: 'asc' as const,
          showHidden: false,
          foldersFirst: false,
        }
      })
      mockInvoke.mockResolvedValue(undefined)
      
      await useAppStore.getState().toggleHiddenFiles()
      
      const updatedStore = useAppStore.getState()
      expect(updatedStore.directoryPreferences['/test']?.showHidden).toBe(false)
    })
  })

  describe('updateDirectoryPreferences', () => {
    it('should update directory-specific preferences', () => {
      const store = useAppStore.getState()
      
      store.updateDirectoryPreferences('/test', { showHidden: true, viewMode: 'list' })
      
      const updated = useAppStore.getState()
      expect(updated.directoryPreferences['/test']).toEqual({
        showHidden: true,
        viewMode: 'list'
      })
    })

    it('should merge with existing directory preferences', () => {
      useAppStore.setState({
        directoryPreferences: {
          '/test': { showHidden: true }
        }
      })
      
      const store = useAppStore.getState()
      store.updateDirectoryPreferences('/test', { viewMode: 'list' })
      
      const updated = useAppStore.getState()
      expect(updated.directoryPreferences['/test']).toEqual({
        showHidden: true,
        viewMode: 'list'
      })
    })
  })

  describe('navigation', () => {
    it('should navigate to new path and update history', () => {
      const store = useAppStore.getState()
      
      store.navigateTo('/new-path')
      
      const updated = useAppStore.getState()
      expect(updated.currentPath).toBe('/new-path')
      expect(updated.pathHistory).toEqual(['/test', '/new-path'])
      expect(updated.historyIndex).toBe(1)
    })

    it('should add paths to history even if duplicates', () => {
      const store = useAppStore.getState()
      
      store.navigateTo('/test') // Same as current path
      
      const updated = useAppStore.getState()
      expect(updated.pathHistory).toEqual(['/test', '/test'])
      expect(updated.historyIndex).toBe(1)
    })

    it('should truncate history when navigating from middle', () => {
      useAppStore.setState({
        pathHistory: ['/a', '/b', '/c'],
        historyIndex: 1,
        currentPath: '/b'
      })
      
      const store = useAppStore.getState()
      store.navigateTo('/d')
      
      const updated = useAppStore.getState()
      expect(updated.pathHistory).toEqual(['/a', '/b', '/d'])
      expect(updated.historyIndex).toBe(2)
    })
  })

  describe('canGoBack/canGoForward', () => {
    it('should return true when can go back', () => {
      useAppStore.setState({
        pathHistory: ['/a', '/b'],
        historyIndex: 1
      })
      
      expect(useAppStore.getState().canGoBack()).toBe(true)
    })

    it('should return false when cannot go back', () => {
      useAppStore.setState({
        pathHistory: ['/a'],
        historyIndex: 0
      })
      
      expect(useAppStore.getState().canGoBack()).toBe(false)
    })

    it('should return true when can go forward', () => {
      useAppStore.setState({
        pathHistory: ['/a', '/b'],
        historyIndex: 0
      })
      
      expect(useAppStore.getState().canGoForward()).toBe(true)
    })

    it('should return false when cannot go forward', () => {
      useAppStore.setState({
        pathHistory: ['/a', '/b'],
        historyIndex: 1
      })
      
      expect(useAppStore.getState().canGoForward()).toBe(false)
    })
  })

  describe('persistence regression tests', () => {
    it('should persist directory preferences when toggling hidden files', async () => {
      mockInvoke.mockResolvedValue(undefined)
      
      const testPath = '/specific/directory'
      useAppStore.setState({ currentPath: testPath })
      
      await useAppStore.getState().toggleHiddenFiles()
      
      expect(mockInvoke).toHaveBeenCalledWith('set_dir_prefs', {
        path: testPath,
        prefs: JSON.stringify({ showHidden: true })
      })
      
      const state = useAppStore.getState()
      expect(state.directoryPreferences[testPath]?.showHidden).toBe(true)
      expect(state.globalPreferences.showHidden).toBe(true)
    })

    it('should maintain directory-specific preferences across navigation', () => {
      const store = useAppStore.getState()
      
      store.updateDirectoryPreferences('/dir1', { showHidden: true })
      store.updateDirectoryPreferences('/dir2', { showHidden: false })
      
      store.navigateTo('/dir1')
      const state1 = useAppStore.getState()
      expect(state1.directoryPreferences['/dir1']?.showHidden).toBe(true)
      
      store.navigateTo('/dir2')
      const state2 = useAppStore.getState()
      expect(state2.directoryPreferences['/dir1']?.showHidden).toBe(true)
      expect(state2.directoryPreferences['/dir2']?.showHidden).toBe(false)
    })

    it('should reset to clean state', () => {
      useAppStore.setState({
        directoryPreferences: {
          '/test1': { showHidden: true },
          '/test2': { showHidden: false }
        }
      })
      
      useAppStore.getState().resetDirectoryPreferences()
      
      const state = useAppStore.getState()
      expect(state.directoryPreferences).toEqual({})
    })
  })
})