import { useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import Sidebar from './components/Sidebar'
import MainPanel from './components/MainPanel'
import PathBar from './components/PathBar'
import { useAppStore } from './store/useAppStore'
import { message } from '@tauri-apps/plugin-dialog'

function App() {
  const { currentPath, setCurrentPath, navigateTo, setLoading, setError, setFiles, loading, setHomeDir, toggleHiddenFiles, toggleFoldersFirst, directoryPreferences } = useAppStore()
  const initializedRef = useRef(false)
  const [showLoadingOverlay, setShowLoadingOverlay] = useState(false)
  const prefsLoadedRef = useRef(false)
  const firstLoadRef = useRef(true)

  // Apply smart default view and sort preferences based on folder name or contents
  const applySmartViewDefaults = async (path: string, files?: any[]) => {
    try {
      const { directoryPreferences, updateDirectoryPreferences } = useAppStore.getState()
      const existing = directoryPreferences[path]
      
      // If user already set sortBy, don't override it — but fill in a missing sortOrder
      if (existing && (existing as any).sortBy) {
        const sb = (existing as any).sortBy as 'name' | 'size' | 'modified' | 'type'
        const so = (existing as any).sortOrder as 'asc' | 'desc' | undefined
        if (!so) {
          const defaultOrder: 'asc' | 'desc' = (sb === 'size' || sb === 'modified') ? 'desc' : 'asc'
          updateDirectoryPreferences(path, { sortOrder: defaultOrder })
          try { await invoke('set_dir_prefs', { path, prefs: JSON.stringify({ sortOrder: defaultOrder }) }) } catch {}
        }
        return
      }

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
        try { await invoke('set_dir_prefs', { path, prefs: JSON.stringify(folderDefaults[base]) }) } catch {}
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
            const prefs = { viewMode: 'grid' as const, sortBy: 'modified' as const, sortOrder: 'desc' as const }
            updateDirectoryPreferences(path, prefs)
            try { await invoke('set_dir_prefs', { path, prefs: JSON.stringify(prefs) }) } catch {}
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
        
        // Apply system accent color (macOS) to CSS variables FIRST
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
          console.warn('Could not get system accent color:', e)
        }
        
        // Now try to load the initial directory
        let loadSuccess = false
        try {
          const files = await invoke<any[]>('read_directory', { path: startPath })
          setFiles(files)
          await applySmartViewDefaults(startPath, files)
          setCurrentPath(startPath)
          navigateTo(startPath)
          loadSuccess = true
        } catch (dirError) {
          console.error('Failed to load initial directory:', startPath, dirError)
          
          // Try fallback to home directory
          if (startPath !== homeDir) {
            try {
              const files = await invoke<any[]>('read_directory', { path: homeDir })
              setFiles(files)
              await applySmartViewDefaults(homeDir, files)
              setCurrentPath(homeDir)
              navigateTo(homeDir)
              loadSuccess = true
            } catch (homeError) {
              console.error('Failed to load home directory:', homeError)
            }
          }
          
          // Last resort: try root
          if (!loadSuccess) {
            try {
              const rootPath = '/'
              const files = await invoke<any[]>('read_directory', { path: rootPath })
              setFiles(files)
              await applySmartViewDefaults(rootPath, files)
              setCurrentPath(rootPath)
              navigateTo(rootPath)
              loadSuccess = true
            } catch (rootError) {
              console.error('Failed to load root directory:', rootError)
              // Show error only if we can't load ANY directory
              await message('Unable to access any directory. Please check filesystem permissions.', {
                title: 'Fatal Error',
                okLabel: 'OK',
                kind: 'error'
              })
            }
          }
        }
        
        // Mark initialization complete only if we successfully loaded something
        if (loadSuccess) {
          setError(undefined)
          prefsLoadedRef.current = true
          initializedRef.current = true
          firstLoadRef.current = false
        }
      } catch (error) {
        // Critical error (can't even get home directory)
        console.error('Critical initialization error:', error)
        await message('Failed to initialize application. Please restart.', {
          title: 'Fatal Error',
          okLabel: 'OK',
          kind: 'error'
        })
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
    // Skip if not initialized yet or if this is the first load (already handled in init)
    if (!initializedRef.current || firstLoadRef.current) return
    
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
        
        // Try to load the directory
        const files = await invoke<any[]>('read_directory', { path: currentPath })
        setFiles(files)
        await applySmartViewDefaults(currentPath, files)
        setError(undefined)  // Clear any previous errors on success
      } catch (error) {
        const errorMessage = error instanceof Error ? error.message : String(error)
        console.error('Failed to load directory:', currentPath, error)
        
        // Show alert for all directory access errors
        const isPermissionError = errorMessage.includes('Operation not permitted')
        const hint = isPermissionError
          ? '\n\nAllow Marlin under System Settings → Privacy & Security → Files and Folders.'
          : ''
        
        // Show native alert dialog
        await message(`Cannot access: ${currentPath}\n\n${errorMessage}${hint}`, {
          title: 'Directory Error',
          okLabel: 'OK',
          kind: 'error'
        })
        
        // Navigate back to previous valid location
        // Don't change files - keep showing the previous directory's content
        const { goBack, pathHistory, historyIndex } = useAppStore.getState()
        if (pathHistory.length > 1 && historyIndex > 0) {
          // Go back in history (this will restore the previous path in the address bar)
          goBack()
        } else {
          // If no history, at least update the path to match what we're showing
          const { homeDir } = useAppStore.getState()
          if (homeDir && currentPath !== homeDir) {
            setCurrentPath(homeDir)
          }
        }
      } finally {
        setLoading(false)
      }
    }

    loadDirectory()
  }, [currentPath, setLoading, setError, setFiles, setCurrentPath])

  // Persist only current directory prefs on change to avoid global clobbering
  useEffect(() => {
    if (!initializedRef.current) return
    const state = useAppStore.getState()
    const prefs = state.directoryPreferences[state.currentPath]
    if (!prefs) return
    ;(async () => {
      try {
        await invoke('set_dir_prefs', { path: state.currentPath, prefs: JSON.stringify(prefs) })
        // Keep native context menu's sort state in sync
        if (prefs.sortBy || prefs.sortOrder) {
          const sortBy = prefs.sortBy ?? state.globalPreferences.sortBy
          const sortOrder = prefs.sortOrder ?? state.globalPreferences.sortOrder
          try { await invoke('update_sort_menu_state', { sort_by: sortBy, ascending: sortOrder === 'asc' }) } catch {}
        }
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
      await register('menu:toggle_hidden', async (e) => {
        const payload = e && typeof e.payload === 'boolean' ? (e.payload as boolean) : undefined
        if (typeof payload === 'boolean') {
          // Set per-directory and remember as global last-used, then reload
          const { currentPath, updateDirectoryPreferences, setFiles, setLoading, setError, setLastUsedShowHidden } = useAppStore.getState() as any
          updateDirectoryPreferences(currentPath, { showHidden: payload })
          await setLastUsedShowHidden(payload)
          try {
            setLoading(true)
            const files = await invoke<any[]>('read_directory', { path: currentPath })
            setFiles(files)
          } catch (err) {
            const msg = err instanceof Error ? err.message : String(err)
            setError(`Failed to reload files: ${msg}`)
          } finally {
            setLoading(false)
          }
          try { await invoke('update_hidden_files_menu', { checked: payload, source: 'frontend' }) } catch {}
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

      // Copy actions from context menu
      const copyToClipboard = async (text: string) => {
        try {
          await navigator.clipboard.writeText(text)
          return
        } catch (_) {
          // Fallback: use a temporary textarea
          try {
            const ta = document.createElement('textarea')
            ta.value = text
            ta.style.position = 'fixed'
            ta.style.opacity = '0'
            document.body.appendChild(ta)
            ta.focus()
            ta.select()
            document.execCommand('copy')
            document.body.removeChild(ta)
          } catch (e) {
            console.error('Failed to copy to clipboard', e)
          }
        }
      }

      const copyNames = async (fullPath: boolean) => {
        const state = useAppStore.getState()
        const selected = state.selectedFiles
        if (!selected || selected.length === 0) return
        const byPath = new Map(state.files.map(f => [f.path, f]))
        const parts: string[] = []
        for (const p of selected) {
          const f = byPath.get(p)
          if (!f) continue
          // fullPath => copy absolute path; else => copy file name with extension
          parts.push(fullPath ? f.path : f.name)
        }
        if (parts.length > 0) {
          await copyToClipboard(parts.join('\n'))
        }
      }

      await register('menu:copy_name', () => { void copyNames(false) })
      await register('menu:copy_full_name', () => { void copyNames(true) })
      
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

      await register('menu:clear_thumbnail_cache', async () => {
        // Clear the thumbnail cache
        try {
          const result = await invoke('clear_thumbnail_cache')
          console.log('Thumbnail cache cleared:', result)
          // Optionally refresh current view to show the effect
          const { refreshCurrentDirectory } = useAppStore.getState()
          await refreshCurrentDirectory()
        } catch (err) {
          console.error('Failed to clear thumbnail cache:', err)
        }
      })
      
    })()

    // Keyboard shortcuts as fallback (mac-like)
    const onKey = (e: KeyboardEvent) => {
      const isMac = navigator.userAgent.toUpperCase().includes('MAC')

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
              ? ' Grant access under System Settings → Privacy & Security → Files and Folders.'
              : ''
            await message(`Failed to refresh: ${msg}${hint}`, {
              title: 'Refresh Error',
              okLabel: 'OK',
              kind: 'error'
            })
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
      try { await invoke('update_sort_menu_state', { sort_by: prefs.sortBy, ascending: prefs.sortOrder === 'asc' }) } catch {}
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
