// Tauri API mocks for e2e testing in browser context
// This script is injected before page load via Playwright's addInitScript
// Based on @tauri-apps/api/mocks patterns

// Export constants for use in test files
export const MOCK_HOME_DIR = '/Users/testuser';
export const MOCK_DOWNLOADS_DIR = `${MOCK_HOME_DIR}/Downloads`;

export function getTauriMockScript(): string {
  return `
    (function() {
      const mockHomeDir = '${MOCK_HOME_DIR}';
      const mockDownloadsDir = '${MOCK_DOWNLOADS_DIR}';

      const mockFiles = [
        {
          name: 'Documents',
          path: mockHomeDir + '/Documents',
          is_directory: true,
          is_hidden: false,
          size: 0,
          modified: new Date().toISOString(),
          created: new Date().toISOString(),
          extension: null,
          permissions: { read: true, write: true, execute: true },
        },
        {
          name: 'Downloads',
          path: mockDownloadsDir,
          is_directory: true,
          is_hidden: false,
          size: 0,
          modified: new Date().toISOString(),
          created: new Date().toISOString(),
          extension: null,
          permissions: { read: true, write: true, execute: true },
        },
        {
          name: '.hidden',
          path: mockHomeDir + '/.hidden',
          is_directory: false,
          is_hidden: true,
          size: 100,
          modified: new Date().toISOString(),
          created: new Date().toISOString(),
          extension: null,
          permissions: { read: true, write: true, execute: false },
        },
      ];

      const mockDownloadsFiles = [
        {
          name: 'sample.pdf',
          path: mockDownloadsDir + '/sample.pdf',
          is_directory: false,
          is_hidden: false,
          size: 2048,
          modified: new Date().toISOString(),
          created: new Date().toISOString(),
          extension: 'pdf',
          permissions: { read: true, write: true, execute: false },
        },
        {
          name: 'image.png',
          path: mockDownloadsDir + '/image.png',
          is_directory: false,
          is_hidden: false,
          size: 4096,
          modified: new Date().toISOString(),
          created: new Date().toISOString(),
          extension: 'png',
          permissions: { read: true, write: true, execute: false },
        },
      ];

      function getFilesForPath(path) {
        const normalizedPath = (path || '').replace(/\\\\/g, '/').replace(/\\/+$/, '') || '/';
        if (normalizedPath === mockDownloadsDir || normalizedPath.endsWith('/Downloads')) {
          return mockDownloadsFiles;
        }
        if (normalizedPath === mockHomeDir || normalizedPath === '~' || normalizedPath === '/') {
          return mockFiles;
        }
        return mockFiles;
      }

      // Event listeners registry (based on @tauri-apps/api/mocks pattern)
      const listeners = new Map();
      const callbacks = new Map();

      function registerCallback(callback, once = false) {
        const identifier = window.crypto.getRandomValues(new Uint32Array(1))[0];
        callbacks.set(identifier, (data) => {
          if (once) {
            unregisterCallback(identifier);
          }
          return callback && callback(data);
        });
        return identifier;
      }

      function unregisterCallback(id) {
        callbacks.delete(id);
      }

      function runCallback(id, data) {
        const callback = callbacks.get(id);
        if (callback) {
          callback(data);
        }
      }

      function unregisterListener(event, id) {
        unregisterCallback(id);
        const eventListeners = listeners.get(event);
        if (eventListeners) {
          const index = eventListeners.indexOf(id);
          if (index !== -1) {
            eventListeners.splice(index, 1);
          }
        }
      }

      function handleListen(args) {
        if (!listeners.has(args.event)) {
          listeners.set(args.event, []);
        }
        listeners.get(args.event).push(args.handler);
        return args.handler;
      }

      function handleEmit(args) {
        const eventListeners = listeners.get(args.event) || [];
        for (const handler of eventListeners) {
          runCallback(handler, args);
        }
        return null;
      }

      function handleUnlisten(args) {
        const eventListeners = listeners.get(args.event);
        if (eventListeners) {
          const index = eventListeners.indexOf(args.id);
          if (index !== -1) {
            eventListeners.splice(index, 1);
          }
        }
      }

      // Schedule a directory batch event
      function scheduleDirectoryBatch(sessionId, path) {
        const files = getFilesForPath(path);
        // Use queueMicrotask + setTimeout for proper event timing
        queueMicrotask(() => {
          setTimeout(() => {
            const eventListeners = listeners.get('directory-batch') || [];
            const payload = {
              sessionId,
              entries: files,
              isFinal: true,
              totalCount: files.length,
              location: {
                path: path,
                displayPath: path,
                raw: path,
              },
              capabilities: {
                canCreate: true,
                canDelete: true,
                canRename: true,
              },
            };
            for (const handler of eventListeners) {
              runCallback(handler, { event: 'directory-batch', payload });
            }
          }, 50);
        });
      }

      const commandHandlers = {
        get_home_directory: () => mockHomeDir,
        read_directory: (args) => {
          const path = args?.path || mockHomeDir;
          const files = getFilesForPath(path);
          return {
            entries: files,
            location: { path, displayPath: path, raw: path },
            capabilities: { canCreate: true, canDelete: true, canRename: true },
          };
        },
        read_directory_streaming_command: (args) => {
          const path = args?.path || mockHomeDir;
          const sessionId = args?.sessionId || 'mock-session';
          scheduleDirectoryBatch(sessionId, path);
          return {
            location: { path, displayPath: path, raw: path },
            capabilities: { canCreate: true, canDelete: true, canRename: true },
          };
        },
        cancel_directory_stream: () => undefined,
        read_preferences: () => JSON.stringify({}),
        get_dir_prefs: () => JSON.stringify({}),
        set_dir_prefs: () => undefined,
        set_last_dir: () => undefined,
        get_system_accent_color: () => '#007AFF',
        initialize_thumbnail_service: () => true,
        start_watching_directory: () => undefined,
        stop_watching_directory: () => undefined,
        update_hidden_files_menu: () => undefined,
        update_folders_first_menu: () => undefined,
        update_sort_menu_state: () => undefined,
        get_pinned_directories: () => [],
        load_pinned_directories: () => [],
        get_system_drives: () => [],
        get_disk_usage: () => ({ total: 1000000000, free: 500000000 }),
        enable_drag_detection: () => undefined,
        set_drop_zone: () => undefined,
        'plugin:app|version': () => '0.1.0',
        'plugin:os|platform': () => 'linux', // Return non-macOS to skip permission check
        'plugin:os|version': () => '5.0',
        'plugin:os|os_type': () => 'linux',
        'plugin:os|arch': () => 'x86_64',
        'plugin:os|locale': () => 'en-US',
        'plugin:os|hostname': () => 'test-host',
        'plugin:event|listen': (args) => handleListen(args),
        'plugin:event|emit': (args) => handleEmit(args),
        'plugin:event|unlisten': (args) => handleUnlisten(args),
      };

      // Initialize __TAURI_INTERNALS__ (matches @tauri-apps/api/mocks pattern)
      window.__TAURI_INTERNALS__ = window.__TAURI_INTERNALS__ || {};
      window.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
        console.log('[Tauri Mock] invoke:', cmd, args);
        const handler = commandHandlers[cmd];
        if (handler) {
          return handler(args);
        }
        console.warn('[Tauri Mock] Unhandled command:', cmd);
        return undefined;
      };
      window.__TAURI_INTERNALS__.metadata = {
        currentWindow: { label: 'main' },
        currentWebview: { windowLabel: 'main', label: 'main' },
      };
      window.__TAURI_INTERNALS__.transformCallback = registerCallback;
      window.__TAURI_INTERNALS__.unregisterCallback = unregisterCallback;
      window.__TAURI_INTERNALS__.runCallback = runCallback;
      window.__TAURI_INTERNALS__.callbacks = callbacks;

      // Initialize __TAURI_EVENT_PLUGIN_INTERNALS__ (required for event cleanup)
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = window.__TAURI_EVENT_PLUGIN_INTERNALS__ || {};
      window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener = unregisterListener;

      // Initialize __TAURI_OS_PLUGIN_INTERNALS__ (required for platform())
      window.__TAURI_OS_PLUGIN_INTERNALS__ = window.__TAURI_OS_PLUGIN_INTERNALS__ || {};
      window.__TAURI_OS_PLUGIN_INTERNALS__.platform = 'linux';
      window.__TAURI_OS_PLUGIN_INTERNALS__.version = '5.0';
      window.__TAURI_OS_PLUGIN_INTERNALS__.osType = 'linux';
      window.__TAURI_OS_PLUGIN_INTERNALS__.arch = 'x86_64';
      window.__TAURI_OS_PLUGIN_INTERNALS__.locale = 'en-US';
      window.__TAURI_OS_PLUGIN_INTERNALS__.hostname = 'test-host';

      console.log('[Tauri Mock] Mocks injected successfully');
    })();
  `;
}
