import { test, expect, Page } from '@playwright/test';
import { getTauriMockScript, MOCK_HOME_DIR } from './tauri-mocks';

// Mock Google Drive data
const MOCK_EMAIL = 'test@example.com';
const MOCK_GDRIVE_ROOT = `gdrive://${MOCK_EMAIL}`;
const MOCK_SHARED_DRIVE_NAME = 'Test Team Drive';
const MOCK_SHARED_DRIVE_ID = 'shared-drive-123';
const MOCK_FOLDER_ID = 'folder-456';
const MOCK_FOLDER_NAME = 'Test Images';

// Mock files in the test folder (mostly images for smart defaults testing)
const mockGdriveImageFiles = [
  { name: 'photo1.jpg', extension: 'jpg' },
  { name: 'photo2.png', extension: 'png' },
  { name: 'photo3.jpeg', extension: 'jpeg' },
  { name: 'photo4.webp', extension: 'webp' },
  { name: 'screenshot.png', extension: 'png' },
  { name: 'document.pdf', extension: 'pdf' },
].map((f, i) => ({
  name: f.name,
  path: `${MOCK_GDRIVE_ROOT}/Shared drives/${MOCK_SHARED_DRIVE_NAME}/${MOCK_FOLDER_NAME}/${f.name}`,
  is_directory: false,
  is_hidden: false,
  size: 1024 * (i + 1),
  modified: new Date().toISOString(),
  extension: f.extension,
  remote_id: `file-${i}`,
  thumbnail_url: null,
  download_url: null,
}));

// Generate script that adds Google Drive mock handlers
function getGdriveMockScript(): string {
  return `
    (function() {
      const mockEmail = '${MOCK_EMAIL}';
      const mockGdriveRoot = '${MOCK_GDRIVE_ROOT}';
      const mockSharedDriveName = '${MOCK_SHARED_DRIVE_NAME}';
      const mockSharedDriveId = '${MOCK_SHARED_DRIVE_ID}';
      const mockFolderId = '${MOCK_FOLDER_ID}';
      const mockFolderName = '${MOCK_FOLDER_NAME}';

      const mockGoogleAccounts = [
        { email: mockEmail, displayName: null }
      ];

      const mockGdriveVirtualFolders = [
        { name: 'My Drive', path: mockGdriveRoot + '/My Drive', is_directory: true, is_hidden: false, size: 0, modified: new Date().toISOString(), extension: null },
        { name: 'Shared drives', path: mockGdriveRoot + '/Shared drives', is_directory: true, is_hidden: false, size: 0, modified: new Date().toISOString(), extension: null },
        { name: 'Shared with me', path: mockGdriveRoot + '/Shared with me', is_directory: true, is_hidden: false, size: 0, modified: new Date().toISOString(), extension: null },
        { name: 'Starred', path: mockGdriveRoot + '/Starred', is_directory: true, is_hidden: false, size: 0, modified: new Date().toISOString(), extension: null },
        { name: 'Recent', path: mockGdriveRoot + '/Recent', is_directory: true, is_hidden: false, size: 0, modified: new Date().toISOString(), extension: null },
      ];

      const mockSharedDrives = [
        { name: mockSharedDriveName, path: mockGdriveRoot + '/Shared drives/' + mockSharedDriveName, is_directory: true, is_hidden: false, size: 0, modified: new Date().toISOString(), extension: null, remote_id: mockSharedDriveId },
      ];

      const mockSharedDriveContents = [
        { name: mockFolderName, path: mockGdriveRoot + '/Shared drives/' + mockSharedDriveName + '/' + mockFolderName, is_directory: true, is_hidden: false, size: 0, modified: new Date().toISOString(), extension: null, remote_id: mockFolderId },
      ];

      const mockImageFiles = ${JSON.stringify(mockGdriveImageFiles)};

      function getGdriveFilesForPath(path) {
        const normalizedPath = (path || '').replace(/\\\\/g, '/');

        // Root of Google Drive account
        if (normalizedPath === mockGdriveRoot + '/' || normalizedPath === mockGdriveRoot) {
          return mockGdriveVirtualFolders;
        }

        // Shared drives listing
        if (normalizedPath === mockGdriveRoot + '/Shared drives') {
          return mockSharedDrives;
        }

        // Specific shared drive root
        if (normalizedPath === mockGdriveRoot + '/Shared drives/' + mockSharedDriveName) {
          return mockSharedDriveContents;
        }

        // Test folder with images
        if (normalizedPath.includes(mockFolderName)) {
          return mockImageFiles;
        }

        return [];
      }

      // Store original invoke - handle case where it may not be set yet
      const originalInvoke = window.__TAURI_INTERNALS__?.invoke || (async () => undefined);

      // Create a unified invoke handler that checks GDrive commands first
      const gdriveInvoke = async (cmd, args) => {
        console.log('[GDrive Mock] invoke:', cmd, args);

        // Google Drive specific commands
        if (cmd === 'get_google_accounts') {
          console.log('[GDrive Mock] returning accounts:', mockGoogleAccounts);
          return mockGoogleAccounts;
        }

        if (cmd === 'resolve_gdrive_folder_url') {
          // Simulate resolving a folder URL to a path
          return [mockEmail, '/Shared drives/' + mockSharedDriveName + '/' + mockFolderName, mockFolderName];
        }

        if (cmd === 'read_directory_streaming_command' && args?.path?.startsWith('gdrive://')) {
          const path = args.path;
          const sessionId = args?.sessionId || 'mock-session';
          const files = getGdriveFilesForPath(path);

          // Schedule directory batch event
          queueMicrotask(() => {
            setTimeout(() => {
              const eventListeners = window.__TAURI_INTERNALS__?.callbacks;
              if (eventListeners) {
                // Find and call directory-batch listeners
                eventListeners.forEach((callback) => {
                  try {
                    callback({
                      event: 'directory-batch',
                      payload: {
                        sessionId,
                        entries: files,
                        isFinal: true,
                        totalCount: files.length,
                        location: {
                          path: path.replace(mockGdriveRoot, ''),
                          displayPath: path.replace('gdrive://', ''),
                          raw: path,
                          scheme: 'gdrive',
                          authority: mockEmail
                        },
                        capabilities: {
                          scheme: 'gdrive',
                          displayName: 'Google Drive',
                          canCreate: true,
                          canDelete: true,
                          canRename: true,
                          canRead: true,
                          canWrite: true,
                          canCopy: true,
                          canMove: true,
                          supportsWatching: false,
                          requiresExplicitRefresh: true
                        },
                      }
                    });
                  } catch (e) {
                    // Ignore callback errors
                  }
                });
              }
            }, 50);
          });

          return {
            location: {
              path: path.replace(mockGdriveRoot, ''),
              displayPath: path.replace('gdrive://', ''),
              raw: path,
              scheme: 'gdrive',
              authority: mockEmail
            },
            capabilities: {
              scheme: 'gdrive',
              displayName: 'Google Drive',
              canCreate: true,
              canDelete: true,
              canRename: true,
              canRead: true,
              canWrite: true,
              canCopy: true,
              canMove: true,
              supportsWatching: false,
              requiresExplicitRefresh: true
            },
          };
        }

        if (cmd === 'read_directory' && args?.path?.startsWith('gdrive://')) {
          const path = args.path;
          const files = getGdriveFilesForPath(path);
          return {
            entries: files,
            location: {
              path: path.replace(mockGdriveRoot, ''),
              displayPath: path.replace('gdrive://', ''),
              raw: path,
              scheme: 'gdrive',
              authority: mockEmail
            },
            capabilities: {
              scheme: 'gdrive',
              displayName: 'Google Drive',
              canCreate: true,
              canDelete: true,
              canRename: true,
              canRead: true,
              canWrite: true,
              canCopy: true,
              canMove: true,
              supportsWatching: false,
              requiresExplicitRefresh: true
            },
          };
        }

        // Fall back to original handler
        return originalInvoke(cmd, args);
      };

      // Ensure __TAURI_INTERNALS__ exists and set our invoke
      window.__TAURI_INTERNALS__ = window.__TAURI_INTERNALS__ || {};
      window.__TAURI_INTERNALS__.invoke = gdriveInvoke;

      console.log('[GDrive Mock] Google Drive mocks installed');
    })();
  `;
}

