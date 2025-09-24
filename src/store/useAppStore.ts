import { create } from 'zustand';
import {
  FileItem,
  ViewPreferences,
  Theme,
  PinnedDirectory,
  DirectoryPreferencesMap,
  GitStatus,
} from '../types';
import { invoke } from '@tauri-apps/api/core';
import { open as openShell } from '@tauri-apps/plugin-shell';
import { message, open as openDialog } from '@tauri-apps/plugin-dialog';
import { getExtractableArchiveFormat, isArchiveFile } from '@/utils/fileTypes';
import { useToastStore } from './useToastStore';

// Concurrency limiter for app icon generation requests (macOS)
const __iconQueue: Array<() => void> = [];
let __iconActive = 0;
const __ICON_MAX = 4;
const __pumpIconQueue = () => {
  while (__iconActive < __ICON_MAX && __iconQueue.length) {
    const fn = __iconQueue.shift();
    if (fn) fn();
  }
};
const __scheduleIconTask = <T>(task: () => Promise<T>): Promise<T> =>
  new Promise((resolve, reject) => {
    const run = async () => {
      __iconActive++;
      try {
        const result = await task();
        resolve(result);
      } catch (e) {
        reject(e);
      } finally {
        __iconActive--;
        __pumpIconQueue();
      }
    };
    __iconQueue.push(run);
    __pumpIconQueue();
  });

const GIT_STATUS_TTL_MS = 5_000;
let lastOpenWithDefaultPath: string | undefined;
const activeArchiveExtractions = new Set<string>();

interface GitStatusCacheEntry {
  status: GitStatus | null;
  timestamp: number;
}

const normalizePath = (input: string): string => {
  const raw = (input ?? '').trim();
  if (!raw) return '/';

  let path = raw.replace(/\\/g, '/');
  let drivePrefix = '';

  if (/^[A-Za-z]:/.test(path)) {
    drivePrefix = path.slice(0, 2);
    path = path.slice(2);
  }

  let rootPrefix = '';
  if (!drivePrefix && path.startsWith('//')) {
    rootPrefix = '//';
    path = path.slice(2);
  }

  if (drivePrefix) {
    // Ensure we treat drive paths as absolute
    path = path.replace(/^\/+/, '');
    rootPrefix = '/';
  } else if (path.startsWith('/')) {
    rootPrefix = '/';
  }

  path = path.replace(/^\/+/, '');

  const segments: string[] = [];
  for (const segment of path.split('/')) {
    if (!segment || segment === '.') continue;
    if (segment === '..') {
      if (segments.length > 0) {
        segments.pop();
      } else if (!drivePrefix && !rootPrefix) {
        segments.push(segment);
      }
      continue;
    }
    segments.push(segment);
  }

  let normalized = segments.join('/');

  if (drivePrefix) {
    normalized = `${drivePrefix}/${normalized}`;
    if (!segments.length) normalized = `${drivePrefix}/`;
  } else if (rootPrefix) {
    normalized = `${rootPrefix}${normalized}`;
    if (!segments.length) normalized = rootPrefix || '/';
  } else if (!normalized) {
    normalized = '.';
  }

  const driveRoot = drivePrefix ? `${drivePrefix}/` : null;
  if (driveRoot && normalized === driveRoot) {
    return driveRoot;
  }

  if (normalized.length > 1 && normalized.endsWith('/')) {
    normalized = normalized.slice(0, -1);
  }

  if (!normalized) return '/';
  return normalized;
};

const isPathInsideRepo = (path: string, repoRoot: string): boolean => {
  if (!repoRoot) return false;
  const normalizeForCompare = (value: string) =>
    normalizePath(value).replace(/\/+$/, '').toLowerCase();

  const normalizedPath = normalizeForCompare(path);
  let normalizedRoot = normalizeForCompare(repoRoot);

  if (normalizedRoot === '') {
    normalizedRoot = '/';
  }

  if (normalizedPath === normalizedRoot) {
    return true;
  }

  const rootWithSlash = normalizedRoot.endsWith('/') ? normalizedRoot : `${normalizedRoot}/`;
  return normalizedPath.startsWith(rootWithSlash);
};

