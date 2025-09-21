import { useEffect, useRef, useState, MouseEvent, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { Event } from '@tauri-apps/api/event';
import Sidebar from './components/Sidebar';
import MainPanel from './components/MainPanel';
import PathBar from './components/PathBar';
import StatusBar from './components/StatusBar';
import { useAppStore } from './store/useAppStore';
import { useToastStore } from './store/useToastStore';
import { openFolderSizeWindow } from './store/useFolderSizeStore';
import { message } from '@tauri-apps/plugin-dialog';
import { getCurrentWindow } from '@tauri-apps/api/window';

import Toast from './components/Toast';
import type {
  DirectoryChangeEventPayload,
  FileItem,
  PersistedPreferences,
  ViewPreferences,
} from './types';

function App() {
  const {
    currentPath,
    setCurrentPath,
    navigateTo,
    setLoading,
    setError,
    setFiles,
    loading,
    setHomeDir,
    toggleHiddenFiles,
    toggleFoldersFirst,
    directoryPreferences,
    globalPreferences,
    loadPinnedDirectories,
  } = useAppStore();
  const initializedRef = useRef(false);
  const [showLoadingOverlay, setShowLoadingOverlay] = useState(false);
  const prefsLoadedRef = useRef(false);
  const firstLoadRef = useRef(true);
  const altTogglePendingRef = useRef(false);
  const windowRef = useRef(getCurrentWindow());
  const currentDirectoryPreference = directoryPreferences[currentPath];
  // Apply smart default view and sort preferences based on folder name or contents
  const applySmartViewDefaults = async (path: string, files?: FileItem[]) => {
    try {
      const { directoryPreferences, updateDirectoryPreferences } = useAppStore.getState();
      const existing = directoryPreferences[path];

      // If user already set any preferences, don't override them — but fill in missing defaults
      if (existing && Object.keys(existing).length > 0) {
        const sb = existing.sortBy;
        const so = existing.sortOrder;
        // Only fill in missing sortOrder if sortBy is set but sortOrder is not
        if (sb && !so) {
          const defaultOrder: ViewPreferences['sortOrder'] =
            sb === 'size' || sb === 'modified' ? 'desc' : 'asc';
          updateDirectoryPreferences(path, { sortOrder: defaultOrder });
          try {
            await invoke('set_dir_prefs', {
              path,
              prefs: JSON.stringify({ sortOrder: defaultOrder }),
            });
          } catch (error) {
            console.warn('Failed to persist sort order default:', error);
          }
        }
        return;
      }

      const normalized = path.replace(/\\/g, '/').replace(/\/+$/g, '') || '/';
      const base = normalized.split('/').pop()?.toLowerCase() ?? '';

      // Smart defaults based on folder name
      const folderDefaults: Record<string, Partial<ViewPreferences>> = {
        downloads: { sortBy: 'modified', sortOrder: 'desc' },
        download: { sortBy: 'modified', sortOrder: 'desc' },
        pictures: { viewMode: 'grid', sortBy: 'modified', sortOrder: 'desc' },
        photos: { viewMode: 'grid', sortBy: 'modified', sortOrder: 'desc' },
        screenshots: { viewMode: 'grid', sortBy: 'modified', sortOrder: 'desc' },
        videos: { viewMode: 'grid', sortBy: 'modified', sortOrder: 'desc' },
        movies: { viewMode: 'grid', sortBy: 'modified', sortOrder: 'desc' },
        applications: { viewMode: 'grid', sortBy: 'name', sortOrder: 'asc' },
        documents: { sortBy: 'modified', sortOrder: 'desc' },
        desktop: { sortBy: 'modified', sortOrder: 'desc' },
      };

      const folderDefault = folderDefaults[base];
      if (folderDefault) {
        updateDirectoryPreferences(path, folderDefault);
        try {
          await invoke('set_dir_prefs', { path, prefs: JSON.stringify(folderDefault) });
        } catch (error) {
          console.warn('Failed to persist folder defaults:', error);
        }
        return;
      }

      if (!files || files.length === 0) {
        return;
      }

      const mediaRelevantFiles = files.filter((file) => !file.is_directory);
      if (mediaRelevantFiles.length === 0) {
        return;
      }

      const mediaExtensions = new Set([
        'jpg',
        'jpeg',
        'png',
        'gif',
        'webp',
        'svg',
        'bmp',
        'heic',
        'raw',
        'mp4',
        'mkv',
        'avi',
        'mov',
        'webm',
        'flv',
        'm4v',
      ]);
      const mediaFiles = mediaRelevantFiles.filter((file) => {
        const ext = file.extension?.toLowerCase();
        return !!ext && mediaExtensions.has(ext);
      });

      if (mediaFiles.length / mediaRelevantFiles.length >= 0.75) {
        const prefs: Partial<ViewPreferences> = {
          viewMode: 'grid',
          sortBy: 'modified',
          sortOrder: 'desc',
        };
        updateDirectoryPreferences(path, prefs);
        try {
          await invoke('set_dir_prefs', { path, prefs: JSON.stringify(prefs) });
        } catch (error) {
          console.warn('Failed to persist media folder defaults:', error);
        }
        return;
      }

      const stlFiles = mediaRelevantFiles.filter((file) => file.extension?.toLowerCase() === 'stl');
      if (stlFiles.length >= 2 && stlFiles.length / mediaRelevantFiles.length >= 0.6) {
        const prefs: Partial<ViewPreferences> = { viewMode: 'grid' };
        updateDirectoryPreferences(path, prefs);
        try {
          await invoke('set_dir_prefs', { path, prefs: JSON.stringify(prefs) });
        } catch (error) {
          console.warn('Failed to persist STL folder defaults:', error);
        }
      }
    } catch (error) {
      console.warn('Failed to apply smart view defaults:', error);
    }
  };

  // Only show the blocking loading overlay if loading lasts > 500ms
  useEffect(() => {
    let timer: number | undefined;
    if (loading) {
      timer = window.setTimeout(() => setShowLoadingOverlay(true), 500);
    } else {
      setShowLoadingOverlay(false);
    }
    return () => {
      if (timer) window.clearTimeout(timer);
    };
  }, [loading]);

  // Remove global subscriptions that write the entire file to avoid clobbering across windows

  useEffect(() => {
    // Initialize the app by getting the home directory
    async function initializeApp() {
      try {
        setLoading(true);
        setError(undefined);

        // Load persisted preferences first
        let lastDir: string | undefined;
        try {
          const raw = await invoke<string>('read_preferences');
          if (raw) {
            const parsed = JSON.parse(raw || '{}') as PersistedPreferences;
            if (parsed.globalPreferences) {
              useAppStore.getState().updateGlobalPreferences(parsed.globalPreferences);
            }
            if (parsed.directoryPreferences) {
              Object.entries(parsed.directoryPreferences).forEach(([dirPath, prefs]) => {
                useAppStore.getState().updateDirectoryPreferences(dirPath, prefs);
              });
            }
            if (parsed.lastDir) {
              lastDir = parsed.lastDir;
            }
          }
        } catch (error) {
          console.error('❌ Error loading preferences:', error);
        }

        // Mark preferences as loaded regardless of whether we found any
        prefsLoadedRef.current = true;

        const homeDir = await invoke<string>('get_home_directory');
        setHomeDir(homeDir);

        // Load pinned directories
        await loadPinnedDirectories();

        // Check if a path was provided via URL parameter (for new windows)
        const urlParams = new URLSearchParams(window.location.search);
        const initialPath = urlParams.get('path');
        const startPath = initialPath ? decodeURIComponent(initialPath) : lastDir || homeDir;

        // Apply system accent color (macOS) to CSS variables FIRST
        try {
          const accent = await invoke<string>('get_system_accent_color');
          if (accent && /^#?[0-9a-fA-F]{6}$/.test(accent)) {
            const hex = accent.startsWith('#') ? accent.slice(1) : accent;
            const r = parseInt(hex.substring(0, 2), 16);
            const g = parseInt(hex.substring(2, 4), 16);
            const b = parseInt(hex.substring(4, 6), 16);
            const soft = `rgba(${r}, ${g}, ${b}, 0.15)`;
            const selected = `rgba(${r}, ${g}, ${b}, 0.28)`;
            document.documentElement.style.setProperty('--accent', `#${hex}`);
            document.documentElement.style.setProperty('--accent-soft', soft);
            document.documentElement.style.setProperty('--accent-selected', selected);
          }
        } catch (e) {
          console.warn('Could not get system accent color:', e);
        }

        // Now try to load the initial directory
        let loadSuccess = false;
        try {
          const files = await invoke<FileItem[]>('read_directory', { path: startPath });
          setFiles(files);
          await applySmartViewDefaults(startPath, files);
          setCurrentPath(startPath);
          navigateTo(startPath);
          loadSuccess = true;
        } catch (dirError) {
          console.error('Failed to load initial directory:', startPath, dirError);

          // Try fallback to home directory
          if (startPath !== homeDir) {
            try {
              const files = await invoke<FileItem[]>('read_directory', { path: homeDir });
              setFiles(files);
              await applySmartViewDefaults(homeDir, files);
              setCurrentPath(homeDir);
              navigateTo(homeDir);
              loadSuccess = true;
            } catch (homeError) {
              console.error('Failed to load home directory:', homeError);
            }
          }

          // Last resort: try root
          if (!loadSuccess) {
            try {
              const rootPath = '/';
              const files = await invoke<FileItem[]>('read_directory', { path: rootPath });
              setFiles(files);
              await applySmartViewDefaults(rootPath, files);
              setCurrentPath(rootPath);
              navigateTo(rootPath);
              loadSuccess = true;
            } catch (rootError) {
              console.error('Failed to load root directory:', rootError);
              // Show error only if we can't load ANY directory
              await message(
                'Unable to access any directory. Please check filesystem permissions.',
                {
                  title: 'Fatal Error',
                  okLabel: 'OK',
                  kind: 'error',
                }
              );
            }
          }
        }

        // Mark initialization complete only if we successfully loaded something
        if (loadSuccess) {
          setError(undefined);
          initializedRef.current = true;
          firstLoadRef.current = false;
        }
      } catch (error) {
        // Critical error (can't even get home directory)
        console.error('Critical initialization error:', error);
        await message('Failed to initialize application. Please restart.', {
          title: 'Fatal Error',
          okLabel: 'OK',
          kind: 'error',
        });
      } finally {
        setLoading(false);
      }
    }

    initializeApp();
  }, [
    setCurrentPath,
    navigateTo,
    setLoading,
    setError,
    setFiles,
    setHomeDir,
    loadPinnedDirectories,
  ]);

  // Persist lastDir on navigation
  useEffect(() => {
    if (!prefsLoadedRef.current) {
      return;
    }
    const currentPath = useAppStore.getState().currentPath;
    (async () => {
      try {
        await invoke('set_last_dir', { path: currentPath });
      } catch (error) {
        console.error('❌ Failed to save lastDir:', error);
      }
    })();
  }, [currentPath]);

  // Note: Menu checkbox sync is now handled directly in the centralized toggleHiddenFiles function

  // Load directory preferences and files when currentPath changes
  useEffect(() => {
    // Skip if not initialized yet or if this is the first load (already handled in init)
    if (!initializedRef.current || firstLoadRef.current) return;

    async function loadDirectory() {
      try {
        setLoading(true);
        setError(undefined);

        // Load per-directory preferences first (so sort applies before rendering)
        // Skip if preferences were recently updated to prevent race conditions
        try {
          const { lastPreferenceUpdate } = useAppStore.getState();
          const timeSinceUpdate = Date.now() - lastPreferenceUpdate;
          const shouldSkipLoad = timeSinceUpdate < 2000; // Skip if updated within 2 seconds

          if (!shouldSkipLoad) {
            const raw = await invoke<string>('get_dir_prefs', { path: currentPath });
            if (raw) {
              const parsed = JSON.parse(raw || '{}') as unknown;
              if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
                const prefs = parsed as Partial<ViewPreferences>;
                useAppStore.getState().updateDirectoryPreferences(currentPath, prefs);
              }
            }
          }
        } catch (error) {
          console.warn('Failed to load directory preferences:', error);
        }

        // Try to load the directory
        const files = await invoke<FileItem[]>('read_directory', { path: currentPath });
        setFiles(files);
        await applySmartViewDefaults(currentPath, files);
        setError(undefined); // Clear any previous errors on success
      } catch (error) {
        const errorMessage = error instanceof Error ? error.message : String(error);
        console.error('Failed to load directory:', currentPath, error);

        // Show alert for all directory access errors
        const isPermissionError = errorMessage.includes('Operation not permitted');
        const hint = isPermissionError
          ? '\n\nAllow Marlin under System Settings → Privacy & Security → Files and Folders.'
          : '';

        // Show native alert dialog
        await message(`Cannot access: ${currentPath}\n\n${errorMessage}${hint}`, {
          title: 'Directory Error',
          okLabel: 'OK',
          kind: 'error',
        });

        // Navigate back to previous valid location
        // Don't change files - keep showing the previous directory's content
        const { goBack, pathHistory, historyIndex } = useAppStore.getState();
        if (pathHistory.length > 1 && historyIndex > 0) {
          // Go back in history (this will restore the previous path in the address bar)
          goBack();
        } else {
          // If no history, at least update the path to match what we're showing
          const { homeDir } = useAppStore.getState();
          if (homeDir && currentPath !== homeDir) {
            setCurrentPath(homeDir);
          }
        }
      } finally {
        setLoading(false);
      }
    }

    loadDirectory();
  }, [currentPath, setLoading, setError, setFiles, setCurrentPath]);

  // File system watcher for auto-reload
  useEffect(() => {
    if (!currentPath || loading) {
      return;
    }

    let isActive = true;
    let debounceTimer: number | undefined;
    let cleanupFunction: (() => void) | undefined;

    const handleDirectoryChanged = (event: Event<DirectoryChangeEventPayload>) => {
      if (!isActive) return;

      const payload = event.payload;
      if (payload && payload.path === currentPath) {
        // Clear any existing debounce timer
        if (debounceTimer) {
          window.clearTimeout(debounceTimer);
        }

        // Debounce the refresh to avoid excessive reloads
        debounceTimer = window.setTimeout(async () => {
          if (!isActive || loading) return;

          try {
            const { refreshCurrentDirectory, selectedFiles } = useAppStore.getState();
            await refreshCurrentDirectory();

            // Preserve selection if possible after refresh
            if (selectedFiles.length > 0) {
              const { files, setSelectedFiles } = useAppStore.getState();
              const stillExist = selectedFiles.filter((path) => files.some((f) => f.path === path));
              if (stillExist.length !== selectedFiles.length) {
                setSelectedFiles(stillExist);
              }
            }
          } catch (error) {
            console.warn('Auto-refresh failed:', error);
          }
        }, 500); // 500ms debounce
      }
    };

    const setupWatcher = async () => {
      if (!isActive) return;

      try {
        // Start watching current directory
        await invoke('start_watching_directory', { path: currentPath });

        // Listen for directory change events
        const unlisten = await listen('directory-changed', handleDirectoryChanged);

        // Store cleanup function
        cleanupFunction = () => {
          unlisten();
          invoke('stop_watching_directory', { path: currentPath }).catch(() => {
            // Ignore errors during cleanup
          });
        };
      } catch (error) {
        console.warn('Failed to setup file watcher:', error);
      }
    };

    setupWatcher();

    return () => {
      isActive = false;
      if (debounceTimer) {
        window.clearTimeout(debounceTimer);
      }
      if (cleanupFunction) {
        cleanupFunction();
      }
    };
  }, [currentPath, loading]);

  // Persist only current directory prefs on change to avoid global clobbering
  useEffect(() => {
    if (!initializedRef.current) return;
    const state = useAppStore.getState();
    const prefs = state.directoryPreferences[state.currentPath];
    if (!prefs) return;
    (async () => {
      try {
        await invoke('set_dir_prefs', { path: state.currentPath, prefs: JSON.stringify(prefs) });
        // Keep native context menu's sort state in sync
        if (prefs.sortBy || prefs.sortOrder) {
          const sortBy = prefs.sortBy ?? state.globalPreferences.sortBy;
          const sortOrder = prefs.sortOrder ?? state.globalPreferences.sortOrder;
          try {
            await invoke('update_sort_menu_state', { sortBy, ascending: sortOrder === 'asc' });
          } catch (error) {
            console.warn('Failed to update sort menu state:', error);
          }
        }
      } catch (error) {
        console.warn('Failed to persist directory preferences:', error);
      }
    })();
  }, [currentPath, currentDirectoryPreference]);

  // View and sort controls via system menu or keyboard
  useEffect(() => {
    const unsubs: Array<() => void> = [];

    // Tauri menu events (if provided by backend)
    const register = async <Payload,>(
      eventName: string,
      handler: (evt?: Event<Payload>) => void | Promise<void>
    ) => {
      try {
        const unlisten = await listen<Payload>(eventName, async (evt) => {
          await handler(evt);
        });
        unsubs.push(unlisten);
      } catch (error) {
        console.warn('Failed to register menu listener:', { eventName, error });
      }
    };

    // Helpers to update preferences
    const setView = (mode: 'grid' | 'list') => {
      useAppStore
        .getState()
        .updateDirectoryPreferences(useAppStore.getState().currentPath, { viewMode: mode });
    };
    const setSortBy = (sortBy: 'name' | 'size' | 'modified' | 'type') => {
      const defaultOrder: 'asc' | 'desc' =
        sortBy === 'size' || sortBy === 'modified' ? 'desc' : 'asc';
      useAppStore.getState().updateDirectoryPreferences(useAppStore.getState().currentPath, {
        sortBy,
        sortOrder: defaultOrder,
      });
    };
    const setSortOrder = (sortOrder: 'asc' | 'desc') => {
      useAppStore
        .getState()
        .updateDirectoryPreferences(useAppStore.getState().currentPath, { sortOrder });
    };
    const toggleHidden = (value?: boolean) => {
      toggleHiddenFiles(value);
    };

    // Menu bindings - properly await async registration

    // Register all listeners asynchronously
    (async () => {
      const handleToggleHidden = async (event?: Event<boolean>) => {
        const nextValue = typeof event?.payload === 'boolean' ? event.payload : undefined;
        await toggleHidden(nextValue);
      };
      await register('menu:toggle_hidden', handleToggleHidden);
      await register('ctx:toggle_hidden', handleToggleHidden);

      await register('menu:view_list', () => setView('list'));
      await register('menu:view_grid', () => setView('grid'));
      await register('menu:sort_name', () => setSortBy('name'));
      await register('menu:sort_size', () => setSortBy('size'));
      await register('menu:sort_modified', () => setSortBy('modified'));
      await register('menu:sort_type', () => setSortBy('type'));
      await register('menu:sort_order_asc', () => setSortOrder('asc'));
      await register('menu:sort_order_desc', () => setSortOrder('desc'));

      const handleCalculateTotalSize = async () => {
        const state = useAppStore.getState();
        const selection = state.selectedFiles;
        if (!selection || selection.length === 0) return;
        const byPath = new Map(state.files.map((f) => [f.path, f]));
        const targets = selection
          .map((path) => byPath.get(path))
          .filter((file): file is FileItem => Boolean(file));
        if (targets.length === 0) return;
        if (!targets.some((file) => file.is_directory)) {
          return;
        }
        await openFolderSizeWindow(
          targets.map((file) => ({
            path: file.path,
            name: file.name,
            isDirectory: file.is_directory,
          }))
        );
      };

      await register('menu:calculate_total_size', () => handleCalculateTotalSize());

      // Copy actions from context menu
      const copyToClipboard = async (text: string) => {
        try {
          await navigator.clipboard.writeText(text);
          return;
        } catch (clipboardError) {
          console.warn('Direct clipboard write failed, trying fallback:', clipboardError);
          // Fallback: use a temporary textarea
          try {
            const ta = document.createElement('textarea');
            ta.value = text;
            ta.style.position = 'fixed';
            ta.style.opacity = '0';
            document.body.appendChild(ta);
            ta.focus();
            ta.select();
            document.execCommand('copy');
            document.body.removeChild(ta);
          } catch (fallbackError) {
            console.error('Failed to copy to clipboard via fallback', fallbackError);
          }
        }
      };

      const copyNames = async (fullPath: boolean) => {
        const state = useAppStore.getState();
        const selected = state.selectedFiles;
        if (!selected || selected.length === 0) return;
        const byPath = new Map(state.files.map((f) => [f.path, f]));
        const parts: string[] = [];
        for (const p of selected) {
          const f = byPath.get(p);
          if (!f) continue;
          // fullPath => copy absolute path; else => copy file name with extension
          parts.push(fullPath ? f.path : f.name);
        }
        if (parts.length > 0) {
          await copyToClipboard(parts.join('\n'));
        }
      };

      await register('menu:copy_name', () => {
        void copyNames(false);
      });
      await register('menu:copy_full_name', () => {
        void copyNames(true);
      });
      await register('menu:rename', () => {
        useAppStore.getState().beginRenameSelected();
      });
      await register('menu:reveal_symlink', async () => {
        const state = useAppStore.getState();
        const selection = state.selectedFiles;
        if (!selection || selection.length === 0) return;
        const target = selection[0];
        try {
          const result = await invoke<{ parent: string; target: string }>(
            'resolve_symlink_parent_command',
            {
              path: target,
            }
          );
          state.setPendingRevealTarget(result.target);
          state.navigateTo(result.parent);
        } catch (error) {
          console.warn('Failed to resolve symlink parent from menu:', error);
          useToastStore.getState().addToast({
            type: 'error',
            message: 'Unable to locate the original item for this link.',
          });
        }
      });
      await register('menu:new_window', () => {
        // Create new window in current directory
        const currentPath = useAppStore.getState().currentPath;
        invoke('new_window', { path: currentPath }).catch((err) => {
          console.error('Failed to create new window:', err);
        });
      });

      const handleFoldersFirst = (event?: Event<boolean>) => {
        if (typeof event?.payload === 'boolean') {
          useAppStore.getState().updateGlobalPreferences({ foldersFirst: event.payload });
        } else {
          toggleFoldersFirst();
        }
      };
      await register('menu:folders_first', handleFoldersFirst);
      await register('ctx:folders_first', handleFoldersFirst);

      await register('menu:reset_folder_defaults', async () => {
        // Clear all directory preferences and persist the change
        useAppStore.getState().resetDirectoryPreferences();
        try {
          await invoke('clear_all_dir_prefs');
        } catch (err) {
          console.error('Failed to clear directory preferences:', err);
        }
      });

      await register('menu:clear_thumbnail_cache', async () => {
        // Clear the thumbnail cache
        try {
          await invoke('clear_thumbnail_cache');
          // Optionally refresh current view to show the effect
          const { refreshCurrentDirectory } = useAppStore.getState();
          await refreshCurrentDirectory();
        } catch (err) {
          console.error('Failed to clear thumbnail cache:', err);
        }
      });
    })();

    // Keyboard shortcuts as fallback (mac-like)
    const onKey = (e: KeyboardEvent) => {
      const uaUpper = navigator.userAgent.toUpperCase();
      const isMac = uaUpper.includes('MAC');
      const isLinux = uaUpper.includes('LINUX');
      const active = document.activeElement as HTMLElement | null;
      const inEditable =
        !!active &&
        (active.tagName === 'INPUT' || active.tagName === 'TEXTAREA' || active.isContentEditable);

      if (!isMac && isLinux) {
        if (e.key === 'Alt') {
          if (!e.repeat) {
            altTogglePendingRef.current = true;
          }
          return;
        }
        if (e.altKey) {
          altTogglePendingRef.current = false;
        }
      }

      // Arrow-key file navigation (no modifiers, not typing in inputs)
      if (!inEditable && !e.metaKey && !e.ctrlKey && !e.altKey) {
        const key = e.key;
        if (
          key === 'ArrowUp' ||
          key === 'ArrowDown' ||
          key === 'ArrowLeft' ||
          key === 'ArrowRight'
        ) {
          const state = useAppStore.getState();
          const selected = state.selectedFiles || [];

          // Determine current visible items in DOM order
          const nodes = Array.from(
            document.querySelectorAll<HTMLElement>('[data-file-item="true"][data-file-path]')
          );
          const order = nodes.map((n) => n.getAttribute('data-file-path') || '').filter(Boolean);
          if (order.length === 0) return;

          // Helper: ensure a given index is selected and scrolled into view
          const wrap = (v: number, n: number) => {
            if (n <= 0) return 0;
            let r = v % n;
            if (r < 0) r += n;
            return r;
          };
          const selectIndex = (idx: number) => {
            const index = wrap(idx, order.length);
            const path = order[index];
            state.setSelectedFiles([path]);
            state.setSelectionAnchor(path);
            state.setSelectionLead(path);
            state.setShiftBaseSelection(null);
            // Scroll into view
            const el = nodes[index];
            if (el && typeof el.scrollIntoView === 'function') {
              try {
                el.scrollIntoView({ block: 'nearest', inline: 'nearest' });
              } catch (error) {
                console.warn('Failed to scroll selection into view:', error);
              }
            }
          };

          // Determine if we're in grid (thumb) view to support 4-way nav
          const gridEl = document.querySelector<HTMLElement>('.file-grid');
          let cols = 1;
          if (gridEl) {
            // Estimate columns by counting how many items share the first row's top
            const firstRowTop = nodes[0]?.offsetTop ?? 0;
            let count = 0;
            for (let i = 0; i < nodes.length; i++) {
              if (Math.abs((nodes[i].offsetTop ?? 0) - firstRowTop) < 1) count++;
              else break;
            }
            cols = Math.max(1, count || 1);
          }

          // Map current selection to visible indices
          const visibleSelectedIdx = selected
            .map((p) => order.indexOf(p))
            .filter((i) => i >= 0)
            .sort((a, b) => a - b);

          const noneSelectedVisible = visibleSelectedIdx.length === 0;
          const highest = noneSelectedVisible ? -1 : visibleSelectedIdx[0];
          const lowest = noneSelectedVisible
            ? -1
            : visibleSelectedIdx[visibleSelectedIdx.length - 1];

          // Compute target index based on rules
          let targetIdx: number | null = null;

          if (key === 'ArrowUp') {
            if (noneSelectedVisible) {
              // If none selected: Up selects last
              targetIdx = order.length - 1;
            } else {
              // Up from highest selection
              targetIdx = gridEl ? highest - cols : highest - 1;
            }
          } else if (key === 'ArrowDown') {
            if (noneSelectedVisible) {
              // If none selected: Down selects first
              targetIdx = 0;
            } else {
              // Down from lowest selection
              targetIdx = gridEl ? lowest + cols : lowest + 1;
            }
          } else if (key === 'ArrowLeft') {
            if (!gridEl) {
              // List view: ignore Left/Right
              return;
            }
            if (noneSelectedVisible) {
              // Start at last on Left if nothing selected
              targetIdx = order.length - 1;
            } else {
              // Left from highest selection (reading order)
              targetIdx = highest - 1;
            }
          } else if (key === 'ArrowRight') {
            if (!gridEl) {
              // List view: ignore Left/Right
              return;
            }
            if (noneSelectedVisible) {
              // Start at first on Right if nothing selected
              targetIdx = 0;
            } else {
              // Right from lowest selection (reading order)
              targetIdx = lowest + 1;
            }
          }

          if (targetIdx === null) return;

          // Grid Up/Down: rollover when not using Shift; clamp when extending selection
          if (gridEl && (key === 'ArrowUp' || key === 'ArrowDown')) {
            if (e.shiftKey) {
              // Clamp to bounds when extending a range
              if (targetIdx < 0) targetIdx = 0;
              if (targetIdx >= order.length) targetIdx = order.length - 1;
            } else {
              const refIdx =
                key === 'ArrowUp'
                  ? noneSelectedVisible
                    ? 0
                    : highest
                  : noneSelectedVisible
                    ? 0
                    : lowest;
              const col = Math.max(0, refIdx % cols);
              if (key === 'ArrowUp' && targetIdx < 0) {
                // Wrap to last row same column
                const lastRowStart = Math.floor((order.length - 1) / cols) * cols;
                let cand = lastRowStart + col;
                while (cand >= order.length && cand >= 0) cand -= cols;
                targetIdx = cand >= 0 ? cand : order.length - 1;
              } else if (key === 'ArrowDown' && targetIdx >= order.length) {
                // Wrap to first row same column (or last if fewer items)
                let cand = col;
                if (cand >= order.length) cand = order.length - 1;
                targetIdx = cand;
              }
            }
          }

          // List: rollover when not using Shift; clamp when extending selection
          if (!gridEl && (key === 'ArrowUp' || key === 'ArrowDown')) {
            if (e.shiftKey) {
              if (targetIdx < 0) targetIdx = 0;
              if (targetIdx >= order.length) targetIdx = order.length - 1;
            } else {
              if (key === 'ArrowUp' && (noneSelectedVisible || highest <= 0)) {
                targetIdx = order.length - 1;
              }
              if (key === 'ArrowDown' && (noneSelectedVisible || lowest >= order.length - 1)) {
                targetIdx = 0;
              }
            }
          }

          // Horizontal grid: rollover when not using Shift; clamp with Shift
          if (gridEl && (key === 'ArrowLeft' || key === 'ArrowRight')) {
            if (e.shiftKey) {
              if (targetIdx < 0) targetIdx = 0;
              if (targetIdx >= order.length) targetIdx = order.length - 1;
            } else {
              if (key === 'ArrowLeft' && (noneSelectedVisible || highest <= 0)) {
                targetIdx = order.length - 1;
              }
              if (key === 'ArrowRight' && (noneSelectedVisible || lowest >= order.length - 1)) {
                targetIdx = 0;
              }
            }
          }

          // Shift+Arrow: extend selection as a contiguous range from anchor (preserve pre-selection)
          if (e.shiftKey) {
            // Initialize shift session base if needed
            const base = state.shiftBaseSelection ?? state.selectedFiles;
            state.setShiftBaseSelection(base.slice());

            // Establish anchor if missing or off-screen
            const anchorPath = state.selectionAnchor;
            let anchorIdx = anchorPath ? order.indexOf(anchorPath) : -1;
            if (anchorIdx < 0) {
              // If no anchor, set to the first visible selected index or current target
              anchorIdx = noneSelectedVisible ? targetIdx : (visibleSelectedIdx[0] ?? targetIdx);
              state.setSelectionAnchor(order[anchorIdx]);
            }

            // Establish current lead (caret)
            const leadPath = state.selectionLead;
            let leadIdx = leadPath ? order.indexOf(leadPath) : -1;
            if (leadIdx < 0) {
              // Start from anchor when no existing lead
              leadIdx = anchorIdx;
            }

            // Compute step for this key
            const delta =
              key === 'ArrowUp'
                ? gridEl
                  ? -cols
                  : -1
                : key === 'ArrowDown'
                  ? gridEl
                    ? +cols
                    : +1
                  : key === 'ArrowLeft'
                    ? -1
                    : key === 'ArrowRight'
                      ? +1
                      : 0;
            let newLead = leadIdx + delta;
            // Clamp without wrap during shift
            if (newLead < 0) newLead = 0;
            if (newLead >= order.length) newLead = order.length - 1;

            const start = Math.min(anchorIdx, newLead);
            const end = Math.max(anchorIdx, newLead);
            const range = order.slice(start, end + 1);
            const merged = Array.from(new Set([...(state.shiftBaseSelection || []), ...range]));
            e.preventDefault();
            state.setSelectedFiles(merged);
            state.setSelectionLead(order[newLead]);
            const el = nodes[newLead];
            if (el && typeof el.scrollIntoView === 'function') {
              try {
                el.scrollIntoView({ block: 'nearest', inline: 'nearest' });
              } catch (error) {
                console.warn('Failed to scroll shifted selection into view:', error);
              }
            }
            return;
          }

          e.preventDefault();
          selectIndex(targetIdx);
          return;
        }
      }

      // Back: macOS Cmd+[ , Windows/Linux Alt+Left
      if ((isMac && e.metaKey && e.key === '[') || (!isMac && e.altKey && e.key === 'ArrowLeft')) {
        e.preventDefault();
        useAppStore.getState().goBack();
        return;
      }

      // Forward: macOS Cmd+] , Windows/Linux Alt+Right
      if ((isMac && e.metaKey && e.key === ']') || (!isMac && e.altKey && e.key === 'ArrowRight')) {
        e.preventDefault();
        useAppStore.getState().goForward();
        return;
      }

      // Go Up: macOS Cmd+Up, Windows/Linux Alt+Up
      if (
        (isMac && e.metaKey && e.key === 'ArrowUp') ||
        (!isMac && e.altKey && e.key === 'ArrowUp')
      ) {
        e.preventDefault();
        useAppStore.getState().goUp();
        return;
      }

      // Refresh: F5 (all), macOS Cmd+R, Windows/Linux Ctrl+R
      const keyLower = e.key.toLowerCase();
      if (
        e.key === 'F5' ||
        (isMac && e.metaKey && keyLower === 'r') ||
        (!isMac && e.ctrlKey && keyLower === 'r')
      ) {
        e.preventDefault();
        (async () => {
          const { currentPath, setFiles, setLoading, setError } = useAppStore.getState();
          try {
            setLoading(true);
            setError(undefined);
            const files = await invoke<FileItem[]>('read_directory', { path: currentPath });
            setFiles(files);
          } catch (err) {
            const msg = err instanceof Error ? err.message : String(err);
            const hint = msg.includes('Operation not permitted')
              ? ' Grant access under System Settings → Privacy & Security → Files and Folders.'
              : '';
            await message(`Failed to refresh: ${msg}${hint}`, {
              title: 'Refresh Error',
              okLabel: 'OK',
              kind: 'error',
            });
          } finally {
            setLoading(false);
          }
        })();
        return;
      }

      // Rename: F2 everywhere; macOS Return (Enter) with no modifiers
      if (!inEditable) {
        if (e.key === 'F2') {
          e.preventDefault();
          useAppStore.getState().beginRenameSelected();
          return;
        }
        if (isMac && !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey && e.key === 'Enter') {
          e.preventDefault();
          useAppStore.getState().beginRenameSelected();
          return;
        }
      }

      const meta = e.metaKey || e.ctrlKey;
      if (!meta) return;

      // New window: Cmd/Ctrl+N
      if (e.key === 'n' || e.key === 'N') {
        e.preventDefault();
        const currentPath = useAppStore.getState().currentPath;
        invoke('new_window', { path: currentPath }).catch((err) => {
          console.error('Failed to create new window:', err);
        });
        return;
      }

      if (e.key === '1') {
        e.preventDefault();
        setView('grid');
      }
      if (e.key === '2') {
        e.preventDefault();
        setView('list');
      }
      // Toggle hidden: Cmd+Shift+.
      if (e.key === '.' && e.shiftKey) {
        e.preventDefault();
        toggleHidden();
      }
    };
    window.addEventListener('keydown', onKey);
    unsubs.push(() => window.removeEventListener('keydown', onKey));

    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === 'Alt') {
        const uaUpper = navigator.userAgent.toUpperCase();
        const isMac = uaUpper.includes('MAC');
        const isLinux = uaUpper.includes('LINUX');
        if (!isMac && isLinux) {
          if (altTogglePendingRef.current) {
            void invoke('toggle_menu_visibility').catch((err) => {
              console.warn('Failed to toggle menu visibility:', err);
            });
          }
          altTogglePendingRef.current = false;
        }
      }
      if (e.key === 'Shift') {
        useAppStore.getState().setShiftBaseSelection(null);
      }
    };
    window.addEventListener('keyup', onKeyUp);
    unsubs.push(() => window.removeEventListener('keyup', onKeyUp));

    return () => {
      unsubs.forEach((u) => u());
    };
  }, [toggleHiddenFiles, toggleFoldersFirst]);

  // Sync native menu checkboxes when preferences change
  useEffect(() => {
    if (!initializedRef.current) return;

    const state = useAppStore.getState();
    const currentDirPrefs = state.directoryPreferences[state.currentPath] || {};
    const effectivePrefs = { ...state.globalPreferences, ...currentDirPrefs };

    const sync = async () => {
      try {
        await invoke('update_hidden_files_menu', {
          checked: !!effectivePrefs.showHidden,
          source: 'frontend',
        });
      } catch (e) {
        console.warn('Failed to sync hidden files menu:', e);
      }

      try {
        await invoke('update_folders_first_menu', {
          checked: !!effectivePrefs.foldersFirst,
          source: 'frontend',
        });
      } catch (e) {
        console.warn('Failed to sync folders first menu:', e);
      }

      try {
        await invoke('update_sort_menu_state', {
          sortBy: effectivePrefs.sortBy,
          ascending: effectivePrefs.sortOrder === 'asc',
        });
      } catch (e) {
        console.warn('Failed to sync sort menu:', e);
      }
    };

    // Sync after a small delay to ensure all state updates are complete
    const timeoutId = setTimeout(sync, 10);
    return () => clearTimeout(timeoutId);
  }, [
    currentPath,
    currentDirectoryPreference,
    globalPreferences.showHidden,
    globalPreferences.foldersFirst,
    globalPreferences.sortBy,
    globalPreferences.sortOrder,
  ]);

  // No verbose debug logging in production UI

  const handleSidebarFrameMouseDown = useCallback(async (event: MouseEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    if (event.target !== event.currentTarget) return;
    try {
      await windowRef.current.startDragging();
    } catch (error) {
      console.warn('Frame drag start failed:', error);
    }
  }, []);

  if (showLoadingOverlay) {
    return (
      <div className="h-screen flex items-center justify-center bg-app-dark text-app-text">
        <div className="text-center">
          <div className="animate-spin w-8 h-8 border-2 border-app-accent border-t-transparent rounded-full mx-auto mb-4"></div>
          <div className="text-app-muted">Loading Marlin File Browser...</div>
          <div className="text-xs text-app-muted mt-2">Initializing Tauri backend...</div>
        </div>
      </div>
    );
  }

  return (
    <div className="relative h-screen flex bg-app-dark text-app-text overflow-hidden">
      {/* Sidebar full-height */}
      <div className="h-full p-2" data-tauri-drag-region onMouseDown={handleSidebarFrameMouseDown}>
        <Sidebar />
      </div>

      {/* Content column with path bar + panel */}
      <div className="flex-1 flex flex-col overflow-hidden min-h-0 relative">
        <PathBar />
        <MainPanel />
        <div className="pointer-events-none absolute bottom-0 right-0 z-20">
          <StatusBar />
        </div>
      </div>

      {/* Toast notifications */}
      <Toast />
    </div>
  );
}

export default App;
