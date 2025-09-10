import { useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import Sidebar from './components/Sidebar'
import MainPanel from './components/MainPanel'
import PathBar from './components/PathBar'
import { useAppStore } from './store/useAppStore'
import { open } from '@tauri-apps/plugin-dialog'

function App() {
  const { currentPath, setCurrentPath, navigateTo, setLoading, setError, setFiles, loading, error, setHomeDir, toggleHiddenFiles, toggleFoldersFirst, directoryPreferences } = useAppStore()
  const initializedRef = useRef(false)
  const [showLoadingOverlay, setShowLoadingOverlay] = useState(false)
  const prefsLoadedRef = useRef(false)

  // Apply smart default view and sort preferences based on folder name or contents
  const applySmartViewDefaults = (path: string, files?: any[]) => {
    try {
      const { directoryPreferences, updateDirectoryPreferences } = useAppStore.getState()
      const existing = directoryPreferences[path]
      
      // Don't override if user has already set preferences for this folder
      if (existing && (existing.viewMode || existing.sortBy)) return

      const normalized = path.replace(/\\/g, '/').replace(/\/+$/g, '') || '/'
      const base = normalized.split('/').pop()?.toLowerCase() || ''
      
      // Smart defaults based on folder name
      const folderDefaults: Record<string, Partial<typeof existing>> = {
        'downloads': { sortBy: 'modified', sortOrder: 'desc' },
        'download': { sortBy: 'modified', sortOrder: 'desc' },
        'pictures': { viewMode: 'grid', sortBy: 'modified', sortOrder: 'desc' },
        'photos': { viewMode: 'grid', sortBy: 'modified', sortOrder: 'desc' },
        'screenshots': { viewMode: 'grid', sortBy: 'modified', sortOrder: 'desc' },
        'videos': { viewMode: 'grid', sortBy: 'modified', sortOrder: 'desc' },
        'movies': { viewMode: 'grid', sortBy: 'modified', sortOrder: 'desc' },
        'applications': { viewMode: 'grid', sortBy: 'name', sortOrder: 'asc' },
        'documents': { sortBy: 'modified', sortOrder: 'desc' },
        'desktop': { sortBy: 'modified', sortOrder: 'desc' }
      }
      
      // Apply folder-specific defaults if found
      if (folderDefaults[base]) {
        updateDirectoryPreferences(path, folderDefaults[base])
        return
      }

      // Media-heavy folders default to grid with date sorting (>= 75% media)
      if (files && Array.isArray(files)) {
        const nonFolder = files.filter((f) => !f.is_directory)
        if (nonFolder.length > 0) {
          const mediaExtensions = new Set([
            'jpg', 'jpeg', 'png', 'gif', 'webp', 'svg', 'bmp', 'heic', 'raw',
            'mp4', 'mkv', 'avi', 'mov', 'webm', 'flv', 'm4v'
          ])
          const mediaFiles = nonFolder.filter((f) => {
            const ext = f.extension?.toLowerCase()
            return ext && mediaExtensions.has(ext)
          })
          if (mediaFiles.length / nonFolder.length >= 0.75) {
            updateDirectoryPreferences(path, { 
              viewMode: 'grid',
              sortBy: 'modified',
              sortOrder: 'desc'
            })
          }
        }
      }
    } catch (_) {}
  }

  // Only show the blocking loading overlay if loading lasts > 500ms
  useEffect(() => {
    let timer: number | undefined
    if (loading) {
      timer = window.setTimeout(() => setShowLoadingOverlay(true), 500)
    } else {
      setShowLoadingOverlay(false)
    }
    return () => { if (timer) window.clearTimeout(timer) }
  }, [loading])

  // Remove global subscriptions that write the entire file to avoid clobbering across windows

  useEffect(() => {
    // Initialize the app by getting the home directory
    async function initializeApp() {
      try {
        setLoading(true)
        setError(undefined)
        
        // Load persisted preferences first
        let lastDir: string | undefined
        try {
          const raw = await invoke<string>('read_preferences')
          if (raw) {
            const parsed = JSON.parse(raw || '{}') as any
            if (parsed && typeof parsed === 'object') {
              if (parsed.globalPreferences && typeof parsed.globalPreferences === 'object') {
                useAppStore.getState().updateGlobalPreferences(parsed.globalPreferences)
              }
              if (parsed.directoryPreferences && typeof parsed.directoryPreferences === 'object') {
                Object.entries(parsed.directoryPreferences).forEach(([path, prefs]) => {
                  useAppStore.getState().updateDirectoryPreferences(path, prefs as any)
                })
              }
              if (typeof parsed.lastDir === 'string') lastDir = parsed.lastDir
            }
          }
        } catch { /* ignore */ }

        const homeDir = await invoke<string>('get_home_directory')

        setHomeDir(homeDir)

        // Check if a path was provided via URL parameter (for new windows)
        const urlParams = new URLSearchParams(window.location.search)
        const initialPath = urlParams.get('path')
        const startPath = initialPath ? decodeURIComponent(initialPath) : (lastDir || homeDir)
        
        setCurrentPath(startPath)
        navigateTo(startPath)
        
        // Load initial files and apply smart defaults
        const files = await invoke<any[]>('read_directory', { path: startPath })
        setFiles(files)
        applySmartViewDefaults(startPath, files)
        
        // Apply system accent color (macOS) to CSS variables
        try {
          const accent = await invoke<string>('get_system_accent_color')
          if (accent && /^#?[0-9a-fA-F]{6}$/.test(accent)) {
            const hex = accent.startsWith('#') ? accent.slice(1) : accent
            const r = parseInt(hex.substring(0,2), 16)
            const g = parseInt(hex.substring(2,4), 16)
            const b = parseInt(hex.substring(4,6), 16)
            const soft = `rgba(${r}, ${g}, ${b}, 0.15)`
            const selected = `rgba(${r}, ${g}, ${b}, 0.28)`
            document.documentElement.style.setProperty('--accent', `#${hex}`)
            document.documentElement.style.setProperty('--accent-soft', soft)
            document.documentElement.style.setProperty('--accent-selected', selected)
          }
        } catch (e) {
          // Ignore accent color lookup failures
        }
        setError(undefined)
        prefsLoadedRef.current = true
        
        initializedRef.current = true
      } catch (error) {
        const errorMessage = error instanceof Error ? error.message : String(error)
        const hint = errorMessage.includes('Operation not permitted')
          ? '\nHint (macOS): Allow Marlin under System Settings → Privacy & Security → Files and Folders (Desktop/Documents/Downloads), then retry.'
          : ''
        setError(`Failed to initialize: ${errorMessage}${hint}`)
        
        // Fallback: try to show some content even if initialization fails
        try {
          // Fallback: go to filesystem root to avoid showing ~
          const fallbackPath = '/'
          setCurrentPath(fallbackPath)
        } catch (fallbackError) {
          // ignore
        }
      } finally {
        setLoading(false)
      }
    }

    initializeApp()
  }, [setCurrentPath, navigateTo, setLoading, setError, setFiles, setHomeDir])

  // Persist lastDir on navigation
  useEffect(() => {
    if (!prefsLoadedRef.current) return
    ;(async () => {
      try {
        await invoke('set_last_dir', { path: useAppStore.getState().currentPath })
      } catch {}
    })()
  }, [currentPath])

  // Note: Menu checkbox sync is now handled directly in the centralized toggleHiddenFiles function

  // Load directory preferences and files when currentPath changes
  useEffect(() => {
    // Skip if not initialized yet or if this is the initial setup
    if (!initializedRef.current) return
    
    async function loadDirectory() {
      try {
        setLoading(true)
        setError(undefined)
        // Load per-directory preferences first (so sort applies before rendering)
        try {
          const raw = await invoke<string>('get_dir_prefs', { path: currentPath })
          if (raw) {
            const prefs = JSON.parse(raw || '{}') as any
            if (prefs && typeof prefs === 'object') {
              useAppStore.getState().updateDirectoryPreferences(currentPath, prefs)
            }
          }
        } catch { /* ignore */ }
        const files = await invoke<any[]>('read_directory', { path: currentPath })
        setFiles(files)
        applySmartViewDefaults(currentPath, files)
      } catch (error) {
        const errorMessage = error instanceof Error ? error.message : String(error)
        const hint = errorMessage.includes('Operation not permitted')
          ? '\nHint (macOS): Allow Marlin under System Settings → Privacy & Security → Files and Folders for this folder (Desktop/Documents/Downloads), then retry.'
          : ''
        setError(`Failed to load directory: ${errorMessage}${hint}`)
      } finally {
        setLoading(false)
      }
    }

    loadDirectory()
  }, [currentPath, setLoading, setError, setFiles])

  // Persist only current directory prefs on change to avoid global clobbering
  useEffect(() => {
    if (!initializedRef.current) return
    const state = useAppStore.getState()
    const prefs = state.directoryPreferences[state.currentPath]
    if (!prefs) return
    ;(async () => {
      try {
        await invoke('set_dir_prefs', { path: state.currentPath, prefs: JSON.stringify(prefs) })
      } catch { /* ignore */ }
    })()
  }, [currentPath, directoryPreferences[currentPath]])

  // View and sort controls via system menu or keyboard
  useEffect(() => {
    const unsubs: Array<() => void> = []

    // Tauri menu events (if provided by backend)
    const register = async (event: string, handler: (e?: any) => void) => {
      try {
        // Wrap the handler to properly handle the event object
        const un = await listen(event, (_event) => {
          handler(_event)
        })
        unsubs.push(() => un())
      } catch (error) {
        // ignore listener registration failures
      }
    }

    // Helpers to update preferences
    const setView = (mode: 'grid' | 'list') => {
      useAppStore.getState().updateDirectoryPreferences(
        useAppStore.getState().currentPath,
        { viewMode: mode }
      )
    }
    const setSortBy = (sortBy: 'name' | 'size' | 'modified' | 'type') => {
      const defaultOrder: 'asc' | 'desc' = (sortBy === 'size' || sortBy === 'modified') ? 'desc' : 'asc'
      useAppStore.getState().updateDirectoryPreferences(
        useAppStore.getState().currentPath,
        { sortBy, sortOrder: defaultOrder }
      )
    }
    const setSortOrder = (sortOrder: 'asc' | 'desc') => {
      useAppStore.getState().updateDirectoryPreferences(
        useAppStore.getState().currentPath,
        { sortOrder }
      )
    }
    const toggleHidden = () => { toggleHiddenFiles() }

    // Menu bindings - properly await async registration
    
    // Register all listeners asynchronously
    ;(async () => {
      await register('menu:toggle_hidden', (e) => {
        const payload = e && typeof e.payload === 'boolean' ? (e.payload as boolean) : undefined
        if (typeof payload === 'boolean') {
          // Per-directory hidden toggle
          const { currentPath, updateDirectoryPreferences } = useAppStore.getState()
          updateDirectoryPreferences(currentPath, { showHidden: payload })
        } else {
          toggleHidden()
        }
      })
      
      await register('menu:view_list', () => setView('list'))
      await register('menu:view_grid', () => setView('grid'))
      await register('menu:sort_name', () => setSortBy('name'))
      await register('menu:sort_size', () => setSortBy('size'))
      await register('menu:sort_modified', () => setSortBy('modified'))
      await register('menu:sort_type', () => setSortBy('type'))
      await register('menu:sort_order_asc', () => setSortOrder('asc'))
      await register('menu:sort_order_desc', () => setSortOrder('desc'))
      
      await register('menu:new_window', () => {
        // Create new window in current directory
        const currentPath = useAppStore.getState().currentPath
        invoke('new_window', { path: currentPath }).catch(err => {
          console.error('Failed to create new window:', err)
        })
      })

      await register('menu:folders_first', (e) => {
        const payload = e && typeof e.payload === 'boolean' ? (e.payload as boolean) : undefined
        if (typeof payload === 'boolean') {
          useAppStore.getState().updateGlobalPreferences({ foldersFirst: payload })
        } else {
          toggleFoldersFirst()
        }
      })

      await register('menu:reset_folder_defaults', async () => {
        // Clear all directory preferences and persist the change
        useAppStore.getState().resetDirectoryPreferences()
        try {
          await invoke('clear_all_dir_prefs')
        } catch (err) {
          console.error('Failed to clear directory preferences:', err)
        }
      })
      
    })()

    // Keyboard shortcuts as fallback (mac-like)
    const onKey = (e: KeyboardEvent) => {
      const isMac = navigator.platform.toUpperCase().includes('MAC')

      // Back: macOS Cmd+[ , Windows/Linux Alt+Left
      if ((isMac && e.metaKey && e.key === '[') || (!isMac && e.altKey && e.key === 'ArrowLeft')) {
        e.preventDefault()
        useAppStore.getState().goBack()
        return
      }

      // Forward: macOS Cmd+] , Windows/Linux Alt+Right
      if ((isMac && e.metaKey && e.key === ']') || (!isMac && e.altKey && e.key === 'ArrowRight')) {
        e.preventDefault()
        useAppStore.getState().goForward()
        return
      }

      // Go Up: macOS Cmd+Up, Windows/Linux Alt+Up
      if ((isMac && e.metaKey && e.key === 'ArrowUp') || (!isMac && e.altKey && e.key === 'ArrowUp')) {
        e.preventDefault()
        useAppStore.getState().goUp()
        return
      }

      // Refresh: F5 (all), macOS Cmd+R, Windows/Linux Ctrl+R
      const keyLower = e.key.toLowerCase()
      if (e.key === 'F5' || (isMac && e.metaKey && keyLower === 'r') || (!isMac && e.ctrlKey && keyLower === 'r')) {
        e.preventDefault()
        ;(async () => {
          const { currentPath, setFiles, setLoading, setError } = useAppStore.getState()
          try {
            setLoading(true)
            setError(undefined)
            const files = await invoke<any[]>('read_directory', { path: currentPath })
            setFiles(files)
          } catch (err) {
            const msg = err instanceof Error ? err.message : String(err)
            const hint = msg.includes('Operation not permitted')
              ? '\nHint (macOS): Grant access under System Settings → Privacy & Security → Files and Folders.'
              : ''
            setError(`Failed to refresh: ${msg}${hint}`)
          } finally {
            setLoading(false)
          }
        })()
        return
      }

      const meta = e.metaKey || e.ctrlKey
      if (!meta) return
      
      // New window: Cmd/Ctrl+N
      if (e.key === 'n' || e.key === 'N') {
        e.preventDefault()
        const currentPath = useAppStore.getState().currentPath
        invoke('new_window', { path: currentPath }).catch(err => {
          console.error('Failed to create new window:', err)
        })
        return
      }
      
      if (e.key === '1') { e.preventDefault(); setView('grid') }
      if (e.key === '2') { e.preventDefault(); setView('list') }
      // Toggle hidden: Cmd+Shift+.
      if (e.key === '.' && e.shiftKey) { e.preventDefault(); toggleHidden() }
    }
    window.addEventListener('keydown', onKey)
    unsubs.push(() => window.removeEventListener('keydown', onKey))

    return () => { unsubs.forEach((u) => u()) }
  }, [])

  // Keep native menu checkboxes in sync when changing directories (per-dir)
  useEffect(() => {
    const state = useAppStore.getState()
    const prefs = { ...state.globalPreferences, ...state.directoryPreferences[state.currentPath] }
    const sync = async () => {
      try { await invoke('update_hidden_files_menu', { checked: !!prefs.showHidden, source: 'frontend' }) } catch {}
      try { await invoke('update_folders_first_menu', { checked: !!prefs.foldersFirst, source: 'frontend' }) } catch {}
    }
    sync()
  }, [currentPath])
  
  // No verbose debug logging in production UI

  if (showLoadingOverlay) {
    return (
      <div className="h-screen flex items-center justify-center bg-app-dark text-app-text">
        <div className="text-center">
          <div className="animate-spin w-8 h-8 border-2 border-app-accent border-t-transparent rounded-full mx-auto mb-4"></div>
          <div className="text-app-muted">Loading Marlin File Browser...</div>
          <div className="text-xs text-app-muted mt-2">Initializing Tauri backend...</div>
        </div>
      </div>
    )
  }

  if (error) {
    const isMac = navigator.platform.toUpperCase().includes('MAC')
    const maybePermissionIssue = /Operation not permitted/i.test(error)

    const grantAccess = async () => {
      try {
        const path = await open({
          directory: true,
          multiple: false,
          defaultPath: currentPath,
          title: 'Grant Access to Folder',
        })
        if (!path) return
        // Try reloading the current directory (permission should be granted via picker)
        setLoading(true)
        setError(undefined)
        const files = await invoke<any[]>('read_directory', { path: currentPath })
        setFiles(files)
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e)
        setError(`Failed to grant access: ${msg}`)
      } finally {
        setLoading(false)
      }
    }
    return (
      <div className="h-screen flex items-center justify-center bg-app-dark text-app-text">
        <div className="text-center max-w-lg p-6">
          <div className="text-app-red text-6xl mb-4">⚠</div>
          <h1 className="text-xl font-semibold mb-4">Application Error</h1>
          <div className="text-app-red bg-app-gray p-4 rounded-md text-sm mb-4">
            {error}
          </div>
          <div className="text-app-muted text-sm mb-4">
            Check the browser console for more detailed error information.
          </div>
          <div className="flex items-center justify-center gap-3">
            {isMac && maybePermissionIssue && (
              <button
                onClick={grantAccess}
                className="button-secondary"
              >
                Grant Access…
              </button>
            )}
            <button 
              onClick={() => window.location.reload()}
              className="button-primary"
            >
              Reload Application
            </button>
          </div>
        </div>
      </div>
    )
  }

  return (
    <div className="h-screen flex bg-app-dark text-app-text overflow-hidden">
      {/* Sidebar full-height */}
      <div className="h-screen p-2">
        <Sidebar />
      </div>

      {/* Content column with path bar + panel */}
      <div className="flex-1 flex flex-col overflow-hidden min-h-0">
        <PathBar />
        <MainPanel />
      </div>
    </div>
  )
}

export default App
