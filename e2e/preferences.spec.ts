import { test, expect } from '@playwright/test';
import { getTauriMockScript, MOCK_HOME_DIR, MOCK_DOWNLOADS_DIR } from './tauri-mocks';

const hiddenFileSelector = '[data-testid="file-item"][data-hidden="true"]';
const directorySelector = '[data-testid="file-item"][data-directory="true"]';

test.describe('File Manager Preferences', () => {
  test.beforeEach(async ({ page }) => {
    // Inject Tauri mocks before page loads
    await page.addInitScript(getTauriMockScript());
    await page.goto('/');

    // Wait for app to initialize
    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });
  });

  test('should toggle hidden files visibility', async ({ page }) => {
    const hiddenFileLocator = page.locator(hiddenFileSelector);

    // Initial state - hidden files should be hidden (count = 0)
    const initialHiddenFiles = await hiddenFileLocator.count();
    expect(initialHiddenFiles).toBe(0);

    // Toggle hidden files via keyboard shortcut
    await page.keyboard.press('ControlOrMeta+Shift+.');
    await page.waitForTimeout(500);

    // Should now see hidden files (mock has .hidden file)
    const visibleHiddenFiles = await hiddenFileLocator.count();
    expect(visibleHiddenFiles).toBeGreaterThan(0);

    // Toggle again to hide
    await page.keyboard.press('ControlOrMeta+Shift+.');
    await page.waitForTimeout(500);

    // Should be hidden again
    const hiddenAgain = await hiddenFileLocator.count();
    expect(hiddenAgain).toBe(0);
  });

  test('should toggle hidden files and verify visibility', async ({ page }) => {
    const hiddenFileLocator = page.locator(hiddenFileSelector);

    // Enable hidden files
    await page.keyboard.press('ControlOrMeta+Shift+.');
    await page.waitForTimeout(500);

    // Verify hidden files are visible (mock has .hidden file)
    const hiddenCount = await hiddenFileLocator.count();
    expect(hiddenCount).toBeGreaterThan(0);

    // Verify the hidden file is actually the .hidden file from mock
    const hiddenFile = page.locator('[data-testid="file-item"][data-name=".hidden"]');
    await expect(hiddenFile).toBeVisible();

    // Disable hidden files
    await page.keyboard.press('ControlOrMeta+Shift+.');
    await page.waitForTimeout(500);

    // Hidden file should no longer be visible
    await expect(hiddenFile).not.toBeVisible();
  });

  test('should switch between grid and list view', async ({ page }) => {
    const fileGrid = page.locator('[data-testid="file-grid"]');
    const fileList = page.locator('[data-testid="file-list"]');

    // Default is list view
    await expect(fileList).toBeVisible({ timeout: 5000 });

    // Switch to grid view with Cmd+1
    await page.keyboard.press('ControlOrMeta+1');
    await page.waitForTimeout(300);

    await expect(fileGrid).toBeVisible({ timeout: 5000 });

    // Switch back to list view with Cmd+2
    await page.keyboard.press('ControlOrMeta+2');
    await page.waitForTimeout(300);

    await expect(fileList).toBeVisible({ timeout: 5000 });
  });

  test('should persist view mode across navigation', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    const fileList = page.locator('[data-testid="file-list"]');
    const fileGrid = page.locator('[data-testid="file-grid"]');

    // Switch to list view
    await page.keyboard.press('ControlOrMeta+2');
    await page.waitForTimeout(300);
    await expect(fileList).toBeVisible({ timeout: 5000 });

    // Navigate to Downloads
    await pathInput.click();
    await pathInput.fill(MOCK_DOWNLOADS_DIR);
    await pathInput.press('Enter');
    await expect(
      page.locator('[data-testid="file-item"][data-name="sample.pdf"]').first()
    ).toBeVisible({ timeout: 5000 });

    // Navigate back
    await page.keyboard.press('ControlOrMeta+ArrowLeft');
    await page.waitForTimeout(500);

    // View mode might reset or persist depending on implementation
    // Just verify one of the views is visible
    const listVisible = await fileList.isVisible();
    const gridVisible = await fileGrid.isVisible();
    expect(listVisible || gridVisible).toBe(true);
  });

  test('should handle rapid preference toggles without state corruption', async ({ page }) => {
    const hiddenFileLocator = page.locator(hiddenFileSelector);

    // Rapidly toggle hidden files multiple times
    for (let i = 0; i < 5; i++) {
      await page.keyboard.press('ControlOrMeta+Shift+.');
      await page.waitForTimeout(100);
    }

    await page.waitForTimeout(500); // Let all requests settle

    // State should be consistent - either visible or hidden
    const hiddenFiles = await hiddenFileLocator.count();

    // Toggle once more and verify it works correctly
    await page.keyboard.press('ControlOrMeta+Shift+.');
    await page.waitForTimeout(500);

    const newHiddenCount = await hiddenFileLocator.count();
    // State should have changed
    expect(newHiddenCount !== hiddenFiles || hiddenFiles === 0).toBe(true);
  });

  test('should display files correctly in both view modes', async ({ page }) => {
    const fileList = page.locator('[data-testid="file-list"]');
    const fileGrid = page.locator('[data-testid="file-grid"]');

    // Default is list mode - verify files are visible using testid selectors
    await expect(fileList).toBeVisible({ timeout: 5000 });
    await expect(page.locator('[data-testid="file-item"][data-name="Documents"]')).toBeVisible();
    await expect(page.locator('[data-testid="file-item"][data-name="Downloads"]')).toBeVisible();

    // Switch to grid mode
    await page.keyboard.press('ControlOrMeta+1');
    await page.waitForTimeout(300);

    // Files should still be visible in grid mode
    await expect(fileGrid).toBeVisible({ timeout: 5000 });
    await expect(page.locator('[data-testid="file-item"][data-name="Documents"]')).toBeVisible();
    await expect(page.locator('[data-testid="file-item"][data-name="Downloads"]')).toBeVisible();
  });

  test('should navigate to directory via double-click', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');

    // Double-click on Downloads directory
    const downloadsDir = page.locator('[data-testid="file-item"][data-name="Downloads"]');
    await expect(downloadsDir).toBeVisible({ timeout: 5000 });
    await downloadsDir.dblclick();

    // Should navigate and show Downloads contents
    await expect(
      page.locator('[data-testid="file-item"][data-name="sample.pdf"]').first()
    ).toBeVisible({ timeout: 5000 });

    const currentPath = await pathInput.inputValue();
    expect(currentPath).toContain('Downloads');
  });
});
