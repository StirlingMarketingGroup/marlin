import { create } from 'zustand';
import {
  FileItem,
  ViewPreferences,
  Theme,
  PinnedDirectory,
  DirectoryPreferencesMap,
  GitStatus,
  DirectoryListingResponse,
  StreamingDirectoryResponse,
  DirectoryBatch,
  MetadataBatch,
  LocationCapabilities,
  TrashPathsResponse,
  UndoTrashResponse,
  DeletePathsResponse,
  DeleteItemPayload,
  GoogleAccountInfo,
  SmbServerInfo,
} from '../types';
import { invoke } from '@tauri-apps/api/core';
import { open as openShell } from '@tauri-apps/plugin-shell';
import { ask, message, open as openDialog } from '@tauri-apps/plugin-dialog';
import { getExtractableArchiveFormat, isArchiveFile } from '@/utils/fileTypes';
import { basename } from '@/utils/pathUtils';
import { useToastStore } from './useToastStore';
import { parseGoogleDriveUrl } from '@/utils/googleDriveUrl';

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

  // Check for URI scheme (e.g., gdrive://, smb://, s3://)
  // URI format: scheme://authority/path
  const uriMatch = raw.match(/^([a-zA-Z][a-zA-Z0-9+.-]*):\/\//);
  if (uriMatch) {
    // For URIs, preserve scheme and authority, only normalize path portion
    const scheme = uriMatch[1];
    const afterScheme = raw.slice(uriMatch[0].length);
    const slashIdx = afterScheme.indexOf('/');
    if (slashIdx === -1) {
      // Just scheme://authority with no path
      return raw;
    }
    const authority = afterScheme.slice(0, slashIdx);
    const pathPart = afterScheme.slice(slashIdx);
    // Clean up the path part (remove double slashes, etc.) but keep it simple
    const cleanPath = pathPart.replace(/\/+/g, '/').replace(/\/$/, '') || '/';
    return `${scheme}://${authority}${cleanPath}`;
  }

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

const toFileUri = (path: string): string => {
  const normalized = normalizePath(path);
  if (normalized.startsWith('//')) {
    return `file:${normalized}`;
  }
  if (normalized.startsWith('/')) {
    return `file://${normalized}`;
  }
  return `file:///${normalized}`;
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
  currentLocationRaw: string;
  currentProviderCapabilities?: LocationCapabilities;
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

  // Streaming state
  streamingSessionId: string | null;
  streamingTotalCount: number | null;
  isStreamingComplete: boolean;
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

  // Google Drive accounts
  googleAccounts: GoogleAccountInfo[];

  // SMB network shares
  smbServers: SmbServerInfo[];
  // Pending credential request (when navigating to an SMB path without stored credentials)
  pendingSmbCredentialRequest: { hostname: string; targetPath: string } | null;

  // UI State
  sidebarWidth: number;
  showSidebar: boolean;
  showPreviewPanel: boolean;
  // UI ephemeral
  showZoomSlider: boolean;
  _zoomSliderHideTimer?: number;

  // Filter state
  filterText: string;
  showFilterInput: boolean;

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
  setFilterText: (text: string) => void;
  appendToFilter: (char: string) => void;
  clearFilter: () => void;
  navigateTo: (path: string) => Promise<void>;
  goBack: () => void;
  goForward: () => void;
  canGoBack: () => boolean;
  canGoForward: () => boolean;
  goUp: () => void;
  canGoUp: () => boolean;
  toggleHiddenFiles: (forceValue?: boolean) => Promise<void>;
  toggleFoldersFirst: () => Promise<void>;
  refreshCurrentDirectory: () => Promise<void>;
  refreshCurrentDirectoryStreaming: () => Promise<void>;
  appendStreamingBatch: (batch: DirectoryBatch) => void;
  applyMetadataUpdates: (batch: MetadataBatch) => void;
  updateFileDimensions: (path: string, width: number, height: number) => void;
  cancelDirectoryStream: () => Promise<void>;
  openFile: (file: FileItem) => Promise<void>;
  extractArchive: (file: FileItem) => Promise<boolean>;
  trashSelected: () => Promise<void>;
  deleteSelectedPermanently: () => Promise<void>;
  fetchAppIcon: (path: string, size?: number) => Promise<string | undefined>;
  resetDirectoryPreferences: () => void;
  refreshGitStatus: (options?: { force?: boolean; path?: string }) => Promise<void>;
  invalidateGitStatus: (path?: string) => void;
  // Pinned directories
  loadPinnedDirectories: () => Promise<void>;
  addPinnedDirectory: (path: string, name?: string) => Promise<PinnedDirectory>;
  removePinnedDirectory: (path: string) => Promise<boolean>;
  reorderPinnedDirectories: (paths: string[]) => Promise<void>;
  // Google Drive accounts
  loadGoogleAccounts: () => Promise<void>;
  addGoogleAccount: () => Promise<GoogleAccountInfo>;
  removeGoogleAccount: (email: string) => Promise<void>;
  // SMB network shares
  loadSmbServers: () => Promise<void>;
  addSmbServer: (
    hostname: string,
    username: string,
    password: string,
    domain?: string
  ) => Promise<SmbServerInfo>;
  removeSmbServer: (hostname: string) => Promise<void>;
  setPendingSmbCredentialRequest: (
    request: { hostname: string; targetPath: string } | null
  ) => void;
  // Rename UX
  renameTargetPath?: string;
  renameLoading: boolean;
  setRenameTarget: (path?: string) => void;
  beginRenameSelected: () => void;
  renameFile: (newName: string) => Promise<void>;
  pendingRevealTarget?: string;
  setPendingRevealTarget: (path?: string) => void;
}

export const useAppStore = create<AppState>((set, get) => ({
  // Initial state
  currentPath: '/', // Will be replaced at init
  currentLocationRaw: 'file:///',
  currentProviderCapabilities: undefined,
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
  streamingSessionId: null,
  streamingTotalCount: null,
  isStreamingComplete: true,
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
  googleAccounts: [],
  smbServers: [],
  pendingSmbCredentialRequest: null,

  sidebarWidth: 240,
  showSidebar: true,
  showPreviewPanel: false,
  showZoomSlider: false,
  _zoomSliderHideTimer: undefined,

  filterText: '',
  showFilterInput: false,

  // Actions
  setCurrentPath: (path) => {
    const norm = normalizePath(path);
    set({ currentPath: norm, currentLocationRaw: toFileUri(norm) });
    void get().refreshGitStatus({ path: norm });
  },
  setHomeDir: (path) => set({ homeDir: path }),
  setFiles: (files) =>
    set((state) => {
      const prevByPath = new Map(state.files.map((file) => [file.path, file]));
      const merged = files.map((file) => {
        const prev = prevByPath.get(file.path);
        if (!prev) return file;
        return {
          ...file,
          child_count: file.child_count ?? prev.child_count,
          image_width: file.image_width ?? prev.image_width,
          image_height: file.image_height ?? prev.image_height,
          extension: file.extension ?? prev.extension,
          remote_id: file.remote_id ?? prev.remote_id,
          thumbnail_url: file.thumbnail_url ?? prev.thumbnail_url,
          download_url: file.download_url ?? prev.download_url,
        };
      });

      return { files: merged };
    }),
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

  setFilterText: (text: string) =>
    set({
      filterText: text,
      // Don't change showFilterInput here - only clearFilter should hide the filter
      // This allows the filter to stay visible when empty (user can backspace to clear)
    }),

  appendToFilter: (char: string) =>
    set((state) => ({
      filterText: state.filterText + char,
      showFilterInput: true,
    })),

  clearFilter: () =>
    set({
      filterText: '',
      showFilterInput: false,
    }),

  navigateTo: async (path) => {
    const { pathHistory, historyIndex, googleAccounts } = get();
    const trimmed = path.trim();

    // Check if this is a Google Drive URL
    const gdriveId = parseGoogleDriveUrl(trimmed);
    if (gdriveId) {
      // Resolve the folder ID to a path
      const accountEmails = googleAccounts.map((a) => a.email);
      if (accountEmails.length === 0) {
        console.error('No Google accounts connected');
        await message(
          'No Google Drive accounts are connected. Please add an account in the sidebar first.',
          { title: 'Cannot Open Folder', kind: 'error' }
        );
        return;
      }

      try {
        const [email, resolvedPath, folderName] = await invoke<[string, string, string]>(
          'resolve_gdrive_folder_url',
          { folderId: gdriveId, accounts: accountEmails }
        );
        console.info('[navigateTo] Resolved to folder:', folderName);

        const fullPath = `gdrive://${email}${resolvedPath}`;
        console.info('[navigateTo] Resolved Google Drive URL to:', fullPath);

        const newHistory = [...pathHistory.slice(0, historyIndex + 1), fullPath];
        set({
          currentPath: fullPath,
          currentLocationRaw: fullPath,
          pathHistory: newHistory,
          historyIndex: newHistory.length - 1,
          filterText: '',
          showFilterInput: false,
        });
        // Explicitly refresh the directory for Google Drive URLs
        await get().refreshCurrentDirectory();
        void get().refreshGitStatus({ path: fullPath });
        return;
      } catch (error) {
        console.error('Failed to resolve Google Drive URL:', error);
        await message(
          'Could not access this Google Drive folder. Make sure the folder exists and you have access with one of your connected accounts.',
          { title: 'Cannot Open Folder', kind: 'error' }
        );
        return;
      }
    }

    // Normalize path (handles URIs like gdrive://, smb://, etc.)
    const norm = normalizePath(path);
    // For URIs, use the normalized URI as locationRaw; for file paths, convert to file:// URI
    const isUri = /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(norm);
    const locationRaw = isUri ? norm : toFileUri(norm);

    const newHistory = [...pathHistory.slice(0, historyIndex + 1), norm];
    set({
      currentPath: norm,
      currentLocationRaw: locationRaw,
      pathHistory: newHistory,
      historyIndex: newHistory.length - 1,
      filterText: '',
      showFilterInput: false,
    });

    // For URI paths (smb://, gdrive://, etc.), explicitly refresh the directory
    if (isUri) {
      await get().refreshCurrentDirectory();
    }

    void get().refreshGitStatus({ path: norm });
  },

  goBack: () => {
    const { pathHistory, historyIndex } = get();
    if (historyIndex > 0) {
      const newIndex = historyIndex - 1;
      const newPath = pathHistory[newIndex];
      const locationRaw = newPath.startsWith('gdrive://') ? newPath : toFileUri(newPath);
      set({
        currentPath: newPath,
        currentLocationRaw: locationRaw,
        historyIndex: newIndex,
      });
      void get().refreshGitStatus({ path: newPath });
    }
  },

  goForward: () => {
    const { pathHistory, historyIndex } = get();
    if (historyIndex < pathHistory.length - 1) {
      const newIndex = historyIndex + 1;
      const newPath = pathHistory[newIndex];
      const locationRaw = newPath.startsWith('gdrive://') ? newPath : toFileUri(newPath);
      set({
        currentPath: newPath,
        currentLocationRaw: locationRaw,
        historyIndex: newIndex,
      });
      void get().refreshGitStatus({ path: newPath });
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
    await get().refreshCurrentDirectoryStreaming();
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
    const { currentPath, setLoading, setError } = get();
    try {
      setLoading(true);
      setError(undefined);
      const listing = await invoke<DirectoryListingResponse>('read_directory', {
        path: currentPath,
      });

      // For gdrive:// paths, use the raw URI directly; otherwise normalize the display path
      const isGdrive = listing.location.scheme === 'gdrive';
      const normalized = isGdrive
        ? listing.location.raw
        : normalizePath(listing.location.displayPath || listing.location.path);

      const prevByPath = new Map(get().files.map((file) => [file.path, file]));
      const mergedEntries = listing.entries.map((file) => {
        const prev = prevByPath.get(file.path);
        if (!prev) return file;
        return {
          ...file,
          child_count: file.child_count ?? prev.child_count,
          image_width: file.image_width ?? prev.image_width,
          image_height: file.image_height ?? prev.image_height,
          extension: file.extension ?? prev.extension,
          remote_id: file.remote_id ?? prev.remote_id,
          thumbnail_url: file.thumbnail_url ?? prev.thumbnail_url,
          download_url: file.download_url ?? prev.download_url,
        };
      });

      set((state) => {
        const updatedHistory = [...state.pathHistory];
        if (state.historyIndex >= 0 && state.historyIndex < updatedHistory.length) {
          updatedHistory[state.historyIndex] = normalized;
        }

        return {
          files: mergedEntries,
          currentPath: normalized,
          currentLocationRaw: listing.location.raw,
          currentProviderCapabilities: listing.capabilities,
          pathHistory: updatedHistory,
        };
      });

      void get().refreshGitStatus({ path: listing.location.path, force: true });
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      console.error('❌ refreshCurrentDirectory failed:', msg);

      // Check if this is an SMB "no credentials" error
      if (msg.includes('[SMB_NO_CREDENTIALS]')) {
        // Extract hostname from the current path (smb://hostname/...)
        const smbMatch = currentPath.match(/^smb:\/\/([^/]+)/);
        if (smbMatch) {
          const hostname = smbMatch[1];
          // Set pending credential request to trigger the dialog
          set({
            pendingSmbCredentialRequest: { hostname, targetPath: currentPath },
            error: undefined, // Don't show error, we'll show the dialog instead
          });
          return;
        }
      }

      setError(`Failed to refresh: ${msg}`);
    } finally {
      setLoading(false);
    }
  },

  refreshCurrentDirectoryStreaming: async () => {
    const { currentPath, setLoading, setError } = get();

    // For remote paths (gdrive://), use non-streaming refresh since streaming isn't supported
    if (currentPath?.includes('://')) {
      await get().refreshCurrentDirectory();
      return;
    }

    // Cancel any existing streaming session
    await get().cancelDirectoryStream();

    // Generate session ID on frontend BEFORE calling backend
    // This ensures we can set up the session ID in state before batches arrive
    const sessionId = crypto.randomUUID();

    try {
      setLoading(true);
      setError(undefined);

      // Set the session ID in state BEFORE calling the backend
      // This ensures any batches that arrive via events will be accepted immediately
      set({
        files: [],
        streamingSessionId: sessionId,
        streamingTotalCount: null,
        isStreamingComplete: false,
      });

      // Start streaming - pass the session ID to the backend
      const response = await invoke<StreamingDirectoryResponse>(
        'read_directory_streaming_command',
        { path: currentPath, sessionId }
      );

      // For gdrive:// paths, use the raw URI directly; otherwise normalize the display path
      const isGdrive = response.location.scheme === 'gdrive';
      const normalized = isGdrive
        ? response.location.raw
        : normalizePath(response.location.displayPath || response.location.path);
      const locationRaw = response.location.raw;

      set((state) => {
        const updatedHistory = [...state.pathHistory];
        if (state.historyIndex >= 0 && state.historyIndex < updatedHistory.length) {
          updatedHistory[state.historyIndex] = normalized;
        }

        return {
          currentPath: normalized,
          currentLocationRaw: locationRaw,
          currentProviderCapabilities: response.capabilities,
          pathHistory: updatedHistory,
        };
      });

      void get().refreshGitStatus({ path: response.location.path, force: true });
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      console.error('❌ refreshCurrentDirectoryStreaming failed:', msg);
      setError(`Failed to refresh: ${msg}`);
      set({ isStreamingComplete: true, loading: false, streamingSessionId: null });
    }
    // Note: We don't set loading=false in finally because streaming continues in background
    // It will be set to false when isStreamingComplete becomes true via appendStreamingBatch
  },

  appendStreamingBatch: (batch: DirectoryBatch) => {
    set((state) => {
      // Ignore batches from other sessions
      if (state.streamingSessionId !== batch.sessionId) {
        return {};
      }

      // Append new files
      const newFiles = [...state.files, ...batch.entries];

      return {
        files: newFiles,
        streamingTotalCount: batch.totalCount ?? state.streamingTotalCount,
        isStreamingComplete: batch.isFinal,
        // Show content as soon as first batch arrives (loading=false)
        // The "loading more..." indicator uses isStreamingComplete instead
        loading: false,
      };
    });
  },

  applyMetadataUpdates: (batch: MetadataBatch) => {
    set((state) => {
      // Ignore metadata for other sessions
      if (state.streamingSessionId !== batch.sessionId) {
        return {};
      }

      // Create a map of updates by path for efficient lookup
      const updateMap = new Map(batch.updates.map((u) => [u.path, u]));

      // Update files with metadata
      const updatedFiles = state.files.map((file) => {
        const update = updateMap.get(file.path);
        if (!update) return file;

        return {
          ...file,
          size: update.size,
          modified: update.modified,
          is_directory: update.isDirectory,
          is_symlink: update.isSymlink,
          is_git_repo: update.isGitRepo,
          child_count: update.childCount != null ? update.childCount : file.child_count,
          image_width: update.imageWidth != null ? update.imageWidth : file.image_width,
          image_height: update.imageHeight != null ? update.imageHeight : file.image_height,
        };
      });

      return { files: updatedFiles };
    });
  },

  updateFileDimensions: (path: string, width: number, height: number) => {
    set((state) => {
      const idx = state.files.findIndex((f) => f.path === path);
      if (idx === -1) {
        return {}; // File not found, no update needed
      }
      // Only update if dimensions are not already set
      if (state.files[idx].image_width != null && state.files[idx].image_height != null) {
        return {}; // Already has dimensions
      }
      const updatedFiles = [...state.files];
      updatedFiles[idx] = {
        ...updatedFiles[idx],
        image_width: width,
        image_height: height,
      };
      return { files: updatedFiles };
    });
  },

  cancelDirectoryStream: async () => {
    const { streamingSessionId } = get();
    if (streamingSessionId) {
      try {
        await invoke('cancel_directory_stream', { sessionId: streamingSessionId });
      } catch (err) {
        console.warn('Failed to cancel directory stream:', err);
      }
      set({
        streamingSessionId: null,
        isStreamingComplete: true,
      });
    }
  },

  trashSelected: async () => {
    const state = get();
    const selectedPaths = state.selectedFiles.slice();
    if (!selectedPaths.length) return;

    const fileMap = new Map(state.files.map((file) => [file.path, file]));
    const selectedItems = selectedPaths
      .map((path) => fileMap.get(path))
      .filter((file): file is FileItem => Boolean(file));

    const toastStore = useToastStore.getState();

    try {
      const response = await invoke<TrashPathsResponse>('trash_paths', { paths: selectedPaths });

      if (response.fallbackToPermanent) {
        const count = selectedPaths.length;
        const targetLabel =
          count === 1 ? (selectedItems[0]?.name ?? basename(selectedPaths[0])) : `${count} items`;

        const confirmed = await ask(
          `Unable to move ${targetLabel} to the Trash.\nDelete permanently instead?`,
          {
            title: 'Delete Permanently',
            kind: 'warning',
            okLabel: 'Delete',
            cancelLabel: 'Cancel',
          }
        ).catch((dialogErr) => {
          console.warn('Failed to present permanent delete prompt:', dialogErr);
          return false;
        });

        if (confirmed) {
          await get().deleteSelectedPermanently();
        }
        return;
      }

      await state.refreshCurrentDirectoryStreaming();
      state.setSelectedFiles([]);

      const count = selectedPaths.length;
      const messageText =
        count === 1
          ? `${selectedItems[0]?.name ?? basename(selectedPaths[0])} moved to Trash.`
          : `${count} items moved to Trash.`;

      const undoToken = response.undoToken;
      const isMacPlatform = typeof navigator !== 'undefined' && /mac/i.test(navigator.userAgent);
      const infoMessage =
        undoToken || !isMacPlatform ? messageText : `${messageText} Restore via Finder if needed.`;

      if (undoToken) {
        let toastId = '';
        toastId = toastStore.addToast({
          type: 'info',
          message: infoMessage,
          duration: 8000,
          action: {
            label: 'Undo',
            onClick: () => {
              toastStore.removeToast(toastId);
              void (async () => {
                try {
                  const undoResult = await invoke<UndoTrashResponse>('undo_trash', {
                    token: undoToken,
                  });
                  await state.refreshCurrentDirectoryStreaming();
                  if (Array.isArray(undoResult.restored) && undoResult.restored.length > 0) {
                    state.setSelectedFiles(undoResult.restored);
                  }
                  toastStore.addToast({
                    type: 'success',
                    message:
                      undoResult.restored.length <= 1
                        ? 'Item restored from Trash.'
                        : `${undoResult.restored.length} items restored from Trash.`,
                    duration: 5000,
                  });
                } catch (undoErr) {
                  const undoMessage = undoErr instanceof Error ? undoErr.message : String(undoErr);
                  toastStore.addToast({
                    type: 'error',
                    message: `Undo failed: ${undoMessage}`,
                    duration: 6000,
                  });
                }
              })();
            },
          },
        });
      } else {
        toastStore.addToast({
          type: 'info',
          message: infoMessage,
          duration: 5000,
        });
      }
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      toastStore.addToast({
        type: 'error',
        message: `Unable to move selection to Trash: ${errorMessage}`,
        duration: 6000,
      });
    }
  },

  deleteSelectedPermanently: async () => {
    const state = get();
    const selectedPaths = state.selectedFiles.slice();
    if (!selectedPaths.length) return;

    const fileMap = new Map(state.files.map((file) => [file.path, file]));
    const selectedItems = selectedPaths.map((path) => fileMap.get(path));

    const count = selectedPaths.length;
    const targetLabel =
      count === 1 ? (selectedItems[0]?.name ?? basename(selectedPaths[0])) : `${count} items`;

    let confirmed = false;
    try {
      confirmed = await ask(`Permanently delete ${targetLabel}? This action cannot be undone.`, {
        title: 'Delete Permanently',
        kind: 'warning',
        okLabel: 'Delete',
        cancelLabel: 'Cancel',
      });
    } catch (dialogErr) {
      console.warn('Failed to display permanent delete confirmation:', dialogErr);
    }

    if (!confirmed) {
      return;
    }

    const deleteItems: DeleteItemPayload[] = selectedPaths.map((path) => {
      const item = fileMap.get(path);
      return {
        path,
        name: item?.name ?? basename(path),
        isDirectory: item?.is_directory ?? false,
      };
    });

    const requestId =
      typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function'
        ? crypto.randomUUID()
        : `delete-${Date.now()}`;

    let progressWindowShown = false;
    const showWindow = async () => {
      try {
        await invoke('show_delete_progress_window', {
          requestId,
          items: deleteItems,
        });
        progressWindowShown = true;
      } catch (windowErr) {
        console.warn('Failed to show delete progress window:', windowErr);
      }
    };

    const timerId = window.setTimeout(() => {
      void showWindow();
    }, 500);

    const toastStore = useToastStore.getState();

    try {
      await invoke<DeletePathsResponse>('delete_paths_permanently', {
        paths: selectedPaths,
        requestId,
      });

      state.setSelectedFiles([]);
      await state.refreshCurrentDirectoryStreaming();

      const messageText =
        count === 1
          ? `${selectedItems[0]?.name ?? basename(selectedPaths[0])} deleted permanently.`
          : `${count} items deleted permanently.`;

      toastStore.addToast({
        type: 'success',
        message: messageText,
        duration: 5000,
      });
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      toastStore.addToast({
        type: 'error',
        message: `Unable to delete selection: ${errorMessage}`,
        duration: 6000,
      });
    } finally {
      window.clearTimeout(timerId);
      if (progressWindowShown) {
        void invoke('hide_delete_progress_window').catch((hideErr) => {
          console.warn('Failed to hide delete progress window:', hideErr);
        });
      }
    }
  },

  openFile: async (file) => {
    const toastStore = useToastStore.getState();

    // Handle Google Drive files - need to download first
    if (file.path.startsWith('gdrive://') && file.remote_id) {
      try {
        // Extract email from path: gdrive://email/path
        const pathWithoutScheme = file.path.slice('gdrive://'.length);
        const slashIndex = pathWithoutScheme.indexOf('/');
        const email = slashIndex >= 0 ? pathWithoutScheme.slice(0, slashIndex) : pathWithoutScheme;

        // Download file to temp location
        const tempPath = await invoke<string>('download_gdrive_file', {
          email,
          fileId: file.remote_id,
          fileName: file.name,
        });

        // Open the downloaded file
        try {
          await openShell(tempPath);
          return;
        } catch {
          await invoke('open_path', { path: tempPath });
          return;
        }
      } catch (downloadError) {
        console.error('Failed to download Google Drive file:', downloadError);
        const errorMessage =
          downloadError instanceof Error ? downloadError.message : String(downloadError);
        toastStore.addToast({
          type: 'error',
          message: `Unable to open ${file.name}: ${errorMessage}`,
          duration: 6000,
        });
        return;
      }
    }

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

    // Check if this is a Google Drive file
    const isGdriveFile = archivePath.startsWith('gdrive://');

    if (isGdriveFile) {
      // For Google Drive files: extract and upload back to Google Drive
      if (!file.remote_id) {
        console.error('Google Drive file missing remote_id');
        activeArchiveExtractions.delete(archivePath);
        return false;
      }

      // Extract email and path from gdrive://email/path
      const pathWithoutScheme = archivePath.slice('gdrive://'.length);
      const slashIndex = pathWithoutScheme.indexOf('/');
      const email = slashIndex >= 0 ? pathWithoutScheme.slice(0, slashIndex) : pathWithoutScheme;
      const gdrivePathPart = slashIndex >= 0 ? pathWithoutScheme.slice(slashIndex) : '/';

      // Get parent folder path (remove file name from path)
      const parentPath =
        gdrivePathPart.substring(0, gdrivePathPart.lastIndexOf('/')) || '/My Drive';

      console.info('[extractArchive] Google Drive extraction', {
        email,
        fileId: file.remote_id,
        fileName: file.name,
        parentPath,
      });

      const progressTimer = window.setTimeout(() => {
        void (async () => {
          try {
            await invoke('show_archive_progress_window', {
              fileName: file.name,
              destinationDir: destinationDir,
              format: archiveFormat,
            });
          } catch (error) {
            console.warn('Failed to show archive progress window:', error);
          }
        })();
      }, 500);

      try {
        // Get the folder ID of the current directory
        const destFolderId = await invoke<string>('get_gdrive_folder_id', {
          email,
          path: parentPath,
        });

        console.info('[extractArchive] destination folder ID:', destFolderId);

        // Extract and upload back to Google Drive
        await invoke<string>('extract_gdrive_archive', {
          email,
          fileId: file.remote_id,
          fileName: file.name,
          destinationFolderId: destFolderId,
        });

        console.info('[extractArchive] Google Drive extraction complete');

        // Refresh the current directory to show the new folder
        await state.refreshCurrentDirectory();
        return true;
      } catch (error) {
        console.error('Failed to extract Google Drive archive:', error);
        const errorMsg = error instanceof Error ? error.message : String(error);
        try {
          await message(`Failed to extract ${file.name}: ${errorMsg}`, {
            title: 'Archive Extraction',
            kind: 'error',
          });
        } catch (dialogError) {
          console.warn('Failed to show extraction error dialog:', dialogError);
        }
        return false;
      } finally {
        window.clearTimeout(progressTimer);
        void invoke('hide_archive_progress_window').catch((error) => {
          console.warn('Failed to hide archive progress window:', error);
        });
        activeArchiveExtractions.delete(archivePath);
      }
    }

    // Local file extraction
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
          destinationDir: destinationDir,
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
        archivePath: archivePath,
        destinationDir: destinationDir,
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

      await state.refreshCurrentDirectoryStreaming();
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

  // Google Drive accounts
  loadGoogleAccounts: async () => {
    try {
      const accounts = await invoke<GoogleAccountInfo[]>('get_google_accounts');
      set({ googleAccounts: accounts });
    } catch (error) {
      console.error('Failed to load Google accounts:', error);
      set({ googleAccounts: [] });
    }
  },

  addGoogleAccount: async () => {
    try {
      const newAccount = await invoke<GoogleAccountInfo>('add_google_account');
      set((state) => ({
        googleAccounts: [...state.googleAccounts, newAccount],
      }));
      return newAccount;
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      throw new Error(errorMessage);
    }
  },

  removeGoogleAccount: async (email: string) => {
    try {
      await invoke('remove_google_account', { email });
      set((state) => ({
        googleAccounts: state.googleAccounts.filter((a) => a.email !== email),
      }));
    } catch (error) {
      console.error('Failed to remove Google account:', error);
      throw error;
    }
  },

  // SMB network shares
  loadSmbServers: async () => {
    try {
      const servers = await invoke<SmbServerInfo[]>('get_smb_servers');
      set({ smbServers: servers });
    } catch (error) {
      console.error('Failed to load SMB servers:', error);
      set({ smbServers: [] });
    }
  },

  addSmbServer: async (hostname: string, username: string, password: string, domain?: string) => {
    try {
      const newServer = await invoke<SmbServerInfo>('add_smb_server', {
        hostname,
        username,
        password,
        domain,
      });
      set((state) => ({
        smbServers: [...state.smbServers.filter((s) => s.hostname !== hostname), newServer],
      }));
      return newServer;
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      throw new Error(errorMessage);
    }
  },

  removeSmbServer: async (hostname: string) => {
    try {
      await invoke('remove_smb_server', { hostname });
      set((state) => ({
        smbServers: state.smbServers.filter((s) => s.hostname !== hostname),
      }));
    } catch (error) {
      console.error('Failed to remove SMB server:', error);
      throw error;
    }
  },

  setPendingSmbCredentialRequest: (request) => {
    set({ pendingSmbCredentialRequest: request });
  },

  // Rename state
  renameTargetPath: undefined,
  renameLoading: false,
  pendingRevealTarget: undefined,
  setRenameTarget: (path?: string) => set({ renameTargetPath: path, renameLoading: false }),
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

    // Set loading state to show feedback during rename operation
    set({ renameLoading: true });

    try {
      // Tauri command args expect camelCase keys
      await invoke('rename_file', { fromPath: target, toPath });
      set({ renameTargetPath: undefined, renameLoading: false });
      state.setSelectedFiles([toPath]);
      // Use non-streaming refresh for remote paths (gdrive://)
      if (state.currentPath?.includes('://')) {
        await state.refreshCurrentDirectory();
      } else {
        await state.refreshCurrentDirectoryStreaming();
      }
    } catch (err) {
      set({ renameLoading: false });
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
