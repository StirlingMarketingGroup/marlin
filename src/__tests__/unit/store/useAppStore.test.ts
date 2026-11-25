import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { useAppStore } from '../../../store/useAppStore';
import { useUndoStore } from '../../../store/useUndoStore';
import { useToastStore } from '../../../store/useToastStore';
import { invoke } from '@tauri-apps/api/core';
import { ask, message } from '@tauri-apps/plugin-dialog';
import { open as openShell } from '@tauri-apps/plugin-shell';
import { getPlatform } from '@/hooks/usePlatform';
import { FileItem, NoTrash } from '@/types';
import { buildArchiveUri } from '@/utils/archiveUri';

vi.mock('@tauri-apps/api/core');
vi.mock('@tauri-apps/plugin-dialog');
vi.mock('@tauri-apps/plugin-shell', () => ({
  open: vi.fn(),
}));
// jsdom's user agent on a Mac holds "darwin", which `getPlatform` reads as Windows.
vi.mock('@/hooks/usePlatform', () => ({
  getPlatform: vi.fn(() => ({ isMac: false, isWindows: false, isLinux: true })),
}));

const mockInvoke = vi.mocked(invoke);
const mockAsk = vi.mocked(ask);
const mockMessage = vi.mocked(message);
const mockOpen = vi.mocked(openShell);

function fileItem(overrides: Partial<FileItem> & Pick<FileItem, 'name' | 'path'>): FileItem {
  return {
    size: 1,
    modified: '2026-01-01T00:00:00.000Z',
    is_directory: false,
    is_hidden: false,
    is_symlink: false,
    is_git_repo: false,
    ...overrides,
  };
}

function shellOpenPaths(): string[] {
  return mockOpen.mock.calls.map((call) => call[0]);
}

function newWindowPaths(): string[] {
  return mockInvoke.mock.calls
    .filter((call) => call[0] === 'new_window')
    .map((call) => (call[1] as { path: string }).path);
}

// Answers the named commands, and leaves the rest to the mock already set.
function answerCommands(answers: Record<string, unknown>) {
  const fallback = mockInvoke.getMockImplementation()!;
  mockInvoke.mockImplementation((cmd, payload) =>
    cmd in answers ? Promise.resolve(answers[cmd]) : fallback(cmd, payload)
  );
}

