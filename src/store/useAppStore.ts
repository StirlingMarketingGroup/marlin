import { create } from 'zustand'
import { FileItem, ViewPreferences, Theme } from '../types'
import { invoke } from '@tauri-apps/api/core'
import { message } from '@tauri-apps/plugin-dialog'

// Concurrency limiter for app icon generation requests (macOS)
let __iconQueue: Array<() => void> = []
let __iconActive = 0
const __ICON_MAX = 4
const __pumpIconQueue = () => {
  while (__iconActive < __ICON_MAX && __iconQueue.length) {
    const fn = __iconQueue.shift()
    if (fn) fn()
  }
}
const __scheduleIconTask = <T,>(task: () => Promise<T>): Promise<T> =>
  new Promise((resolve, reject) => {
    const run = async () => {
      __iconActive++
      try {
        const result = await task()
        resolve(result)
      } catch (e) {
        reject(e)
      } finally {
        __iconActive--
        __pumpIconQueue()
      }
    }
    __iconQueue.push(run)
    __pumpIconQueue()
  })

interface AppState {
  // Navigation
  currentPath: string
  pathHistory: string[]
  historyIndex: number
  homeDir?: string
  
  // Files
  files: FileItem[]
  selectedFiles: string[]
  selectionAnchor?: string
  selectionLead?: string
  shiftBaseSelection?: string[] | null
  loading: boolean
  error?: string
  
  // Preferences
  globalPreferences: ViewPreferences
  directoryPreferences: Record<string, Partial<ViewPreferences>>
  theme: Theme
  
  // App icon cache for macOS Applications view
  appIconCache: Record<string, string>
  
  // UI State
  sidebarWidth: number
  showSidebar: boolean
  showPreviewPanel: boolean
  // UI ephemeral
  showZoomSlider: boolean
  _zoomSliderHideTimer?: number
  
  // Actions
  setCurrentPath: (path: string) => void
  setHomeDir: (path: string) => void
  setFiles: (files: FileItem[]) => void
  setLoading: (loading: boolean) => void
  setError: (error?: string) => void
  setSelectedFiles: (files: string[]) => void
  setSelectionAnchor: (path?: string) => void
  setSelectionLead: (path?: string) => void
  setShiftBaseSelection: (paths: string[] | null) => void
  updateGlobalPreferences: (preferences: Partial<ViewPreferences>) => void
  updateDirectoryPreferences: (path: string, preferences: Partial<ViewPreferences>) => void
  setTheme: (theme: Theme) => void
  setSidebarWidth: (width: number) => void
  toggleSidebar: () => void
  togglePreviewPanel: () => void
  showZoomSliderNow: () => void
  hideZoomSliderNow: () => void
  scheduleHideZoomSlider: (delayMs?: number) => void
  navigateTo: (path: string) => void
  goBack: () => void
  goForward: () => void
  canGoBack: () => boolean
  canGoForward: () => boolean
  goUp: () => void
  canGoUp: () => boolean
  toggleHiddenFiles: () => Promise<void>
  toggleFoldersFirst: () => Promise<void>
  refreshCurrentDirectory: () => Promise<void>
  fetchAppIcon: (path: string, size?: number) => Promise<string | undefined>
  resetDirectoryPreferences: () => void
  // Rename UX
  renameTargetPath?: string
  setRenameTarget: (path?: string) => void
  beginRenameSelected: () => void
  renameFile: (newName: string) => Promise<void>
}