interface AppState {
  // Navigation
  currentPath: string;
  pathHistory: string[];
  historyIndex: number;
  homeDir?: string;

  // Files
  files: FileItem[];
  selectedFiles: string[];
  selectionAnchor?: string;
  selectionLead?: string;
  shiftBaseSelection?: string[] | null;
  loading: boolean;
  error?: string;
  gitStatus: GitStatus | null;
  gitStatusLoading: boolean;
  gitStatusError?: string;
  gitStatusCache: Record<string, GitStatusCacheEntry>;
  gitStatusRequestId?: string;
  gitStatusRequestPath?: string;

  // Preferences
  globalPreferences: ViewPreferences;
  directoryPreferences: DirectoryPreferencesMap;
  lastPreferenceUpdate: number; // Timestamp to prevent race conditions
  theme: Theme;

  // App icon cache for macOS Applications view
  appIconCache: Record<string, string>;

  // Pinned directories
  pinnedDirectories: PinnedDirectory[];

  // UI State
  sidebarWidth: number;
  showSidebar: boolean;
  showPreviewPanel: boolean;
  // UI ephemeral
  showZoomSlider: boolean;
  _zoomSliderHideTimer?: number;

  // Actions
  setCurrentPath: (path: string) => void;
  setHomeDir: (path: string) => void;
  setFiles: (files: FileItem[]) => void;
  setLoading: (loading: boolean) => void;
  setError: (error?: string) => void;
  setSelectedFiles: (files: string[]) => void;
  setSelectionAnchor: (path?: string) => void;
  setSelectionLead: (path?: string) => void;
  setShiftBaseSelection: (paths: string[] | null) => void;
  updateGlobalPreferences: (preferences: Partial<ViewPreferences>) => void;
  updateDirectoryPreferences: (path: string, preferences: Partial<ViewPreferences>) => void;
  setTheme: (theme: Theme) => void;
  setSidebarWidth: (width: number) => void;
  toggleSidebar: () => void;
  togglePreviewPanel: () => void;
  showZoomSliderNow: () => void;
  hideZoomSliderNow: () => void;
  scheduleHideZoomSlider: (delayMs?: number) => void;
  navigateTo: (path: string) => void;
  goBack: () => void;
  goForward: () => void;
  canGoBack: () => boolean;
  canGoForward: () => boolean;
  goUp: () => void;
  canGoUp: () => boolean;
  toggleHiddenFiles: (forceValue?: boolean) => Promise<void>;
  toggleFoldersFirst: () => Promise<void>;
  refreshCurrentDirectory: () => Promise<void>;
  openFile: (file: FileItem) => Promise<void>;
  extractArchive: (file: FileItem) => Promise<boolean>;
  fetchAppIcon: (path: string, size?: number) => Promise<string | undefined>;
  resetDirectoryPreferences: () => void;
  refreshGitStatus: (options?: { force?: boolean; path?: string }) => Promise<void>;
  invalidateGitStatus: (path?: string) => void;
  // Pinned directories
  loadPinnedDirectories: () => Promise<void>;
  addPinnedDirectory: (path: string, name?: string) => Promise<PinnedDirectory>;
  removePinnedDirectory: (path: string) => Promise<boolean>;
  reorderPinnedDirectories: (paths: string[]) => Promise<void>;
  // Rename UX
  renameTargetPath?: string;
  setRenameTarget: (path?: string) => void;
  beginRenameSelected: () => void;
  renameFile: (newName: string) => Promise<void>;
  pendingRevealTarget?: string;
  setPendingRevealTarget: (path?: string) => void;
}

