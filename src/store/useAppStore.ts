import { create } from 'zustand'
import { FileItem, ViewPreferences, Theme } from '../types'

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
  
  // Actions
  setCurrentPath: (path: string) => void
  setHomeDir: (path: string) => void
  setFiles: (files: FileItem[]) => void
  setLoading: (loading: boolean) => void
  setError: (error?: string) => void
  setSelectedFiles: (files: string[]) => void
  updateGlobalPreferences: (preferences: Partial<ViewPreferences>) => void
  updateDirectoryPreferences: (path: string, preferences: Partial<ViewPreferences>) => void
  setTheme: (theme: Theme) => void
  setSidebarWidth: (width: number) => void
  toggleSidebar: () => void
  togglePreviewPanel: () => void
  navigateTo: (path: string) => void
  goBack: () => void
  goForward: () => void
  canGoBack: () => boolean
  canGoForward: () => boolean
  goUp: () => void
  canGoUp: () => boolean
  toggleHiddenFiles: () => Promise<void>
  refreshCurrentDirectory: () => Promise<void>
  fetchAppIcon: (path: string, size?: number) => Promise<string | undefined>
}

export const useAppStore = create<AppState>((set, get) => ({
  // Initial state
  currentPath: '/', // Will be replaced at init
  pathHistory: ['/'],
  historyIndex: 0,
  homeDir: undefined,
  files: [],
  selectedFiles: [],
  loading: false,
  error: undefined,
  
  globalPreferences: {
    viewMode: 'list',
    sortBy: 'name',
    sortOrder: 'asc',
    showHidden: false,
  },
  directoryPreferences: {},
  theme: 'system',
  appIconCache: {},
  
  sidebarWidth: 240,
  showSidebar: true,
  showPreviewPanel: false,
  
  // Actions
  setCurrentPath: (path) => set({ currentPath: path }),
  setHomeDir: (path) => set({ homeDir: path }),
  setFiles: (files) => set({ files }),
  setLoading: (loading) => set({ loading }),
  setError: (error) => set({ error }),
  setSelectedFiles: (files) => set({ selectedFiles: files }),
  
  updateGlobalPreferences: (preferences) =>
    set((state) => ({
      globalPreferences: { ...state.globalPreferences, ...preferences },
    })),
    
  updateDirectoryPreferences: (path, preferences) =>
    set((state) => ({
      directoryPreferences: {
        ...state.directoryPreferences,
        [path]: { ...state.directoryPreferences[path], ...preferences },
      },
    })),
    
  setTheme: (theme) => set({ theme }),
  setSidebarWidth: (width) => set({ sidebarWidth: Math.max(200, Math.min(400, width)) }),
  toggleSidebar: () => set((state) => ({ showSidebar: !state.showSidebar })),
  togglePreviewPanel: () => set((state) => ({ showPreviewPanel: !state.showPreviewPanel })),
  
  navigateTo: (path) => {
    const { pathHistory, historyIndex } = get()
    const newHistory = [...pathHistory.slice(0, historyIndex + 1), path]
    set({
      currentPath: path,
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
    const { globalPreferences, updateGlobalPreferences, currentPath, setFiles, setLoading, setError } = get()
    const timestamp = new Date().toISOString()
    
    const newShowHidden = !globalPreferences.showHidden
    
    // Update the global preferences
    updateGlobalPreferences({ showHidden: newShowHidden })
    
    // Sync the native menu checkbox state
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('update_hidden_files_menu', { checked: newShowHidden, source: 'frontend' })
    } catch (_) {}
    
    // Reload files to apply the new filter (menu sync removed to test)
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      setLoading(true)
      setError(undefined)
      const files = await invoke<any[]>('read_directory', { path: currentPath })
      setFiles(files)
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      setError(`Failed to reload files: ${msg}`)
    } finally {
      setLoading(false)
    }
  },

  refreshCurrentDirectory: async () => {
    const { currentPath, setFiles, setLoading, setError } = get()
    try {
      const { invoke } = await import('@tauri-apps/api/core')
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
        const { invoke } = await import('@tauri-apps/api/core')
        const dataUrl = await invoke<string>('get_application_icon', { path, size })
        // dataUrl is already a data:image/png;base64,... string on macOS
        set((state) => ({ appIconCache: { ...state.appIconCache, [path]: dataUrl } }))
        return dataUrl
      } catch (_) {
        return undefined
      }
    })
  },
}))