function openPathsCalls(): string[][] {
  return mockInvoke.mock.calls
    .filter((call) => call[0] === 'open_paths')
    .map((call) => (call[1] as { paths: string[] }).paths);
}

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
      pendingRevealTargets: undefined,
      loading: false,
      pathHistory: ['/test'],
      historyIndex: 0,
      navigation: null,
      failedAddress: null,
      appIconCache: {},
      pinnedDirectories: [],
      sidebarWidth: 200,
      showSidebar: true,
      showPreviewPanel: false,
      showZoomSlider: false,
    });

    vi.clearAllMocks();

    // Mock the directory commands
    mockInvoke.mockImplementation((cmd, payload) => {
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
      // Mock streaming command - use the sessionId passed from frontend
      if (cmd === 'read_directory_streaming_command') {
        const args = payload as { path: string; sessionId: string };
        return Promise.resolve({
          sessionId: args.sessionId, // Return the same sessionId passed by frontend
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
      // Mock git status
      if (cmd === 'get_git_status') {
        return Promise.resolve(null);
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

    it('should call refreshCurrentDirectoryStreaming after toggling', async () => {
      await useAppStore.getState().toggleHiddenFiles();

      expect(mockInvoke).toHaveBeenCalledWith(
        'read_directory_streaming_command',
        expect.objectContaining({ path: '/test', sessionId: expect.any(String) })
      );
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

  describe('setFiles', () => {
    it('should preserve existing image dimensions when new listing lacks them', () => {
      useAppStore.setState({
        files: [
          {
            name: 'photo.jpg',
            path: 'smb://server/share/photo.jpg',
            size: 10,
            modified: new Date().toISOString(),
            is_directory: false,
            is_hidden: false,
            is_symlink: false,
            is_git_repo: false,
            extension: 'jpg',
            image_width: 1920,
            image_height: 1080,
          },
        ],
      });

      useAppStore.getState().setFiles([
        {
          name: 'photo.jpg',
          path: 'smb://server/share/photo.jpg',
          size: 12,
          modified: new Date().toISOString(),
          is_directory: false,
          is_hidden: false,
          is_symlink: false,
          is_git_repo: false,
          extension: 'jpg',
          // No image_width/height in the refreshed listing
        },
      ]);

      const file = useAppStore
        .getState()
        .files.find((f) => f.path === 'smb://server/share/photo.jpg');
      expect(file?.image_width).toBe(1920);
      expect(file?.image_height).toBe(1080);
    });

    it('deduplicates an authoritative listing by path', () => {
      const path = '/test/download.zip';
      const baseFile = {
        name: 'download.zip',
        path,
        size: 10,
        modified: '2024-01-01T00:00:00.000Z',
        is_directory: false,
        is_hidden: false,
        is_symlink: false,
        is_git_repo: false,
        extension: 'zip',
      };

      useAppStore.getState().setFiles([baseFile, { ...baseFile, size: 20 }]);

      expect(useAppStore.getState().files).toEqual([{ ...baseFile, size: 20 }]);
    });
  });

  describe('refreshCurrentDirectory', () => {
    it('should preserve existing image dimensions on refresh when backend does not return them', async () => {
      const smbPath = 'smb://server/share/folder';
      useAppStore.setState({
        currentPath: smbPath,
        currentLocationRaw: smbPath,
        files: [
          {
            name: 'photo.jpg',
            path: 'smb://server/share/folder/photo.jpg',
            size: 10,
            modified: new Date().toISOString(),
            is_directory: false,
            is_hidden: false,
            is_symlink: false,
            is_git_repo: false,
            extension: 'jpg',
            image_width: 1920,
            image_height: 1080,
          },
        ],
      });

      mockInvoke.mockImplementationOnce((cmd) => {
        if (cmd === 'read_directory') {
          return Promise.resolve({
            entries: [
              {
                name: 'photo.jpg',
                path: 'smb://server/share/folder/photo.jpg',
                size: 12,
                modified: new Date().toISOString(),
                is_directory: false,
                is_hidden: false,
                is_symlink: false,
                is_git_repo: false,
                extension: 'jpg',
                // No image_width/height
              },
            ],
            location: {
              raw: smbPath,
              scheme: 'smb',
              authority: 'server',
              path: '/share/folder',
              displayPath: smbPath,
            },
            capabilities: {
              scheme: 'smb',
              displayName: 'SMB',
              canRead: true,
              canWrite: true,
              canCreateDirectories: true,
              canDelete: true,
              canRename: true,
              canCopy: true,
              canMove: true,
              supportsWatching: false,
              requiresExplicitRefresh: false,
            },
          });
        }
        return Promise.resolve(undefined);
      });

      await useAppStore.getState().refreshCurrentDirectory();

      const file = useAppStore
        .getState()
        .files.find((f) => f.path === 'smb://server/share/folder/photo.jpg');
      expect(file?.image_width).toBe(1920);
      expect(file?.image_height).toBe(1080);
    });

    it('trusts an unknown SFTP host key and reads the directory again, for a username with an @', async () => {
      const sftpPath = 'sftp://alice@example.com@files.example.net:2222/';
      const tagged =
        '[SFTP_HOST_KEY_UNKNOWN] files.example.net:2222 is not in /Users/me/.ssh/known_hosts. Its ssh-ed25519 key fingerprint is SHA256:abc.';
      useAppStore.setState({
        currentPath: sftpPath,
        currentLocationRaw: sftpPath,
        pathHistory: [sftpPath],
        historyIndex: 0,
        error: undefined,
      });

      let reads = 0;
      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'read_directory') {
          reads += 1;
          if (reads === 1) {
            return Promise.reject(new Error(tagged));
          }
          return Promise.resolve({
            entries: [],
            location: {
              raw: sftpPath,
              scheme: 'sftp',
              authority: 'alice@example.com@files.example.net:2222',
              path: '/',
              displayPath: sftpPath,
            },
            capabilities: {
              scheme: 'sftp',
              displayName: 'SFTP Server',
              canRead: true,
              canWrite: true,
              canCreateDirectories: true,
              canDelete: true,
              canRename: true,
              canCopy: true,
              canMove: true,
              supportsWatching: false,
              requiresExplicitRefresh: true,
            },
          });
        }
        if (cmd === 'get_sftp_unknown_host_key') {
          return Promise.resolve({
            keyType: 'ssh-ed25519',
            fingerprint: 'SHA256:abc',
            knownHostsPath: '/Users/me/.ssh/known_hosts',
          });
        }
        return Promise.resolve(undefined);
      });
      mockAsk.mockResolvedValue(true);

      await useAppStore.getState().refreshCurrentDirectory();

      expect(mockInvoke).toHaveBeenCalledWith('trust_sftp_host_key', {
        hostname: 'files.example.net',
        port: 2222,
        fingerprint: 'SHA256:abc',
      });
      expect(reads).toBe(2);
      expect(useAppStore.getState().error).toBeUndefined();
    });

    it('shows the host key message without the bracket tag when the user declines', async () => {
      const sftpPath = 'sftp://user@example.com/';
      const tagged =
        '[SFTP_HOST_KEY_UNKNOWN] example.com:22 is not in /Users/me/.ssh/known_hosts. Its ssh-ed25519 key fingerprint is SHA256:abc.';
      useAppStore.setState({
        currentPath: sftpPath,
        currentLocationRaw: sftpPath,
        pathHistory: [sftpPath],
        historyIndex: 0,
        error: undefined,
      });

      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'read_directory') {
          return Promise.reject(new Error(tagged));
        }
        if (cmd === 'get_sftp_unknown_host_key') {
          return Promise.resolve({
            keyType: 'ssh-ed25519',
            fingerprint: 'SHA256:abc',
            knownHostsPath: '/Users/me/.ssh/known_hosts',
          });
        }
        return Promise.resolve(undefined);
      });
      mockAsk.mockResolvedValue(false);

      await useAppStore.getState().refreshCurrentDirectory();

      expect(mockInvoke).not.toHaveBeenCalledWith('trust_sftp_host_key', expect.anything());
      expect(useAppStore.getState().error).toBe(
        'example.com:22 is not in /Users/me/.ssh/known_hosts. Its ssh-ed25519 key fingerprint is SHA256:abc.'
      );
    });

    it('drops a listing when the user opened another folder while it was being read', async () => {
      const gdrivePath = 'gdrive://a@example.com/';
      const kept = {
        name: 'kept.txt',
        path: 'gdrive://a@example.com/kept.txt',
        size: 1,
        modified: '2024-01-01T00:00:00.000Z',
        is_directory: false,
        is_hidden: false,
        is_symlink: false,
        is_git_repo: false,
      };
      useAppStore.setState({
        currentPath: gdrivePath,
        currentLocationRaw: gdrivePath,
        files: [kept],
        pathHistory: [gdrivePath],
        historyIndex: 0,
      });

      let resolveListing: (value: unknown) => void = () => {};
      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'read_directory') {
          return new Promise((resolve) => {
            resolveListing = resolve;
          });
        }
        return Promise.resolve(undefined);
      });

      const refresh = useAppStore.getState().refreshCurrentDirectory();
      useAppStore.setState({ currentPath: '/other' });
      resolveListing({
        entries: [
          {
            name: 'other.txt',
            path: 'gdrive://a@example.com/other.txt',
            size: 2,
            modified: '2024-01-01T00:00:00.000Z',
            is_directory: false,
            is_hidden: false,
            is_symlink: false,
            is_git_repo: false,
          },
        ],
        location: {
          raw: gdrivePath,
          scheme: 'gdrive',
          authority: 'a@example.com',
          path: '/',
          displayPath: gdrivePath,
        },
        capabilities: {
          scheme: 'gdrive',
          displayName: 'Google Drive',
          canRead: true,
          canWrite: true,
          canCreateDirectories: true,
          canDelete: true,
          canRename: true,
          canCopy: true,
          canMove: true,
          supportsWatching: false,
          requiresExplicitRefresh: true,
        },
      });
      await refresh;

      expect(useAppStore.getState().currentPath).toBe('/other');
      expect(useAppStore.getState().files).toEqual([kept]);
    });

    it('asks for a sign-in and leaves the pending navigation when a share has no saved sign-in', async () => {
      const smbPath = 'smb://server/share';
      const navigation = {
        path: smbPath,
        before: {
          currentPath: '/test',
          currentLocationRaw: 'file:///test',
          pathHistory: ['/test'],
          historyIndex: 0,
          files: [],
          filterText: '',
          showFilterInput: false,
          listed: true,
        },
      };
      useAppStore.setState({
        currentPath: smbPath,
        currentLocationRaw: smbPath,
        navigation,
        error: undefined,
      });
      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'read_directory') {
          return Promise.reject(new Error('[SMB_NO_CREDENTIALS] server'));
        }
        return Promise.resolve(undefined);
      });

      await expect(useAppStore.getState().refreshCurrentDirectory()).resolves.toEqual({
        outcome: 'signInRequested',
      });

      expect(useAppStore.getState().navigation).toEqual(navigation);
      expect(useAppStore.getState().pendingSmbCredentialRequest).toEqual({
        hostname: 'server',
        targetPath: smbPath,
      });
      expect(useAppStore.getState().error).toBeUndefined();
    });
  });

  describe('Google accounts', () => {
    it('replaces an account that is already listed and appends a new email', async () => {
      useAppStore.setState({
        googleAccounts: [
          { email: 'a@example.com', displayName: 'Old', mountName: 'gdrive-a@example.com' },
        ],
      });

      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'add_google_account') {
          return Promise.resolve({
            email: 'a@example.com',
            displayName: 'New',
            mountName: 'gdrive-a@example.com',
          });
        }
        return Promise.resolve(undefined);
      });

      await useAppStore.getState().addGoogleAccount();

      expect(useAppStore.getState().googleAccounts).toEqual([
        { email: 'a@example.com', displayName: 'New', mountName: 'gdrive-a@example.com' },
      ]);

      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'add_google_account') {
          return Promise.resolve({
            email: 'b@example.com',
            displayName: 'B',
            mountName: 'gdrive-b@example.com',
          });
        }
        return Promise.resolve(undefined);
      });

      await useAppStore.getState().addGoogleAccount();

      expect(useAppStore.getState().googleAccounts).toEqual([
        { email: 'a@example.com', displayName: 'New', mountName: 'gdrive-a@example.com' },
        { email: 'b@example.com', displayName: 'B', mountName: 'gdrive-b@example.com' },
      ]);
    });

    it('sends the email as loginHint when signing in to a named account', async () => {
      mockInvoke.mockResolvedValue({
        email: 'a@example.com',
        mountName: 'gdrive-a@example.com',
      });

      await useAppStore.getState().addGoogleAccount('a@example.com');

      expect(mockInvoke).toHaveBeenCalledWith('add_google_account', {
        loginHint: 'a@example.com',
      });
    });

    it('keeps listed accounts when loading fails and clears the error on the next success', async () => {
      const listed = [
        { email: 'a@example.com', displayName: 'A', mountName: 'gdrive-a@example.com' },
      ];
      useAppStore.setState({ googleAccounts: listed, googleAccountsError: undefined });

      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'get_google_accounts') {
          return Promise.reject(new Error('keychain locked'));
        }
        return Promise.resolve(undefined);
      });

      await useAppStore.getState().loadGoogleAccounts();

      expect(useAppStore.getState().googleAccounts).toEqual(listed);
      expect(useAppStore.getState().googleAccountsError).toBe('keychain locked');

      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'get_google_accounts') {
          return Promise.resolve([
            { email: 'b@example.com', displayName: 'B', mountName: 'gdrive-b@example.com' },
          ]);
        }
        return Promise.resolve(undefined);
      });

      await useAppStore.getState().loadGoogleAccounts();

      expect(useAppStore.getState().googleAccountsError).toBeUndefined();
      expect(useAppStore.getState().googleAccounts).toEqual([
        { email: 'b@example.com', displayName: 'B', mountName: 'gdrive-b@example.com' },
      ]);
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

    it('opens a Windows share path as an smb address', () => {
      useAppStore.getState().navigateTo('\\\\server\\share\\folder one');

      expect(useAppStore.getState().currentPath).toBe('smb://server/share/folder one');
    });

    it('turns backslashes in an smb address into slashes', () => {
      useAppStore.getState().navigateTo('smb://server\\share\\folder\\');

      expect(useAppStore.getState().currentPath).toBe('smb://server/share/folder');
    });

    it('accepts an smb address written with a backslash authority', () => {
      useAppStore.getState().navigateTo('smb:\\\\server\\share');

      expect(useAppStore.getState().currentPath).toBe('smb://server/share');
    });

    it('expands ~ to the home folder', () => {
      useAppStore.setState({ homeDir: '/Users/me' });

      useAppStore.getState().navigateTo('~');
      expect(useAppStore.getState().currentPath).toBe('/Users/me');

      useAppStore.getState().navigateTo('~/Documents');
      expect(useAppStore.getState().currentPath).toBe('/Users/me/Documents');
    });

    it('leaves ~other alone', () => {
      useAppStore.setState({ homeDir: '/Users/me' });

      useAppStore.getState().navigateTo('~other/x');

      expect(useAppStore.getState().currentPath).toBe('~other/x');
    });

    it('keeps a share path native on Windows', () => {
      vi.mocked(getPlatform).mockReturnValueOnce({ isMac: false, isWindows: true, isLinux: false });

      useAppStore.getState().navigateTo('\\\\server\\share');

      expect(useAppStore.getState().currentPath).toBe('//server/share');
    });

    it('drops the password and keeps the user name of a typed sftp address', () => {
      useAppStore.getState().navigateTo('sftp://alice:secret@host/home');
      expect(useAppStore.getState().currentPath).toBe('sftp://alice@host/home');

      useAppStore.getState().navigateTo('sftp://me@corp.com:p@ss@host:2222/x');
      expect(useAppStore.getState().currentPath).toBe('sftp://me@corp.com@host:2222/x');
    });

    it('drops a user name and password from a typed smb address', () => {
      useAppStore.getState().navigateTo('smb://user:secret@server/share');

      expect(useAppStore.getState().currentPath).toBe('smb://server/share');
    });

    it('clears a pending reveal target', () => {
      useAppStore.setState({ pendingRevealTargets: ['/a/b'] });

      useAppStore.getState().navigateTo('/x');

      expect(useAppStore.getState().currentPath).toBe('/x');
      expect(useAppStore.getState().pendingRevealTargets).toBeUndefined();
    });

    it('returns to the folder it left, with its list, filter, and history, when the new one fails', () => {
      const item = fileItem({ name: 'x', path: '/a/x' });
      useAppStore.setState({
        currentPath: '/a',
        currentLocationRaw: 'file:///a',
        pathHistory: ['/a', '/b'],
        historyIndex: 0,
        files: [item],
        filterText: 'x',
        showFilterInput: true,
      });

      useAppStore.getState().navigateTo('/missing');
      useAppStore.setState({ files: [] });
      useAppStore.getState().returnFromFailedNavigation('Path does not exist');

      const state = useAppStore.getState();
      expect(state.currentPath).toBe('/a');
      expect(state.currentLocationRaw).toBe('file:///a');
      expect(state.pathHistory).toEqual(['/a', '/b']);
      expect(state.historyIndex).toBe(0);
      expect(state.files).toEqual([item]);
      expect(state.filterText).toBe('x');
      expect(state.showFilterInput).toBe(true);
      expect(state.navigation).toBeNull();
      expect(state.failedAddress).toBeNull();
    });

    it('hands a typed address back to the path bar with the reason', () => {
      useAppStore.getState().navigateTo('~/Missing ', { typed: true });
      useAppStore.getState().returnFromFailedNavigation('Path does not exist');

      expect(useAppStore.getState().failedAddress).toEqual({
        text: '~/Missing ',
        reason: 'Path does not exist',
      });
    });

    it('records no navigation for the folder already open, which leaves nothing to return to', () => {
      useAppStore.getState().navigateTo('/test');

      const state = useAppStore.getState();
      expect(state.navigation).toBeNull();
      expect(state.pathHistory).toEqual(['/test', '/test']);
      expect(state.historyIndex).toBe(1);
    });

    it('keeps the folder on screen when a second navigation starts before the first loads', () => {
      useAppStore.getState().navigateTo('/b');
      useAppStore.getState().navigateTo('/c');

      const { navigation } = useAppStore.getState();
      expect(navigation?.before.currentPath).toBe('/test');
      expect(navigation?.path).toBe('/c');
    });

    it('records whether the folder left was fully listed', () => {
      useAppStore.setState({ loading: true });
      useAppStore.getState().navigateTo('/b');
      expect(useAppStore.getState().navigation?.before.listed).toBe(false);

      useAppStore.setState({ navigation: null, loading: false, isMetadataComplete: true });
      useAppStore.getState().navigateTo('/c');
      expect(useAppStore.getState().navigation?.before.listed).toBe(true);

      // A folder showing an error is read again when a failed navigation returns to it.
      useAppStore.setState({ navigation: null, error: 'Path does not exist' });
      useAppStore.getState().navigateTo('/d');
      expect(useAppStore.getState().navigation?.before.listed).toBe(false);
    });

    it('a listing of the current folder ends the pending navigation', async () => {
      useAppStore.getState().navigateTo('/b');

      await expect(useAppStore.getState().refreshCurrentDirectory()).resolves.toEqual({
        outcome: 'listed',
      });
      expect(useAppStore.getState().navigation).toBeNull();
    });

    it('another navigation drops a failed address', () => {
      const failed = { text: '/nope', reason: 'Path does not exist' };

      useAppStore.setState({ failedAddress: failed });
      useAppStore.getState().navigateTo('/x');
      expect(useAppStore.getState().failedAddress).toBeNull();

      useAppStore.setState({
        failedAddress: failed,
        pathHistory: ['/a', '/b'],
        historyIndex: 1,
        currentPath: '/b',
      });
      useAppStore.getState().goBack();
      expect(useAppStore.getState().failedAddress).toBeNull();

      useAppStore.setState({
        failedAddress: failed,
        pathHistory: ['/a', '/b'],
        historyIndex: 0,
        currentPath: '/a',
      });
      useAppStore.getState().goForward();
      expect(useAppStore.getState().failedAddress).toBeNull();
    });

    it('drops a Google Drive link that fails after the user opened another folder', async () => {
      let rejectLookup: (reason: unknown) => void = () => {};
      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'resolve_gdrive_folder_url') {
          return new Promise((_, reject) => {
            rejectLookup = reject;
          });
        }
        return Promise.resolve(undefined);
      });
      useAppStore.setState({
        googleAccounts: [{ email: 'me@example.com', mountName: 'me' }],
      });

      const opening = useAppStore
        .getState()
        .navigateTo('https://drive.google.com/drive/folders/1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS', {
          typed: true,
        });
      useAppStore.setState({ currentPath: '/elsewhere' });
      rejectLookup('Not found');
      await opening;

      expect(useAppStore.getState().failedAddress).toBeNull();
      expect(mockMessage).not.toHaveBeenCalled();
      expect(useAppStore.getState().currentPath).toBe('/elsewhere');
    });

    it('a typed Google Drive link that cannot be opened goes back to the path bar, and an untyped one shows a dialog', async () => {
      const link = 'https://drive.google.com/drive/folders/1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS';
      useAppStore.setState({ googleAccounts: [] });

      await useAppStore.getState().navigateTo(link, { typed: true });
      expect(useAppStore.getState().failedAddress?.reason).toContain('No Google Drive accounts');
      expect(mockMessage).not.toHaveBeenCalled();

      await useAppStore.getState().navigateTo(link);
      expect(mockMessage).toHaveBeenCalledTimes(1);
    });

    it('clears a pending reveal target when going back or forward', () => {
      useAppStore.setState({
        pathHistory: ['/a', '/b', '/c'],
        historyIndex: 1,
        currentPath: '/b',
        pendingRevealTargets: ['/b/child'],
      });

      useAppStore.getState().goBack();
      expect(useAppStore.getState().currentPath).toBe('/a');
      expect(useAppStore.getState().pendingRevealTargets).toBeUndefined();

      useAppStore.setState({ pendingRevealTargets: ['/a/child'] });
      useAppStore.getState().goForward();
      expect(useAppStore.getState().currentPath).toBe('/b');
      expect(useAppStore.getState().pendingRevealTargets).toBeUndefined();
    });
  });

  describe('goUp', () => {
    it('opens the parent and reveals the folder that was left', () => {
      useAppStore.setState({ currentPath: '/a/b' });

      useAppStore.getState().goUp();

      expect(useAppStore.getState().currentPath).toBe('/a');
      expect(useAppStore.getState().pendingRevealTargets).toEqual(['/a/b']);
    });

    it('reveals the archive file when leaving an archive root', () => {
      useAppStore.setState({ currentPath: buildArchiveUri('/a/pack.zip', '/') });

      useAppStore.getState().goUp();

      expect(useAppStore.getState().currentPath).toBe('/a');
      expect(useAppStore.getState().pendingRevealTargets).toEqual(['/a/pack.zip']);
    });

    it('reveals the folder left inside an archive', () => {
      const current = buildArchiveUri('/a/pack.zip', '/inner/deep');
      useAppStore.setState({ currentPath: current });

      useAppStore.getState().goUp();

      expect(useAppStore.getState().currentPath).toBe(buildArchiveUri('/a/pack.zip', '/inner'));
      expect(useAppStore.getState().pendingRevealTargets).toEqual([current]);
    });

    it('leaves a nested archive root for the folder that holds the inner archive', () => {
      const innerFile = buildArchiveUri('/a/outer.zip', '/dir/inner.zip');
      useAppStore.setState({ currentPath: buildArchiveUri(innerFile, '/') });

      useAppStore.getState().goUp();

      expect(useAppStore.getState().currentPath).toBe(buildArchiveUri('/a/outer.zip', '/dir'));
      expect(useAppStore.getState().pendingRevealTargets).toEqual([innerFile]);
    });

    it('does nothing at a root', () => {
      for (const path of ['/', 'smb://server', 'smb://server/', 'gdrive://me@example.com/']) {
        useAppStore.setState({ currentPath: path, pendingRevealTargets: ['/keep'] });

        useAppStore.getState().goUp();

        expect(useAppStore.getState().currentPath).toBe(path);
        expect(useAppStore.getState().pendingRevealTargets).toEqual(['/keep']);
      }
    });

    it('goes up from a share to the server', () => {
      useAppStore.setState({ currentPath: 'smb://server/share' });

      useAppStore.getState().goUp();

      expect(useAppStore.getState().currentPath).toBe('smb://server');
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
      is_git_repo: false,
      is_symlink: false,
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
        is_git_repo: false,
        is_symlink: false,
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

  describe('trashSelected', () => {
    const mockFile = {
      path: '/test/file.txt',
      name: 'file.txt',
      is_directory: false,
      size: 100,
      modified: '2024-01-01T00:00:00.000Z',
      extension: 'txt',
      is_hidden: false,
      is_symlink: false,
      is_git_repo: false,
    };

    beforeEach(() => {
      useAppStore.setState({
        files: [mockFile],
        selectedFiles: ['/test/file.txt'],
      });
    });

    it('should trash selected files and clear selection', async () => {
      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'trash_paths') {
          return Promise.resolve({
            trashed: ['/test/file.txt'],
            undoToken: 'test-token',
            noTrash: null,
          });
        }
        if (cmd === 'read_directory') {
          return Promise.resolve([]);
        }
        return Promise.resolve(undefined);
      });

      await useAppStore.getState().trashSelected();

      expect(mockInvoke).toHaveBeenCalledWith('trash_paths', {
        paths: ['/test/file.txt'],
      });
      expect(useAppStore.getState().selectedFiles).toEqual([]);
    });

    it('should do nothing when no files selected', async () => {
      useAppStore.setState({ selectedFiles: [] });

      await useAppStore.getState().trashSelected();

      expect(mockInvoke).not.toHaveBeenCalledWith('trash_paths', expect.anything());
    });

    function answerTrash(noTrash: NoTrash, deleted: string[]) {
      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'trash_paths') {
          return Promise.resolve({ trashed: [], undoToken: null, noTrash });
        }
        if (cmd === 'delete_paths_permanently') {
          return Promise.resolve({ deleted });
        }
        if (cmd === 'read_directory') {
          return Promise.resolve([]);
        }
        return Promise.resolve(undefined);
      });
    }

    it('asks once, in Finder’s words, then deletes when the folder has no Trash', async () => {
      useUndoStore.setState({ stack: [] });
      answerTrash('remote', ['/test/file.txt']);
      mockAsk.mockResolvedValue(true);

      await useAppStore.getState().trashSelected();

      expect(mockAsk).toHaveBeenCalledTimes(1);
      expect(mockAsk).toHaveBeenCalledWith(
        'This item will be deleted immediately. You can’t undo this action.',
        expect.objectContaining({
          title: 'Are you sure you want to delete “file.txt”?',
          okLabel: 'Delete',
        })
      );
      expect(mockInvoke).toHaveBeenCalledWith(
        'delete_paths_permanently',
        expect.objectContaining({ paths: ['/test/file.txt'] })
      );
      expect(useUndoStore.getState().stack).toEqual([]);
    });

    it('leaves the items when the user says no', async () => {
      answerTrash('remote', ['/test/file.txt']);
      mockAsk.mockResolvedValue(false);

      await useAppStore.getState().trashSelected();

      expect(mockInvoke).not.toHaveBeenCalledWith('delete_paths_permanently', expect.anything());
    });

    it('says a Google Drive delete goes to the Google Drive trash', async () => {
      const second = { ...mockFile, path: '/test/other.txt', name: 'other.txt' };
      useAppStore.setState({
        files: [mockFile, second],
        selectedFiles: ['/test/file.txt', '/test/other.txt'],
      });
      useToastStore.setState({ toasts: [] });
      answerTrash('googleDrive', ['/test/file.txt', '/test/other.txt']);
      mockAsk.mockResolvedValue(true);

      await useAppStore.getState().trashSelected();

      expect(mockAsk).toHaveBeenCalledTimes(1);
      expect(mockAsk).toHaveBeenCalledWith(
        '2 items will be moved to the Google Drive trash. Anything in them that has not finished uploading will be lost.',
        expect.objectContaining({
          title: 'Are you sure you want to delete the 2 selected items?',
        })
      );
      expect(useToastStore.getState().toasts.map((toast) => toast.message)).toContain(
        '2 items deleted.'
      );
    });

    it('asks once when the Trash refuses', async () => {
      answerTrash('refused', ['/test/file.txt']);
      mockAsk.mockResolvedValue(true);

      await useAppStore.getState().trashSelected();

      expect(mockAsk).toHaveBeenCalledTimes(1);
      expect(mockAsk).toHaveBeenCalledWith(
        'Unable to move file.txt to the Trash.\nDelete permanently instead?',
        expect.anything()
      );
    });

    it('deletes the paths trash_paths judged, not a selection that changed after', async () => {
      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'trash_paths') {
          useAppStore.setState({ selectedFiles: ['/test/other.txt'] });
          return Promise.resolve({ trashed: [], undoToken: null, noTrash: 'remote' });
        }
        if (cmd === 'delete_paths_permanently') {
          return Promise.resolve({ deleted: ['/test/file.txt'] });
        }
        return Promise.resolve(undefined);
      });
      mockAsk.mockResolvedValue(true);

      await useAppStore.getState().trashSelected();

      expect(mockInvoke).toHaveBeenCalledWith(
        'delete_paths_permanently',
        expect.objectContaining({ paths: ['/test/file.txt'] })
      );
      expect(useAppStore.getState().selectedFiles).toEqual(['/test/other.txt']);
    });
  });

  describe('deleteSelectedPermanently', () => {
    const mockFile = {
      path: '/test/file.txt',
      name: 'file.txt',
      is_directory: false,
      size: 100,
      modified: '2024-01-01T00:00:00.000Z',
      extension: 'txt',
      is_hidden: false,
      is_symlink: false,
      is_git_repo: false,
    };

    beforeEach(() => {
      useAppStore.setState({
        files: [mockFile],
        selectedFiles: ['/test/file.txt'],
      });
      mockAsk.mockResolvedValue(true);
    });

    it('should delete selected files permanently after confirmation', async () => {
      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'delete_paths_permanently') {
          return Promise.resolve({ deleted: ['/test/file.txt'] });
        }
        if (cmd === 'read_directory') {
          return Promise.resolve([]);
        }
        return Promise.resolve(undefined);
      });

      await useAppStore.getState().deleteSelectedPermanently();

      expect(mockAsk).toHaveBeenCalledWith(
        'Permanently delete file.txt? This action cannot be undone.',
        expect.objectContaining({
          title: 'Delete Permanently',
          kind: 'warning',
        })
      );
      expect(mockInvoke).toHaveBeenCalledWith('delete_paths_permanently', expect.anything());
      expect(useAppStore.getState().selectedFiles).toEqual([]);
    });

    it('should not delete when user cancels confirmation', async () => {
      mockAsk.mockResolvedValue(false);

      await useAppStore.getState().deleteSelectedPermanently();

      expect(mockInvoke).not.toHaveBeenCalledWith('delete_paths_permanently', expect.anything());
      expect(useAppStore.getState().selectedFiles).toEqual(['/test/file.txt']);
    });

    it('should do nothing when no files selected', async () => {
      useAppStore.setState({ selectedFiles: [] });

      await useAppStore.getState().deleteSelectedPermanently();

      expect(mockAsk).not.toHaveBeenCalled();
      expect(mockInvoke).not.toHaveBeenCalledWith('delete_paths_permanently', expect.anything());
    });

    it('should handle multiple files in confirmation message', async () => {
      const mockFile2 = { ...mockFile, path: '/test/file2.txt', name: 'file2.txt' };
      useAppStore.setState({
        files: [mockFile, mockFile2],
        selectedFiles: ['/test/file.txt', '/test/file2.txt'],
      });
      mockInvoke.mockImplementation((cmd) => {
        if (cmd === 'delete_paths_permanently') {
          return Promise.resolve({ deleted: ['/test/file.txt', '/test/file2.txt'] });
        }
        if (cmd === 'read_directory') {
          return Promise.resolve([]);
        }
        return Promise.resolve(undefined);
      });

      await useAppStore.getState().deleteSelectedPermanently();

      expect(mockAsk).toHaveBeenCalledWith(
        'Permanently delete 2 items? This action cannot be undone.',
        expect.anything()
      );
    });
  });

  describe('streaming', () => {
    const mockFile1 = {
      path: '/test/file1.txt',
      name: 'file1.txt',
      is_directory: false,
      size: 100,
      modified: '2024-01-01T00:00:00.000Z',
      extension: 'txt',
      is_hidden: false,
      is_symlink: false,
      is_git_repo: false,
    };

    const mockFile2 = {
      path: '/test/file2.txt',
      name: 'file2.txt',
      is_directory: false,
      size: 200,
      modified: '2024-01-02T00:00:00.000Z',
      extension: 'txt',
      is_hidden: false,
      is_symlink: false,
      is_git_repo: false,
    };

    beforeEach(() => {
      useAppStore.setState({
        files: [],
        streamingSessionId: null,
        streamingPath: null,
        streamingTotalCount: null,
        isStreamingComplete: true,
        loading: false,
      });
    });

    describe('appendStreamingBatch', () => {
      it('should append files from a batch with matching session ID', () => {
        useAppStore.setState({
          streamingSessionId: 'session-1',
          files: [mockFile1],
          isStreamingComplete: false,
        });

        useAppStore.getState().appendStreamingBatch({
          sessionId: 'session-1',
          batchIndex: 1,
          entries: [mockFile2],
          isFinal: false,
          totalCount: 100,
        });

        const state = useAppStore.getState();
        expect(state.files).toHaveLength(2);
        expect(state.files[1]).toEqual(mockFile2);
        expect(state.streamingTotalCount).toBe(100);
        expect(state.isStreamingComplete).toBe(false);
      });

      it('should ignore batches from different session IDs', () => {
        useAppStore.setState({
          streamingSessionId: 'session-1',
          files: [mockFile1],
          isStreamingComplete: false,
        });

        useAppStore.getState().appendStreamingBatch({
          sessionId: 'session-2',
          batchIndex: 0,
          entries: [mockFile2],
          isFinal: false,
          totalCount: 50,
        });

        const state = useAppStore.getState();
        expect(state.files).toHaveLength(1);
        expect(state.files[0]).toEqual(mockFile1);
      });

      it('should set isStreamingComplete to true on final batch', () => {
        useAppStore.setState({
          streamingSessionId: 'session-1',
          files: [mockFile1],
          isStreamingComplete: false,
          loading: true,
        });

        useAppStore.getState().appendStreamingBatch({
          sessionId: 'session-1',
          batchIndex: 1,
          entries: [mockFile2],
          isFinal: true,
          totalCount: 2,
        });

        const state = useAppStore.getState();
        expect(state.isStreamingComplete).toBe(true);
        expect(state.loading).toBe(false);
      });

      it('should preserve existing totalCount if batch does not provide one', () => {
        useAppStore.setState({
          streamingSessionId: 'session-1',
          files: [],
          streamingTotalCount: 100,
          isStreamingComplete: false,
        });

        useAppStore.getState().appendStreamingBatch({
          sessionId: 'session-1',
          batchIndex: 0,
          entries: [mockFile1],
          isFinal: false,
          totalCount: null,
        });

        const state = useAppStore.getState();
        expect(state.streamingTotalCount).toBe(100);
      });

      it('should not duplicate a path repeated by overlapping batches', () => {
        useAppStore.setState({
          streamingSessionId: 'session-1',
          files: [mockFile1],
          isStreamingComplete: false,
        });

        useAppStore.getState().appendStreamingBatch({
          sessionId: 'session-1',
          batchIndex: 1,
          entries: [{ ...mockFile1, size: 250 }],
          isFinal: false,
        });

        expect(useAppStore.getState().files).toEqual([{ ...mockFile1, size: 250 }]);
      });
    });

    describe('isMetadataComplete', () => {
      it('is false from the start of a stream until the final metadata batch', async () => {
        mockInvoke.mockResolvedValue({
          sessionId: 'ignored',
          location: { raw: 'file:///test', scheme: 'file', path: '/test', displayPath: '/test' },
          capabilities: {},
        });

        await useAppStore.getState().refreshCurrentDirectoryStreaming();
        const sessionId = useAppStore.getState().streamingSessionId as string;
        expect(useAppStore.getState().isMetadataComplete).toBe(false);

        useAppStore.getState().appendStreamingBatch({
          sessionId,
          batchIndex: 0,
          entries: [],
          isFinal: true,
        });
        expect(useAppStore.getState().isMetadataComplete).toBe(false);

        useAppStore.getState().applyMetadataUpdates({ sessionId, updates: [], isFinal: false });
        expect(useAppStore.getState().isMetadataComplete).toBe(false);

        useAppStore.getState().applyMetadataUpdates({ sessionId, updates: [], isFinal: true });
        expect(useAppStore.getState().isMetadataComplete).toBe(true);
      });

      it('is true again when the stream is cancelled', async () => {
        useAppStore.setState({ streamingSessionId: 'session-1', isMetadataComplete: false });
        mockInvoke.mockResolvedValue(undefined);

        await useAppStore.getState().cancelDirectoryStream();

        expect(useAppStore.getState().isMetadataComplete).toBe(true);
      });
    });

    describe('cancelDirectoryStream', () => {
      it('should cancel active streaming session', async () => {
        useAppStore.setState({
          streamingSessionId: 'session-1',
          streamingPath: '/test/Artwork',
          isStreamingComplete: false,
        });

        mockInvoke.mockResolvedValueOnce(undefined);

        await useAppStore.getState().cancelDirectoryStream();

        expect(mockInvoke).toHaveBeenCalledWith('cancel_directory_stream', {
          sessionId: 'session-1',
        });
        expect(useAppStore.getState().streamingSessionId).toBeNull();
        expect(useAppStore.getState().streamingPath).toBeNull();
        expect(useAppStore.getState().isStreamingComplete).toBe(true);
      });

      it('should do nothing when no active streaming session', async () => {
        useAppStore.setState({
          streamingSessionId: null,
          isStreamingComplete: true,
        });

        await useAppStore.getState().cancelDirectoryStream();

        expect(mockInvoke).not.toHaveBeenCalledWith('cancel_directory_stream', expect.anything());
      });
    });

    describe('refreshCurrentDirectoryStreaming', () => {
      it('should start streaming and set initial state', async () => {
        mockInvoke.mockImplementation((cmd, payload) => {
          if (cmd === 'read_directory_streaming_command') {
            const args = payload as { path: string; sessionId: string };
            return Promise.resolve({
              sessionId: args.sessionId, // Return the frontend-generated sessionId
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
          // Mock git status refresh
          if (cmd === 'get_git_status') {
            return Promise.resolve(null);
          }
          return Promise.resolve(undefined);
        });

        await useAppStore.getState().refreshCurrentDirectoryStreaming();

        const state = useAppStore.getState();
        expect(state.streamingSessionId).toBeTruthy(); // Frontend generates the sessionId
        expect(state.streamingPath).toBe('/test');
        expect(state.files).toEqual([]);
        expect(state.isStreamingComplete).toBe(false);
      });

      it('should cancel previous streaming session before starting new one', async () => {
        useAppStore.setState({
          streamingSessionId: 'old-session',
          isStreamingComplete: false,
        });

        mockInvoke.mockImplementation((cmd, payload) => {
          if (cmd === 'cancel_directory_stream') {
            return Promise.resolve(undefined);
          }
          if (cmd === 'read_directory_streaming_command') {
            const args = payload as { path: string; sessionId: string };
            return Promise.resolve({
              sessionId: args.sessionId, // Return the frontend-generated sessionId
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
          // Mock git status refresh
          if (cmd === 'get_git_status') {
            return Promise.resolve(null);
          }
          return Promise.resolve(undefined);
        });

        await useAppStore.getState().refreshCurrentDirectoryStreaming();

        expect(mockInvoke).toHaveBeenCalledWith('cancel_directory_stream', {
          sessionId: 'old-session',
        });
        // Session ID is generated by frontend, just verify it changed
        const newSessionId = useAppStore.getState().streamingSessionId;
        expect(newSessionId).toBeTruthy();
        expect(newSessionId).not.toBe('old-session');
      });

      it('writes nothing when the user opens another folder while it is read', async () => {
        let resolveRead: (value: unknown) => void = () => {};
        let commandStarted: () => void = () => {};
        const commandPending = new Promise<void>((resolve) => {
          commandStarted = resolve;
        });
        mockInvoke.mockImplementation((cmd) => {
          if (cmd === 'read_directory_streaming_command') {
            commandStarted();
            return new Promise((resolve) => {
              resolveRead = resolve;
            });
          }
          return Promise.resolve(undefined);
        });

        const refresh = useAppStore.getState().refreshCurrentDirectoryStreaming();
        await commandPending;
        useAppStore.setState({ currentPath: '/other' });
        resolveRead({
          location: {
            raw: 'file:///test',
            scheme: 'file',
            path: '/test',
            displayPath: '/test',
          },
          capabilities: {},
        });

        await expect(refresh).resolves.toEqual({ outcome: 'superseded' });
        expect(useAppStore.getState().currentPath).toBe('/other');
        expect(useAppStore.getState().error).toBeUndefined();
      });

      it('leaves the newer stream alone when a superseded read fails', async () => {
        let rejectRead: (reason: unknown) => void = () => {};
        let commandStarted: () => void = () => {};
        const commandPending = new Promise<void>((resolve) => {
          commandStarted = resolve;
        });
        mockInvoke.mockImplementation((cmd) => {
          if (cmd === 'read_directory_streaming_command') {
            commandStarted();
            return new Promise((_, reject) => {
              rejectRead = reject;
            });
          }
          return Promise.resolve(undefined);
        });

        const refresh = useAppStore.getState().refreshCurrentDirectoryStreaming();
        await commandPending;
        useAppStore.setState({
          currentPath: '/other',
          streamingSessionId: 'newer',
          isStreamingComplete: false,
          loading: true,
        });
        rejectRead('Path does not exist');

        await expect(refresh).resolves.toEqual({ outcome: 'superseded' });
        const state = useAppStore.getState();
        expect(state.streamingSessionId).toBe('newer');
        expect(state.isStreamingComplete).toBe(false);
        expect(state.loading).toBe(true);
        expect(state.error).toBeUndefined();
      });
    });
  });

  describe('openSelected', () => {
    it('opens nothing when the selection is empty', async () => {
      const note = fileItem({ name: 'note.txt', path: '/test/note.txt', extension: 'txt' });
      useAppStore.setState({ files: [note], selectedFiles: [] });

      await useAppStore.getState().openSelected();

      expect(mockOpen).not.toHaveBeenCalled();
      expect(mockAsk).not.toHaveBeenCalled();
      expect(newWindowPaths()).toEqual([]);
      expect(useAppStore.getState().currentPath).toBe('/test');
    });

    it('navigates into one selected folder and opens no window', async () => {
      const folder = fileItem({ name: 'Docs', path: '/test/Docs', is_directory: true });
      useAppStore.setState({ files: [folder], selectedFiles: [folder.path] });

      await useAppStore.getState().openSelected();

      const updated = useAppStore.getState();
      expect(updated.currentPath).toBe('/test/Docs');
      expect(updated.pathHistory).toEqual(['/test', '/test/Docs']);
      expect(updated.historyIndex).toBe(1);
      expect(mockOpen).not.toHaveBeenCalled();
      expect(newWindowPaths()).toEqual([]);
    });

    it('opens one plain file with the shell', async () => {
      const note = fileItem({ name: 'note.txt', path: '/test/note.txt', extension: 'txt' });
      useAppStore.setState({ files: [note], selectedFiles: [note.path] });

      await useAppStore.getState().openSelected();

      expect(mockOpen).toHaveBeenCalledTimes(1);
      expect(mockOpen).toHaveBeenCalledWith(note.path);
      expect(useAppStore.getState().currentPath).toBe('/test');
    });

    it('opens two plain files in selection order', async () => {
      const first = fileItem({ name: 'a.txt', path: '/test/a.txt', extension: 'txt' });
      const second = fileItem({ name: 'b.txt', path: '/test/b.txt', extension: 'txt' });
      useAppStore.setState({
        files: [second, first],
        selectedFiles: [first.path, second.path],
      });

      await useAppStore.getState().openSelected();

      expect(shellOpenPaths()).toEqual([first.path, second.path]);
      expect(openPathsCalls()).toEqual([]);
    });

    it('opens a folder and an archive in new windows and a plain file with the shell', async () => {
      const folder = fileItem({ name: 'Docs', path: '/test/Docs', is_directory: true });
      const zip = fileItem({ name: 'bundle.zip', path: '/test/bundle.zip', extension: 'zip' });
      const note = fileItem({ name: 'note.txt', path: '/test/note.txt', extension: 'txt' });
      useAppStore.setState({
        files: [folder, zip, note],
        selectedFiles: [folder.path, zip.path, note.path],
      });

      await useAppStore.getState().openSelected();

      expect(newWindowPaths()).toEqual([folder.path, buildArchiveUri(zip.path, '/')]);
      expect(shellOpenPaths()).toEqual([note.path]);
      expect(useAppStore.getState().currentPath).toBe('/test');
    });

    it('opens nothing when opening more than 10 items is cancelled', async () => {
      const files = Array.from({ length: 11 }, (_, index) =>
        fileItem({
          name: `file-${index}.txt`,
          path: `/test/file-${index}.txt`,
          extension: 'txt',
        })
      );
      useAppStore.setState({
        files,
        selectedFiles: files.map((file) => file.path),
      });
      mockAsk.mockResolvedValueOnce(false);

      await useAppStore.getState().openSelected();

      expect(mockAsk).toHaveBeenCalledWith('Open 11 items?', {
        title: 'Open',
        okLabel: 'Open',
        cancelLabel: 'Cancel',
      });
      expect(mockOpen).not.toHaveBeenCalled();
      expect(newWindowPaths()).toEqual([]);
    });

    it('opens every file when opening more than 10 items is confirmed', async () => {
      const files = Array.from({ length: 11 }, (_, index) =>
        fileItem({
          name: `file-${index}.txt`,
          path: `/test/file-${index}.txt`,
          extension: 'txt',
        })
      );
      useAppStore.setState({
        files,
        selectedFiles: files.map((file) => file.path),
      });
      mockAsk.mockResolvedValueOnce(true);

      await useAppStore.getState().openSelected();

      expect(shellOpenPaths()).toEqual(files.map((file) => file.path));
    });

    it('does not ask before opening 10 files', async () => {
      const files = Array.from({ length: 10 }, (_, index) =>
        fileItem({
          name: `file-${index}.txt`,
          path: `/test/file-${index}.txt`,
          extension: 'txt',
        })
      );
      useAppStore.setState({
        files,
        selectedFiles: files.map((file) => file.path),
      });

      await useAppStore.getState().openSelected();

      expect(mockAsk).not.toHaveBeenCalled();
      expect(shellOpenPaths()).toEqual(files.map((file) => file.path));
    });

    it('skips a selected path that is not in the file list', async () => {
      const first = fileItem({ name: 'a.txt', path: '/test/a.txt', extension: 'txt' });
      const second = fileItem({ name: 'b.txt', path: '/test/b.txt', extension: 'txt' });
      useAppStore.setState({
        files: [first, second],
        selectedFiles: ['/test/missing', first.path, second.path],
      });

      await useAppStore.getState().openSelected();

      expect(shellOpenPaths()).toEqual([first.path, second.path]);
    });

    describe('on macOS', () => {
      beforeEach(() => {
        vi.mocked(getPlatform).mockReturnValue({ isMac: true, isWindows: false, isLinux: false });
      });

      afterEach(() => {
        vi.mocked(getPlatform).mockReturnValue({ isMac: false, isWindows: false, isLinux: true });
      });

      it('opens two plain files in one open_paths call', async () => {
        const first = fileItem({ name: 'a.txt', path: '/test/a.txt', extension: 'txt' });
        const second = fileItem({ name: 'b.txt', path: '/test/b.txt', extension: 'txt' });
        useAppStore.setState({
          files: [second, first],
          selectedFiles: [first.path, second.path],
        });

        await useAppStore.getState().openSelected();

        expect(openPathsCalls()).toEqual([[first.path, second.path]]);
        expect(mockOpen).not.toHaveBeenCalled();
      });

      it('opens a folder and a zip in new windows and two plain files together', async () => {
        const folder = fileItem({ name: 'Docs', path: '/test/Docs', is_directory: true });
        const zip = fileItem({ name: 'bundle.zip', path: '/test/bundle.zip', extension: 'zip' });
        const first = fileItem({ name: 'a.txt', path: '/test/a.txt', extension: 'txt' });
        const second = fileItem({ name: 'b.txt', path: '/test/b.txt', extension: 'txt' });
        useAppStore.setState({
          files: [folder, zip, first, second],
          selectedFiles: [folder.path, zip.path, first.path, second.path],
        });

        await useAppStore.getState().openSelected();

        expect(newWindowPaths()).toEqual([folder.path, buildArchiveUri(zip.path, '/')]);
        expect(openPathsCalls()).toEqual([[first.path, second.path]]);
        expect(mockOpen).not.toHaveBeenCalled();
      });

      it('opens a folder in a new window and one plain file with the shell', async () => {
        const folder = fileItem({ name: 'Docs', path: '/test/Docs', is_directory: true });
        const note = fileItem({ name: 'note.txt', path: '/test/note.txt', extension: 'txt' });
        useAppStore.setState({
          files: [folder, note],
          selectedFiles: [folder.path, note.path],
        });

        await useAppStore.getState().openSelected();

        expect(newWindowPaths()).toEqual([folder.path]);
        expect(shellOpenPaths()).toEqual([note.path]);
        expect(openPathsCalls()).toEqual([]);
      });

      it('opens 11 files together when confirmed', async () => {
        const files = Array.from({ length: 11 }, (_, index) =>
          fileItem({
            name: `file-${index}.txt`,
            path: `/test/file-${index}.txt`,
            extension: 'txt',
          })
        );
        useAppStore.setState({
          files,
          selectedFiles: files.map((file) => file.path),
        });
        mockAsk.mockResolvedValueOnce(true);

        await useAppStore.getState().openSelected();

        expect(openPathsCalls()).toEqual([files.map((file) => file.path)]);
        expect(mockOpen).not.toHaveBeenCalled();
      });

      it('opens nothing when opening 11 items is cancelled', async () => {
        const files = Array.from({ length: 11 }, (_, index) =>
          fileItem({
            name: `file-${index}.txt`,
            path: `/test/file-${index}.txt`,
            extension: 'txt',
          })
        );
        useAppStore.setState({
          files,
          selectedFiles: files.map((file) => file.path),
        });
        mockAsk.mockResolvedValueOnce(false);

        await useAppStore.getState().openSelected();

        expect(openPathsCalls()).toEqual([]);
        expect(mockOpen).not.toHaveBeenCalled();
      });

      it('opens only folders in new windows', async () => {
        const docs = fileItem({ name: 'Docs', path: '/test/Docs', is_directory: true });
        const photos = fileItem({ name: 'Photos', path: '/test/Photos', is_directory: true });
        useAppStore.setState({
          files: [docs, photos],
          selectedFiles: [docs.path, photos.path],
        });

        await useAppStore.getState().openSelected();

        expect(newWindowPaths()).toEqual([docs.path, photos.path]);
        expect(openPathsCalls()).toEqual([]);
        expect(mockOpen).not.toHaveBeenCalled();
      });

      it('hands an archive entry to open_paths as its extracted path', async () => {
        const note = fileItem({ name: 'a.txt', path: '/test/a.txt', extension: 'txt' });
        const entry = fileItem({
          name: 'b.txt',
          path: buildArchiveUri('/test/bundle.zip', '/b.txt'),
          extension: 'txt',
        });
        useAppStore.setState({
          files: [note, entry],
          selectedFiles: [note.path, entry.path],
        });
        mockInvoke.mockImplementation((cmd: string) =>
          Promise.resolve(cmd === 'extract_archive_entry_to_temp' ? '/tmp/x/b.txt' : undefined)
        );

        await useAppStore.getState().openSelected();

        expect(openPathsCalls()).toEqual([['/test/a.txt', '/tmp/x/b.txt']]);
      });

      it('opens the other files when one archive entry cannot be extracted', async () => {
        const first = fileItem({ name: 'a.txt', path: '/test/a.txt', extension: 'txt' });
        const second = fileItem({ name: 'c.txt', path: '/test/c.txt', extension: 'txt' });
        const entry = fileItem({
          name: 'b.txt',
          path: buildArchiveUri('/test/bundle.zip', '/b.txt'),
          extension: 'txt',
        });
        useAppStore.setState({
          files: [first, entry, second],
          selectedFiles: [first.path, entry.path, second.path],
        });
        mockInvoke.mockImplementation((cmd: string) =>
          cmd === 'extract_archive_entry_to_temp'
            ? Promise.reject(new Error('bad zip'))
            : Promise.resolve(undefined)
        );

        await useAppStore.getState().openSelected();

        expect(openPathsCalls()).toEqual([[first.path, second.path]]);
        expect(
          useToastStore
            .getState()
            .toasts.some((item) => item.message === 'Unable to open b.txt: bad zip')
        ).toBe(true);
      });

      it('shows an error toast when open_paths fails', async () => {
        const first = fileItem({ name: 'a.txt', path: '/test/a.txt', extension: 'txt' });
        const second = fileItem({ name: 'b.txt', path: '/test/b.txt', extension: 'txt' });
        useAppStore.setState({
          files: [first, second],
          selectedFiles: [first.path, second.path],
        });
        mockInvoke.mockImplementation((cmd: string) => {
          if (cmd === 'open_paths') {
            return Promise.reject(new Error('open failed'));
          }
          return Promise.resolve(undefined);
        });

        await useAppStore.getState().openSelected();

        const toast = useToastStore
          .getState()
          .toasts.find((item) => item.message === 'Unable to open some items: open failed');
        expect(toast).toMatchObject({ type: 'error', duration: 6000 });
      });
    });
  });

  describe('copySelectedFiles', () => {
    it('shows why a copy failed', async () => {
      const entry = fileItem({
        name: 'b.iso',
        path: buildArchiveUri('/test/bundle.zip', '/b.iso'),
      });
      useAppStore.setState({ files: [entry], selectedFiles: [entry.path] });
      mockInvoke.mockImplementation((cmd: string) =>
        cmd === 'download_and_copy_to_clipboard'
          ? Promise.reject('Archive entry too large')
          : Promise.resolve(undefined)
      );

      await useAppStore.getState().copySelectedFiles();

      expect(useToastStore.getState().toasts.map((item) => item.message)).toContain(
        'Failed to copy files to clipboard: Archive entry too large'
      );
    });
  });

  describe('cutSelectedFiles', () => {
    it('shows why a cut was refused', async () => {
      const entry = fileItem({
        name: 'b.txt',
        path: buildArchiveUri('/test/bundle.zip', '/b.txt'),
      });
      useAppStore.setState({ files: [entry], selectedFiles: [entry.path] });
      mockInvoke.mockImplementation((cmd: string) =>
        cmd === 'download_and_copy_to_clipboard'
          ? Promise.reject('Files inside an archive cannot be cut. Copy them instead.')
          : Promise.resolve(undefined)
      );

      await useAppStore.getState().cutSelectedFiles();

      expect(useToastStore.getState().toasts.map((item) => item.message)).toContain(
        'Failed to cut files to clipboard: Files inside an archive cannot be cut. Copy them instead.'
      );
      expect(useAppStore.getState().clipboardPaths).toEqual([]);
    });
  });

  describe('openItem', () => {
    it('opens a macOS bundle with the shell instead of navigating into it', async () => {
      const bundle = fileItem({
        name: 'Foo.app',
        path: '/test/Foo.app',
        is_directory: true,
        is_symlink: false,
      });

      await useAppStore.getState().openItem(bundle);

      expect(mockOpen).toHaveBeenCalledTimes(1);
      expect(mockOpen).toHaveBeenCalledWith(bundle.path);
      expect(useAppStore.getState().currentPath).toBe('/test');
      expect(useAppStore.getState().pathHistory).toEqual(['/test']);
    });
  });

  describe('revealInOpenFolder', () => {
    it('keeps only the items in the open folder', () => {
      useAppStore.setState({ currentPath: '/test' });

      useAppStore.getState().revealInOpenFolder(['/test/a.txt', '/other/b.txt', '/test/sub/c.txt']);

      expect(useAppStore.getState().pendingRevealTargets).toEqual(['/test/a.txt']);
    });

    it('asks for nothing when every item is in another folder', () => {
      useAppStore.setState({ currentPath: '/test', pendingRevealTargets: undefined });

      useAppStore.getState().revealInOpenFolder(['/other/b.txt']);

      expect(useAppStore.getState().pendingRevealTargets).toBeUndefined();
    });

    it('keeps a file in the root', () => {
      useAppStore.setState({ currentPath: '/' });

      useAppStore.getState().revealInOpenFolder(['/a.txt']);

      expect(useAppStore.getState().pendingRevealTargets).toEqual(['/a.txt']);
    });

    it('matches a Windows path', () => {
      useAppStore.setState({ currentPath: 'C:/Users/me' });

      useAppStore.getState().revealInOpenFolder(['C:\\Users\\me\\a.txt']);

      expect(useAppStore.getState().pendingRevealTargets).toEqual(['C:\\Users\\me\\a.txt']);
    });
  });

  describe('renameFile', () => {
    it('reveals the item under its new name', async () => {
      useAppStore.setState({
        currentPath: '/test',
        renameTargetPath: '/test/old.txt',
        files: [fileItem({ name: 'old.txt', path: '/test/old.txt' })],
        selectedFiles: ['/test/old.txt'],
      });

      await useAppStore.getState().renameFile('new.txt');

      expect(mockInvoke).toHaveBeenCalledWith('rename_file', {
        fromPath: '/test/old.txt',
        toPath: '/test/new.txt',
      });
      expect(useAppStore.getState().pendingRevealTargets).toEqual(['/test/new.txt']);
    });
  });

  describe('pasteFiles', () => {
    // The OS clipboard holds `filePaths`, and the paste writes `pastedPaths`.
    function mockPaste(
      filePaths: string[],
      isCut: boolean,
      pastedPaths: string[],
      errorMessage: string | null = null
    ) {
      answerCommands({
        clipboard_get_contents: { hasFiles: true, hasImage: false, filePaths, isCut },
        paste_items_to_location: {
          pastedPaths,
          skippedCount: filePaths.length - pastedPaths.length,
          errorMessage,
          cancelled: false,
        },
      });
    }

    beforeEach(() => {
      useAppStore.setState({ currentPath: '/test' });
    });

    it('reveals every pasted item', async () => {
      mockPaste(['/src/a.txt', '/src/b.txt'], false, ['/test/a.txt', '/test/b.txt']);

      await useAppStore.getState().pasteFiles();

      expect(useAppStore.getState().pendingRevealTargets).toEqual(['/test/a.txt', '/test/b.txt']);
    });

    it('deletes the remote originals of its own cut once the paste is done', async () => {
      useToastStore.setState({ toasts: [] });
      useAppStore.setState({
        clipboardMode: 'cut',
        clipboardPaths: ['smb://server/share/a.txt'],
        clipboardTempPaths: ['/tmp/marlin/a.txt'],
      });
      mockPaste(['/tmp/marlin/a.txt'], true, ['/test/a.txt']);

      await useAppStore.getState().pasteFiles();

      expect(mockInvoke).toHaveBeenCalledWith('delete_file', { path: 'smb://server/share/a.txt' });
      // A remote cut has no undo, and its toast still says what happened.
      expect(useToastStore.getState().toasts.map((toast) => toast.message)).toEqual([
        'Moved 1 item',
      ]);
    });

    it('keeps the remote originals of its own cut when an item was skipped', async () => {
      useToastStore.setState({ toasts: [] });
      useAppStore.setState({
        clipboardMode: 'cut',
        clipboardPaths: ['smb://server/share/a.txt', 'smb://server/share/b.txt'],
        clipboardTempPaths: ['/tmp/marlin/a.txt', '/tmp/marlin/b.txt'],
      });
      mockPaste(['/tmp/marlin/a.txt', '/tmp/marlin/b.txt'], true, ['/test/a.txt']);

      await useAppStore.getState().pasteFiles();

      expect(mockInvoke).not.toHaveBeenCalledWith('delete_file', expect.anything());
      expect(useToastStore.getState().toasts.map((toast) => toast.message)).toEqual([
        'Copied 1 item (1 skipped)',
      ]);
    });

    it('says Copied when a remote original could not be deleted', async () => {
      useToastStore.setState({ toasts: [] });
      useAppStore.setState({
        clipboardMode: 'cut',
        clipboardPaths: ['smb://server/share/a.txt'],
        clipboardTempPaths: ['/tmp/marlin/a.txt'],
      });
      mockPaste(['/tmp/marlin/a.txt'], true, ['/test/a.txt']);
      const fallback = mockInvoke.getMockImplementation()!;
      mockInvoke.mockImplementation((cmd, payload) =>
        cmd === 'delete_file' ? Promise.reject(new Error('denied')) : fallback(cmd, payload)
      );

      await useAppStore.getState().pasteFiles();

      expect(useToastStore.getState().toasts.map((toast) => toast.message)).toEqual([
        'Cut complete, but 1 source file could not be deleted.',
        'Copied 1 item',
      ]);
    });

    it('shows the skipped count beside the undo toast of a local paste', async () => {
      useToastStore.setState({ toasts: [] });
      mockPaste(['/src/a.txt', '/src/b.txt'], false, ['/test/a.txt']);

      await useAppStore.getState().pasteFiles();

      expect(useToastStore.getState().toasts.map((toast) => toast.message)).toEqual([
        'Copied 1 item',
        '1 item skipped',
      ]);
    });

    it('says why an item failed beside the undo toast of a local paste', async () => {
      useToastStore.setState({ toasts: [] });
      mockPaste(
        ['/src/a.txt', '/src/b.txt'],
        false,
        ['/test/a.txt'],
        'Failed to copy /src/b.txt: No space left on device'
      );

      await useAppStore.getState().pasteFiles();

      expect(useToastStore.getState().toasts.map((toast) => toast.message)).toEqual([
        'Copied 1 item',
        'Failed to copy /src/b.txt: No space left on device',
      ]);
    });

    it('keeps remote files it only copied', async () => {
      useAppStore.setState({
        clipboardMode: 'copy',
        clipboardPaths: ['smb://server/share/a.txt'],
        clipboardTempPaths: ['/tmp/marlin/a.txt'],
      });
      mockPaste(['/tmp/marlin/a.txt'], true, ['/test/a.txt']);

      await useAppStore.getState().pasteFiles();

      expect(mockInvoke).not.toHaveBeenCalledWith('delete_file', expect.anything());
    });

    it('keeps the remote originals of its own cut once another app cut a file', async () => {
      useAppStore.setState({
        clipboardMode: 'cut',
        clipboardPaths: ['smb://server/share/a.txt'],
        clipboardTempPaths: ['/tmp/marlin/a.txt'],
      });
      mockPaste(['/home/me/b.txt'], true, ['/test/b.txt']);

      await useAppStore.getState().pasteFiles();

      expect(mockInvoke).not.toHaveBeenCalledWith('delete_file', expect.anything());
    });
  });

  describe('undo', () => {
    it('reveals what Cmd+Z restores from the Trash', async () => {
      useUndoStore.setState({ stack: [], undoInProgress: false });
      useUndoStore
        .getState()
        .pushUndo(
          { type: 'trash', undoToken: 'token', trashedPaths: ['/test/a.txt', '/test/b.txt'] },
          'Trash 2 items'
        );
      answerCommands({ undo_trash: { restored: ['/test/a.txt', '/test/b.txt'] } });
      useAppStore.setState({ currentPath: '/test' });

      await useUndoStore.getState().executeUndo();

      expect(useAppStore.getState().pendingRevealTargets).toEqual(['/test/a.txt', '/test/b.txt']);
    });

    it("reveals what the trash toast's Undo restores", async () => {
      useToastStore.setState({ toasts: [] });
      useAppStore.setState({
        currentPath: '/test',
        files: [fileItem({ name: 'a.txt', path: '/test/a.txt' })],
        selectedFiles: ['/test/a.txt'],
      });
      answerCommands({
        trash_paths: { trashed: ['/test/a.txt'], undoToken: 'token', noTrash: null },
        undo_trash: { restored: ['/test/a.txt'] },
      });
      await useAppStore.getState().trashSelected();

      const toast = useToastStore.getState().toasts.find((t) => t.action?.label === 'Undo');
      toast!.action!.onClick();

      await vi.waitFor(() =>
        expect(useAppStore.getState().pendingRevealTargets).toEqual(['/test/a.txt'])
      );
    });
  });

  describe('compressSelectedToZip', () => {
    it('reveals the new zip', async () => {
      useAppStore.setState({ currentPath: '/test', selectedFiles: ['/test/a.txt'] });
      answerCommands({ compress_to_zip: { path: '/test/a.zip' } });

      await useAppStore.getState().compressSelectedToZip('a.zip');

      expect(useAppStore.getState().pendingRevealTargets).toEqual(['/test/a.zip']);
    });
  });

  describe('extractArchive', () => {
    it('reveals the folder it extracts into', async () => {
      useAppStore.setState({ currentPath: '/test' });
      answerCommands({ extract_archive: { folderPath: '/test/a', format: 'zip' } });

      await useAppStore
        .getState()
        .extractArchive(fileItem({ name: 'a.zip', path: '/test/a.zip', extension: 'zip' }));

      expect(useAppStore.getState().pendingRevealTargets).toEqual(['/test/a']);
    });
  });
});