export const useAppStore = create<AppState>((set, get) => ({
  // Helpers
  // Normalize paths for consistent per-directory keys
  // - converts backslashes to slashes
  // - collapses duplicate slashes
  // - trims trailing slash except root
  // - ensures Windows drive roots like C: become C:/
  _normalizePath: (p: string): string => {
    let s = p || '/'
    s = s.replace(/\\/g, '/')
    s = s.replace(/\/+/, '/')
    s = s.replace(/\/+/, '/')
    if (/^[A-Za-z]:$/.test(s)) s = s + '/'
    if (s.length > 1 && s.endsWith('/')) s = s.slice(0, -1)
    if (!s) s = '/'
    return s
  },
  // Initial state
  currentPath: '/', // Will be replaced at init
  pathHistory: ['/'],
  historyIndex: 0,
  homeDir: undefined,
  files: [],
  selectedFiles: [],
  selectionAnchor: undefined,
  selectionLead: undefined,
  shiftBaseSelection: null,
  loading: false,
  error: undefined,
  
  globalPreferences: {
    viewMode: 'list',
    sortBy: 'name',
    sortOrder: 'asc',
    showHidden: false,
    foldersFirst: true,
    gridSize: 120,
  },
  directoryPreferences: {},
  theme: 'system',
  appIconCache: {},
  
  sidebarWidth: 240,
  showSidebar: true,
  showPreviewPanel: false,
  showZoomSlider: false,
  _zoomSliderHideTimer: undefined,
  
  // Actions
  setCurrentPath: (path) => set({ currentPath: (get() as any)._normalizePath(path) }),
  setHomeDir: (path) => set({ homeDir: path }),
  setFiles: (files) => set({ files }),
  setLoading: (loading) => set({ loading }),
  setError: (error) => set({ error }),
  setSelectedFiles: (files) => {
    set({ selectedFiles: files })
    // Update native menu selection state (ignore errors in dev)
    ;(async () => {
      try {
        await invoke('update_selection_menu_state', { hasSelection: Array.isArray(files) && files.length > 0 })
      } catch { /* ignore */ }
    })()
  },
  setSelectionAnchor: (path?: string) => set({ selectionAnchor: path }),
  setSelectionLead: (path?: string) => set({ selectionLead: path }),
  setShiftBaseSelection: (paths: string[] | null) => set({ shiftBaseSelection: paths }),
  
  updateGlobalPreferences: (preferences) =>
    set((state) => ({
      globalPreferences: { ...state.globalPreferences, ...preferences },
    })),
    
  updateDirectoryPreferences: (path, preferences) =>
    set((state) => {
      const norm = (get() as any)._normalizePath(path)
      return {
        directoryPreferences: {
          ...state.directoryPreferences,
          [norm]: { ...state.directoryPreferences[norm], ...preferences },
        },
      }
    }),
    
  setTheme: (theme) => set({ theme }),
  setSidebarWidth: (width) => set({ sidebarWidth: Math.max(200, Math.min(400, width)) }),
  toggleSidebar: () => set((state) => ({ showSidebar: !state.showSidebar })),
  togglePreviewPanel: () => set((state) => ({ showPreviewPanel: !state.showPreviewPanel })),
  showZoomSliderNow: () => set((state) => {
    if (state._zoomSliderHideTimer) {
      window.clearTimeout(state._zoomSliderHideTimer)
    }
    return { showZoomSlider: true, _zoomSliderHideTimer: undefined }
  }),
  hideZoomSliderNow: () => set((state) => {
    if (state._zoomSliderHideTimer) {
      window.clearTimeout(state._zoomSliderHideTimer)
    }
    return { showZoomSlider: false, _zoomSliderHideTimer: undefined }
  }),
  scheduleHideZoomSlider: (delayMs = 300) => set((state) => {
    if (state._zoomSliderHideTimer) {
      window.clearTimeout(state._zoomSliderHideTimer)
    }
    const id = window.setTimeout(() => {
      useAppStore.getState().hideZoomSliderNow()
    }, delayMs)
    return { _zoomSliderHideTimer: id }
  }),
  
  navigateTo: (path) => {
    const { pathHistory, historyIndex } = get()
    const norm = (get() as any)._normalizePath(path)
    const newHistory = [...pathHistory.slice(0, historyIndex + 1), norm]
    set({
      currentPath: norm,
      pathHistory: newHistory,
      historyIndex: newHistory.length - 1,
    })
  },
  
  goBack: () => {
    const { pathHistory, historyIndex } = get()
    if (historyIndex > 0) {
      const newIndex = historyIndex - 1
      set({
        currentPath: pathHistory[newIndex],
        historyIndex: newIndex,
      })
    }
  },
  
  goForward: () => {
    const { pathHistory, historyIndex } = get()
    if (historyIndex < pathHistory.length - 1) {
      const newIndex = historyIndex + 1
      set({
        currentPath: pathHistory[newIndex],
        historyIndex: newIndex,
      })
    }
  },
  
  canGoBack: () => {
    const { historyIndex } = get()
    return historyIndex > 0
  },
  
  canGoForward: () => {
    const { pathHistory, historyIndex } = get()
    return historyIndex < pathHistory.length - 1
  },
  
  goUp: () => {
    const { currentPath, navigateTo } = get()
    // Handle POSIX and basic Windows paths
    if (!currentPath || currentPath === '/') return
    // Normalize backslashes to slashes for finding parent
    const normalized = currentPath.replace(/\\/g, '/').replace(/\/+$/g, '') || '/'
    // If after trimming it's root, nothing to do
    if (normalized === '/') return
    // Windows drive root like C:/
    const driveRootMatch = normalized.match(/^([A-Za-z]:)(\/$)?$/)
    if (driveRootMatch) return
    const lastSlash = normalized.lastIndexOf('/')
    const parent = lastSlash <= 0 ? '/' : normalized.slice(0, lastSlash) || '/'
    navigateTo(parent)
  },
  
  canGoUp: () => {
    const { currentPath } = get()
    if (!currentPath || currentPath === '/') return false
    const normalized = currentPath.replace(/\\/g, '/').replace(/\/+$/g, '') || '/'
    if (normalized === '/') return false
    const driveRootMatch = normalized.match(/^([A-Za-z]:)(\/$)?$/)
    if (driveRootMatch) return false
    return true
  },

  toggleHiddenFiles: async () => {
    const { currentPath } = get()
    
    // Get current state fresh each time to avoid stale closure values
    const getCurrentState = () => {
      const state = get()
      return {
        directoryPrefs: state.directoryPreferences[currentPath] || {},
        globalPrefs: state.globalPreferences,
        currentShowHidden: state.directoryPreferences[currentPath]?.showHidden ?? state.globalPreferences.showHidden
      }
    }
    
    const { currentShowHidden } = getCurrentState()
    const newShowHidden = !currentShowHidden
    
    // Update directory preference first
    get().updateDirectoryPreferences(currentPath, { showHidden: newShowHidden })
    
    // Also update global preference as default for new directories
    get().updateGlobalPreferences({ showHidden: newShowHidden })
    
    // Get fresh state after updates for saving
    const { directoryPrefs, globalPrefs } = getCurrentState()
    const updatedDirPrefs = { ...directoryPrefs, showHidden: newShowHidden }
    const updatedGlobalPrefs = { ...globalPrefs, showHidden: newShowHidden }
    
    // Save to backend with updated values
    try {
      await invoke('set_dir_prefs', { path: currentPath, prefs: JSON.stringify(updatedDirPrefs) })
      
      const freshState = get()
      await invoke('write_preferences', { json: JSON.stringify({ 
        globalPreferences: updatedGlobalPrefs,
        directoryPreferences: { ...freshState.directoryPreferences, [currentPath]: updatedDirPrefs }
      })})
    } catch (error) {
      console.warn('Failed to save preferences:', error)
    }

    // Sync native menu state
    try {
      await invoke('update_hidden_files_menu', { checked: newShowHidden, source: 'frontend' })
    } catch (error) {
      console.warn('Failed to sync menu:', error)
    }

    // Refresh directory to apply new filter
    await get().refreshCurrentDirectory()
  },

  toggleFoldersFirst: async () => {
    const { globalPreferences, updateGlobalPreferences } = get()
    const newValue = !globalPreferences.foldersFirst

    // Update preference
    updateGlobalPreferences({ foldersFirst: newValue })

    // Sync the native menu checkbox state
    try {
      await invoke('update_folders_first_menu', { checked: newValue, source: 'frontend' })
    } catch (_) {}
  },

  refreshCurrentDirectory: async () => {
    const { currentPath, setFiles, setLoading, setError } = get()
    try {
      setLoading(true)
      setError(undefined)
      const files = await invoke<any[]>('read_directory', { path: currentPath })
      setFiles(files)
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      setError(`Failed to refresh: ${msg}`)
    } finally {
      setLoading(false)
    }
  },

  fetchAppIcon: async (path: string, size = 128) => {
    const { appIconCache } = get()
    if (appIconCache[path]) return appIconCache[path]
    return __scheduleIconTask(async () => {
      try {
        const dataUrl = await invoke<string>('get_application_icon', { path, size })
        // dataUrl is already a data:image/png;base64,... string on macOS
        set((state) => ({ appIconCache: { ...state.appIconCache, [path]: dataUrl } }))
        return dataUrl
      } catch (_) {
        return undefined
      }
    })
  },

  resetDirectoryPreferences: () => {
    set({ directoryPreferences: {} })
  },
  
  // Rename state
  renameTargetPath: undefined,
  setRenameTarget: (path?: string) => set({ renameTargetPath: path }),
  beginRenameSelected: () => {
    const { selectedFiles } = get()
    if (!selectedFiles || selectedFiles.length === 0) return
    set({ renameTargetPath: selectedFiles[0] })
  },
  renameFile: async (newName: string) => {
    const state = get()
    const target = state.renameTargetPath
    if (!target) return
    const trimmed = (newName || '').trim()
    if (!trimmed) { set({ renameTargetPath: undefined }); return }
    if (/[\\/]/.test(trimmed)) {
      // Invalid characters for a single path segment
      try {
        await message('Name cannot contain slashes.', { title: 'Invalid Name', kind: 'warning', okLabel: 'OK' })
      } catch (_) {}
      return
    }

    const sep = target.includes('\\') ? '\\' : '/'
    const lastSep = Math.max(target.lastIndexOf('/'), target.lastIndexOf('\\'))
    const parent = lastSep >= 0 ? target.slice(0, lastSep) : state.currentPath
    const toPath = parent ? `${parent}${sep}${trimmed}` : trimmed
    try {
      // Tauri command args expect camelCase keys
      await invoke('rename_file', { fromPath: target, toPath })
      set({ renameTargetPath: undefined })
      state.setSelectedFiles([toPath])
      await state.refreshCurrentDirectory()
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      try {
        await message(`Failed to rename:\n${msg}`, { title: 'Rename Error', kind: 'error', okLabel: 'OK' })
      } catch (_) {
        // ignore
      }
    }
  },
}))