export const useAppStore = create<AppState>((set, get) => ({
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
  gitStatus: null,
  gitStatusLoading: false,
  gitStatusError: undefined,
  gitStatusCache: {},
  gitStatusRequestId: undefined,
  gitStatusRequestPath: undefined,

  globalPreferences: {
    viewMode: 'list',
    sortBy: 'name',
    sortOrder: 'asc',
    showHidden: false,
    foldersFirst: true,
    gridSize: 120,
  },
  directoryPreferences: {},
  lastPreferenceUpdate: 0,
  theme: 'system',
  appIconCache: {},
  pinnedDirectories: [],

  sidebarWidth: 240,
  showSidebar: true,
  showPreviewPanel: false,
  showZoomSlider: false,
  _zoomSliderHideTimer: undefined,

  // Actions
  setCurrentPath: (path) => {
    const norm = normalizePath(path);
    set({ currentPath: norm });
    void get().refreshGitStatus({ path: norm });
  },
  setHomeDir: (path) => set({ homeDir: path }),
  setFiles: (files) => set({ files }),
  setLoading: (loading) => set({ loading }),
  setError: (error) => set({ error }),
  setSelectedFiles: (files) => {
    set({ selectedFiles: files });
    // Update native menu selection state (ignore errors in dev)
    (async () => {
      try {
        await invoke('update_selection_menu_state', {
          hasSelection: Array.isArray(files) && files.length > 0,
        });
      } catch {
        /* ignore */
      }
    })();
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
      const norm = normalizePath(path);
      return {
        directoryPreferences: {
          ...state.directoryPreferences,
          [norm]: { ...state.directoryPreferences[norm], ...preferences },
        },
        lastPreferenceUpdate: Date.now(),
      };
    }),

  setTheme: (theme) => set({ theme }),
  setSidebarWidth: (width) => set({ sidebarWidth: Math.max(200, Math.min(400, width)) }),
  toggleSidebar: () => set((state) => ({ showSidebar: !state.showSidebar })),
  togglePreviewPanel: () => set((state) => ({ showPreviewPanel: !state.showPreviewPanel })),
  showZoomSliderNow: () =>
    set((state) => {
      if (state._zoomSliderHideTimer) {
        window.clearTimeout(state._zoomSliderHideTimer);
      }
      return { showZoomSlider: true, _zoomSliderHideTimer: undefined };
    }),
  hideZoomSliderNow: () =>
    set((state) => {
      if (state._zoomSliderHideTimer) {
        window.clearTimeout(state._zoomSliderHideTimer);
      }
      return { showZoomSlider: false, _zoomSliderHideTimer: undefined };
    }),
  scheduleHideZoomSlider: (delayMs = 300) =>
    set((state) => {
      if (state._zoomSliderHideTimer) {
        window.clearTimeout(state._zoomSliderHideTimer);
      }
      const id = window.setTimeout(() => {
        useAppStore.getState().hideZoomSliderNow();
      }, delayMs);
      return { _zoomSliderHideTimer: id };
    }),

  navigateTo: (path) => {
    const { pathHistory, historyIndex } = get();
    const norm = normalizePath(path);
    const newHistory = [...pathHistory.slice(0, historyIndex + 1), norm];
    set({
      currentPath: norm,
      pathHistory: newHistory,
      historyIndex: newHistory.length - 1,
    });
    void get().refreshGitStatus({ path: norm });
  },

  goBack: () => {
    const { pathHistory, historyIndex } = get();
    if (historyIndex > 0) {
      const newIndex = historyIndex - 1;
      set({
        currentPath: pathHistory[newIndex],
        historyIndex: newIndex,
      });
      void get().refreshGitStatus({ path: pathHistory[newIndex] });
    }
  },

  goForward: () => {
    const { pathHistory, historyIndex } = get();
    if (historyIndex < pathHistory.length - 1) {
      const newIndex = historyIndex + 1;
      set({
        currentPath: pathHistory[newIndex],
        historyIndex: newIndex,
      });
      void get().refreshGitStatus({ path: pathHistory[newIndex] });
    }
  },

  canGoBack: () => {
    const { historyIndex } = get();
    return historyIndex > 0;
  },

  canGoForward: () => {
    const { pathHistory, historyIndex } = get();
    return historyIndex < pathHistory.length - 1;
  },

  goUp: () => {
    const { currentPath, navigateTo } = get();
    // Handle POSIX and basic Windows paths
    if (!currentPath || currentPath === '/') return;
    // Normalize backslashes to slashes for finding parent
    const normalized = currentPath.replace(/\\/g, '/').replace(/\/+$/g, '') || '/';
    // If after trimming it's root, nothing to do
    if (normalized === '/') return;
    // Windows drive root like C:/
    const driveRootMatch = normalized.match(/^([A-Za-z]:)(\/$)?$/);
    if (driveRootMatch) return;
    const lastSlash = normalized.lastIndexOf('/');
    const parent = lastSlash <= 0 ? '/' : normalized.slice(0, lastSlash) || '/';
    navigateTo(parent);
  },

  canGoUp: () => {
    const { currentPath } = get();
    if (!currentPath || currentPath === '/') return false;
    const normalized = currentPath.replace(/\\/g, '/').replace(/\/+$/g, '') || '/';
    if (normalized === '/') return false;
    const driveRootMatch = normalized.match(/^([A-Za-z]:)(\/$)?$/);
    if (driveRootMatch) return false;
    return true;
  },

  toggleHiddenFiles: async (forceValue?: boolean) => {
    const { currentPath } = get();

    // Get current state fresh each time to avoid stale closure values
    const getCurrentState = () => {
      const state = get();
      const directoryPrefs = state.directoryPreferences[currentPath] ?? {};
      return {
        directoryPrefs,
        globalPrefs: state.globalPreferences,
        currentShowHidden: directoryPrefs.showHidden ?? state.globalPreferences.showHidden,
      };
    };

    const { currentShowHidden } = getCurrentState();
    const newShowHidden = typeof forceValue === 'boolean' ? forceValue : !currentShowHidden;

    // Update directory preference first
    get().updateDirectoryPreferences(currentPath, { showHidden: newShowHidden });

    // Also update global preference as default for new directories
    get().updateGlobalPreferences({ showHidden: newShowHidden });

    // Get fresh state after updates for saving
    const { directoryPrefs } = getCurrentState();
    const updatedDirPrefs: Partial<ViewPreferences> = {
      ...directoryPrefs,
      showHidden: newShowHidden,
    };

    // Save to backend with updated values
    try {
      await invoke('set_dir_prefs', { path: currentPath, prefs: JSON.stringify(updatedDirPrefs) });
    } catch (error) {
      console.warn('Failed to save directory preferences:', error);
    }

    // Sync native menu state
    try {
      await invoke('update_hidden_files_menu', { checked: newShowHidden, source: 'frontend' });
    } catch (error) {
      console.warn('Failed to sync menu:', error);
    }

    // Refresh directory to apply new filter
    await get().refreshCurrentDirectory();
  },

  toggleFoldersFirst: async () => {
    const { globalPreferences, updateGlobalPreferences } = get();
    const newValue = !globalPreferences.foldersFirst;

    // Update preference
    updateGlobalPreferences({ foldersFirst: newValue });

    // Sync the native menu checkbox state
    try {
      await invoke('update_folders_first_menu', { checked: newValue, source: 'frontend' });
    } catch (error) {
      console.warn('Failed to sync folders-first menu state:', error);
    }
  },

  refreshCurrentDirectory: async () => {
    const { currentPath, setFiles, setLoading, setError } = get();
    try {
      setLoading(true);
      setError(undefined);
      const files = await invoke<FileItem[]>('read_directory', { path: currentPath });
      setFiles(files);
      void get().refreshGitStatus({ path: currentPath, force: true });
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      console.error('❌ refreshCurrentDirectory failed:', msg);
      setError(`Failed to refresh: ${msg}`);
    } finally {
      setLoading(false);
    }
  },

  openFile: async (file) => {
    const toastStore = useToastStore.getState();

    try {
      await openShell(file.path);
      return;
    } catch (shellError) {
      console.warn('Plugin shell open failed:', shellError);
    }

    try {
      await invoke('open_path', { path: file.path });
      return;
    } catch (fallbackError) {
      console.warn('Fallback open_path failed:', fallbackError);
    }

    const platform = typeof navigator !== 'undefined' ? navigator.platform.toLowerCase() : '';
    const isMac = platform.includes('mac');
    const isWindows = platform.includes('win');

    let applicationPath: string | undefined;
    try {
      const selection = await openDialog({
        title: 'Select Application',
        multiple: false,
        directory: false,
        defaultPath: lastOpenWithDefaultPath,
        filters: (() => {
          if (isMac) {
            return [{ name: 'Applications', extensions: ['app'] }];
          }
          if (isWindows) {
            return [{ name: 'Applications', extensions: ['exe', 'bat', 'cmd', 'com', 'lnk'] }];
          }
          return undefined;
        })(),
      });

      if (selection === null || selection === undefined) {
        return;
      }

      applicationPath = Array.isArray(selection) ? selection[0] : selection;
      if (!applicationPath) {
        return;
      }
    } catch (dialogError) {
      console.warn('Application selection dialog failed:', dialogError);
      toastStore.addToast({
        type: 'error',
        message: `Unable to choose application for ${file.name}.`,
        duration: 6000,
      });
      return;
    }

    try {
      await invoke('open_path_with', {
        path: file.path,
        applicationPath,
      });

      if (isMac) {
        lastOpenWithDefaultPath = applicationPath;
      } else {
        const slashIndex = Math.max(
          applicationPath.lastIndexOf('/'),
          applicationPath.lastIndexOf('\\')
        );
        lastOpenWithDefaultPath =
          slashIndex >= 0 ? applicationPath.slice(0, slashIndex) : applicationPath;
      }
    } catch (error) {
      console.error('Open with application failed:', error);
      const messageText = error instanceof Error ? error.message : String(error);
      toastStore.addToast({
        type: 'error',
        message: `Unable to open ${file.name}: ${messageText}`,
        duration: 6000,
      });
    }
  },

  extractArchive: async (file) => {
    const state = get();
    const archivePath = file?.path;
    const destinationDir = state.currentPath;

    if (!archivePath || !destinationDir) {
      console.warn('extractArchive called without a valid path or destination');
      return false;
    }

    if (!isArchiveFile(file)) {
      await get().openFile(file);
      return false;
    }

    const archiveFormat = getExtractableArchiveFormat(file);
    if (!archiveFormat) {
      try {
        await message(
          `${file.name} uses an archive format we can't extract yet. Opening with the default application instead.`,
          {
            title: 'Archive Extraction',
            kind: 'info',
          }
        );
      } catch (dialogError) {
        console.warn('Failed to show unsupported archive dialog:', dialogError);
      }
      await get().openFile(file);
      return false;
    }

    if (activeArchiveExtractions.has(archivePath)) {
      console.info('[extractArchive] skip duplicate request', archivePath);
      return false;
    }

    activeArchiveExtractions.add(archivePath);
    console.info(
      '[extractArchive] start',
      archivePath,
      '->',
      destinationDir,
      'format:',
      archiveFormat
    );

    const progressTimer = window.setTimeout(() => {
      void showProgressWindow();
    }, 500);
    let progressWindowShown = false;

    const showProgressWindow = async () => {
      try {
        await invoke('show_archive_progress_window', {
          fileName: file.name,
          destinationDir,
          format: archiveFormat,
        });
        progressWindowShown = true;
      } catch (error) {
        console.warn('Failed to show archive progress window:', error);
      }
    };

    try {
      const result = await invoke<{
        folderPath: string;
        usedSystemFallback?: boolean;
        format?: string;
      }>('extract_archive', {
        archivePath,
        destinationDir,
        formatHint: archiveFormat,
      });

      if (result?.folderPath) {
        set({ pendingRevealTarget: result.folderPath });
      }

      const usedFallback = result?.usedSystemFallback === true;
      const resolvedFormat = result?.format ?? archiveFormat;

      console.info('[extractArchive] success', {
        folderPath: result?.folderPath,
        usedFallback,
        format: resolvedFormat,
      });

      await state.refreshCurrentDirectory();
      return true;
    } catch (error) {
      console.error('Failed to extract archive:', error);

      const messageText = error instanceof Error ? error.message : String(error);
      console.info('[extractArchive] error', { error: messageText, format: archiveFormat });

      try {
        await message(`Unable to extract ${file.name}. Opening with the system handler.`, {
          title: 'Archive Extraction',
          kind: 'warning',
        });
      } catch (dialogError) {
        console.warn('Failed to show extraction error dialog:', dialogError);
      }
      try {
        await openShell(file.path);
        return false;
      } catch (openErr) {
        console.warn('Fallback to system open failed after extraction error:', openErr);
        try {
          await invoke('open_path', { path: file.path });
        } catch (invokeErr) {
          console.error('Fallback open_path after extraction error failed:', invokeErr);
        }
      }
      return false;
    } finally {
      window.clearTimeout(progressTimer);
      if (progressWindowShown) {
        void invoke('hide_archive_progress_window').catch((error) => {
          console.warn('Failed to hide archive progress window:', error);
        });
      }
      activeArchiveExtractions.delete(archivePath);
    }
  },

  refreshGitStatus: async (options) => {
    const { force = false, path } = options ?? {};
    const state = get();
    const targetPath = normalizePath(path ?? state.currentPath ?? '/');

    const now = Date.now();
    const cachedDirect = state.gitStatusCache[targetPath];

    if (!force && state.gitStatusRequestPath && state.gitStatusLoading) {
      const activePath = state.gitStatusRequestPath;
      if (activePath === targetPath) {
        return;
      }
    }

    const cachedByRepo = (() => {
      if (cachedDirect) return cachedDirect;
      for (const entry of Object.values(state.gitStatusCache)) {
        if (!entry.status) continue;
        if (now - entry.timestamp >= GIT_STATUS_TTL_MS) continue;
        if (isPathInsideRepo(targetPath, entry.status.repositoryRoot)) {
          return entry;
        }
      }
      return undefined;
    })();

    if (!force && cachedByRepo && now - cachedByRepo.timestamp < GIT_STATUS_TTL_MS) {
      set({
        gitStatus: cachedByRepo.status,
        gitStatusLoading: false,
        gitStatusError: undefined,
      });
      return;
    }

    const requestId = `${targetPath}:${now}`;

    set({
      gitStatusLoading: true,
      gitStatusError: undefined,
      gitStatusRequestId: requestId,
      gitStatusRequestPath: targetPath,
    });

    try {
      const result = await invoke<GitStatus | null>('get_git_status', { path: targetPath });
      const entry: GitStatusCacheEntry = { status: result, timestamp: Date.now() };
      const rootKey = result ? normalizePath(result.repositoryRoot) : null;

      set((current) => {
        if (current.gitStatusRequestId !== requestId) {
          return {};
        }

        const nextCache = { ...current.gitStatusCache, [targetPath]: entry };
        if (rootKey) {
          nextCache[rootKey] = entry;
        }

        return {
          gitStatus: result,
          gitStatusLoading: false,
          gitStatusError: undefined,
          gitStatusCache: nextCache,
          gitStatusRequestId: undefined,
          gitStatusRequestPath: undefined,
        };
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      set((current) => {
        if (current.gitStatusRequestId !== requestId) {
          return {};
        }

        return {
          gitStatusLoading: false,
          gitStatusError: message,
          gitStatusRequestId: undefined,
          gitStatusRequestPath: undefined,
        };
      });
    }
  },

  invalidateGitStatus: (path) => {
    if (!path) {
      set({ gitStatusCache: {}, gitStatus: null, gitStatusError: undefined });
      return;
    }

    const normalized = normalizePath(path);

    set((state) => {
      const next = { ...state.gitStatusCache };
      delete next[normalized];

      if (state.gitStatus && isPathInsideRepo(normalized, state.gitStatus.repositoryRoot)) {
        const rootKey = normalizePath(state.gitStatus.repositoryRoot);
        delete next[rootKey];
      } else {
        for (const [key, entry] of Object.entries(state.gitStatusCache)) {
          if (!entry.status) continue;
          if (isPathInsideRepo(normalized, entry.status.repositoryRoot)) {
            delete next[key];
          }
        }
      }

      return { gitStatusCache: next };
    });
  },

  fetchAppIcon: async (path: string, size = 128) => {
    const { appIconCache } = get();
    if (appIconCache[path]) return appIconCache[path];
    return __scheduleIconTask(async () => {
      try {
        const dataUrl = await invoke<string>('get_application_icon', { path, size });
        // dataUrl is already a data:image/png;base64,... string on macOS
        set((state) => ({ appIconCache: { ...state.appIconCache, [path]: dataUrl } }));
        return dataUrl;
      } catch (error) {
        console.warn('Failed to fetch application icon:', error);
        return undefined;
      }
    });
  },

  resetDirectoryPreferences: () => {
    set({ directoryPreferences: {} });
  },

  loadPinnedDirectories: async () => {
    try {
      const pinnedDirs = await invoke<PinnedDirectory[]>('get_pinned_directories');
      set({ pinnedDirectories: pinnedDirs });
    } catch (error) {
      console.error('Failed to load pinned directories:', error);
      set({ pinnedDirectories: [] });
    }
  },

  addPinnedDirectory: async (path: string, name?: string) => {
    try {
      const newPin = await invoke<PinnedDirectory>('add_pinned_directory', { path, name });
      set((state) => ({
        pinnedDirectories: [...state.pinnedDirectories, newPin],
      }));
      return newPin;
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      throw new Error(errorMessage);
    }
  },

  removePinnedDirectory: async (path: string) => {
    try {
      const removed = await invoke<boolean>('remove_pinned_directory', { path });
      if (removed) {
        set((state) => ({
          pinnedDirectories: state.pinnedDirectories.filter((p) => p.path !== path),
        }));
      }
      return removed;
    } catch (error) {
      console.error('Failed to remove pinned directory:', error);
      return false;
    }
  },

  reorderPinnedDirectories: async (paths: string[]) => {
    try {
      await invoke('reorder_pinned_directories', { paths });
      // Reorder the local state to match
      set((state) => {
        const reordered = paths
          .map((path) => state.pinnedDirectories.find((p) => p.path === path))
          .filter(Boolean) as PinnedDirectory[];

        // Add any pins that weren't in the reorder list
        const missing = state.pinnedDirectories.filter((p) => !paths.includes(p.path));

        return { pinnedDirectories: [...reordered, ...missing] };
      });
    } catch (error) {
      console.error('Failed to reorder pinned directories:', error);
      throw error;
    }
  },

  // Rename state
  renameTargetPath: undefined,
  pendingRevealTarget: undefined,
  setRenameTarget: (path?: string) => set({ renameTargetPath: path }),
  beginRenameSelected: () => {
    const { selectedFiles } = get();
    if (!selectedFiles || selectedFiles.length === 0) return;
    set({ renameTargetPath: selectedFiles[0] });
  },
  renameFile: async (newName: string) => {
    const state = get();
    const target = state.renameTargetPath;
    if (!target) return;
    const trimmed = (newName || '').trim();
    if (!trimmed) {
      set({ renameTargetPath: undefined });
      return;
    }
    if (/[\\/]/.test(trimmed)) {
      // Invalid characters for a single path segment
      try {
        await message('Name cannot contain slashes.', {
          title: 'Invalid Name',
          kind: 'warning',
          okLabel: 'OK',
        });
      } catch (error) {
        console.warn('Failed to show invalid-name warning:', error);
      }
      return;
    }

    const sep = target.includes('\\') ? '\\' : '/';
    const lastSep = Math.max(target.lastIndexOf('/'), target.lastIndexOf('\\'));
    const parent = lastSep >= 0 ? target.slice(0, lastSep) : state.currentPath;
    const toPath = parent ? `${parent}${sep}${trimmed}` : trimmed;
    try {
      // Tauri command args expect camelCase keys
      await invoke('rename_file', { fromPath: target, toPath });
      set({ renameTargetPath: undefined });
      state.setSelectedFiles([toPath]);
      await state.refreshCurrentDirectory();
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      try {
        await message(`Failed to rename:\n${msg}`, {
          title: 'Rename Error',
          kind: 'error',
          okLabel: 'OK',
        });
      } catch (dialogError) {
        console.error('Failed to show rename error dialog:', dialogError);
      }
    }
  },
  setPendingRevealTarget: (path?: string) => set({ pendingRevealTarget: path }),
}));