test.describe('Google Drive Integration', () => {
  test.beforeEach(async ({ page }) => {
    // Inject base Tauri mocks first
    await page.addInitScript(getTauriMockScript());
    // Then add Google Drive specific mocks
    await page.addInitScript(getGdriveMockScript());
    await page.goto('/');

    // Wait for app to initialize
    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });
  });

  test('should show Google Drive accounts in sidebar', async ({ page }) => {
    // Look for the Google account in the sidebar
    await expect(page.getByText(MOCK_EMAIL)).toBeVisible({ timeout: 5000 });
  });

  test('should navigate to Google Drive root', async ({ page }) => {
    // Click on the Google account in sidebar
    await page.getByText(MOCK_EMAIL).click();

    // Wait for virtual folders to appear
    await expect(page.getByText('My Drive')).toBeVisible({ timeout: 5000 });
    await expect(page.getByText('Shared drives')).toBeVisible({ timeout: 5000 });
    await expect(page.getByText('Shared with me')).toBeVisible({ timeout: 5000 });
  });

  test('should navigate to Shared drives', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');

    // Navigate directly to Shared drives
    await pathInput.click();
    await pathInput.fill(`${MOCK_GDRIVE_ROOT}/Shared drives`);
    await pathInput.press('Enter');

    // Should see the test shared drive
    await expect(
      page.locator(`[data-testid="file-item"][data-name="${MOCK_SHARED_DRIVE_NAME}"]`).first()
    ).toBeVisible({ timeout: 5000 });
  });

  test('should resolve Google Drive URL and navigate', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');

    // Paste a Google Drive URL
    await pathInput.click();
    await pathInput.fill('https://drive.google.com/drive/folders/folder-456');
    await pathInput.press('Enter');

    // Should navigate to the resolved path
    await expect(pathInput).toHaveValue(new RegExp(MOCK_FOLDER_NAME), { timeout: 5000 });
  });

  test('should apply smart view defaults for image folder', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');

    // Navigate to the image folder
    await pathInput.click();
    await pathInput.fill(
      `${MOCK_GDRIVE_ROOT}/Shared drives/${MOCK_SHARED_DRIVE_NAME}/${MOCK_FOLDER_NAME}`
    );
    await pathInput.press('Enter');

    // Wait for files to load - use more specific selector to avoid strict mode violation
    await expect(
      page.locator('[data-testid="file-grid"]').getByText('photo1.jpg').first()
    ).toBeVisible({ timeout: 5000 });

    // Check that grid view is applied (smart defaults for image-heavy folders)
    // The file-grid class indicates grid view mode
    await expect(page.locator('[data-testid="file-grid"]')).toBeVisible({ timeout: 5000 });
  });
});
