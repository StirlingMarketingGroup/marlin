import { create } from 'zustand'
import { FileItem, ViewPreferences, Theme } from '../types'

interface AppState {
  // Navigation
  currentPath: string
  pathHistory: string[]
  historyIndex: number
  
  // Files
  files: FileItem[]
  selectedFiles: string[]
  loading: boolean
  error?: string
  
  // Preferences
  globalPreferences: ViewPreferences
  directoryPreferences: Record<string, Partial<ViewPreferences>>
  theme: Theme
  
  // UI State
  sidebarWidth: number
  showSidebar: boolean
  showPreviewPanel: boolean
  
  // Actions
  setCurrentPath: (path: string) => void
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
}

export const useAppStore = create<AppState>((set, get) => ({
  // Initial state
  currentPath: '',
  pathHistory: [],
  historyIndex: -1,
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
  
  sidebarWidth: 240,
  showSidebar: true,
  showPreviewPanel: false,
  
  // Actions
  setCurrentPath: (path) => set({ currentPath: path }),
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
}))