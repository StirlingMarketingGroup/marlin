import { test, expect } from '@playwright/test';
import { getTauriMockScript, MOCK_DOWNLOADS_DIR } from './tauri-mocks';

// Inject Tauri mocks before each test
test.beforeEach(async ({ page }) => {
  // Listen for console messages to debug
  page.on('console', (msg) => {
    console.log(`[Browser ${msg.type()}]`, msg.text());
  });

  // Inject mocks before page loads
  await page.addInitScript(getTauriMockScript());
});

test.describe('Image Dimensions Display', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');

    // Wait for app to initialize
    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });

    // Navigate to Downloads which has images with dimensions
    await pathInput.click();
    await pathInput.fill(MOCK_DOWNLOADS_DIR);
    await pathInput.press('Enter');

    // Wait for files to load
    await expect(
      page.locator('[data-testid="file-item"][data-name="image.png"]').first()
    ).toBeVisible({ timeout: 5000 });
  });

  test('should display image dimensions on initial load', async ({ page }) => {
    // Ensure we're in grid view so dimensions are rendered with thumbnails
    await page.keyboard.press('ControlOrMeta+1');
    await expect(page.locator('[data-testid="file-grid"]')).toBeVisible({ timeout: 5000 });

    // Look for the dimension text (1920×1080 for image.png)
    // Using the × character as displayed in FileNameDisplay.tsx
    const dimensionText = page.getByText('1920×1080');
    await expect(dimensionText).toBeVisible({ timeout: 10000 });

    // Also check for photo.jpg dimensions
    const photoDimensionText = page.getByText('3024×4032');
    await expect(photoDimensionText).toBeVisible({ timeout: 10000 });
  });

  test('should preserve image dimensions after page reload', async ({ page }, testInfo) => {
    // Ensure we're in grid view so dimensions are rendered with thumbnails
    await page.keyboard.press('ControlOrMeta+1');
    await expect(page.locator('[data-testid="file-grid"]')).toBeVisible({ timeout: 5000 });

    // First verify dimensions are visible
    const dimensionText = page.getByText('1920×1080');
    await expect(dimensionText).toBeVisible({ timeout: 10000 });
    console.log('[Test] Dimensions visible before reload');

    // Take screenshot before reload (stored with test artifacts)
    await page.screenshot({ path: testInfo.outputPath('before-reload.png') });

    // Reload the page (simulating F5/Cmd+R)
    console.log('[Test] Reloading page...');
    await page.reload();
    console.log('[Test] Page reloaded, waiting for app...');

    // Wait for app to reinitialize
    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });
    console.log('[Test] Path input visible after reload');

    // The path should still be Downloads (if persisted) or we need to re-navigate
    // Check if we're still in Downloads
    const currentPath = await pathInput.inputValue();
    console.log('[Test] Current path after reload:', currentPath);

    if (!currentPath.includes('Downloads')) {
      console.log('[Test] Re-navigating to Downloads...');
      await pathInput.click();
      await pathInput.fill(MOCK_DOWNLOADS_DIR);
      await pathInput.press('Enter');
    }

    // Wait for files to load
    await expect(
      page.locator('[data-testid="file-item"][data-name="image.png"]').first()
    ).toBeVisible({ timeout: 5000 });
    console.log('[Test] Files visible after reload');

    // Ensure grid view after reload (preferences may not persist in mock mode)
    await page.keyboard.press('ControlOrMeta+1');
    await expect(page.locator('[data-testid="file-grid"]')).toBeVisible({ timeout: 5000 });

    // Take screenshot after reload (stored with test artifacts)
    await page.screenshot({ path: testInfo.outputPath('after-reload.png') });

    // Check if dimensions are still visible after reload
    // This is the key assertion - if this fails, we've reproduced the bug
    const dimensionTextAfterReload = page.getByText('1920×1080');
    await expect(dimensionTextAfterReload).toBeVisible({ timeout: 10000 });
    console.log('[Test] Dimensions still visible after reload - TEST PASSED');
  });

  test('should show dimensions in grid view', async ({ page }, testInfo) => {
    // Make sure we're in grid view (thumbnail view)
    // The dimensions should be displayed below the file name
    await page.keyboard.press('ControlOrMeta+1');
    await expect(page.locator('[data-testid="file-grid"]')).toBeVisible({ timeout: 5000 });

    // Wait for thumbnail to load (uses the mock request_thumbnail)
    await page.waitForTimeout(500);

    // Check for dimension display
    const dimensionText = page.getByText('1920×1080');

    // Take a screenshot for debugging (stored with test artifacts)
    await page.screenshot({ path: testInfo.outputPath('grid-view-dimensions.png') });

    const isVisible = await dimensionText.isVisible();
    console.log('[Test] Dimensions visible in grid view:', isVisible);

    if (!isVisible) {
      // Debug: log the page content
      const content = await page.content();
      console.log('[Test] Page HTML contains "1920":', content.includes('1920'));
      console.log('[Test] Page HTML contains "×":', content.includes('×'));

      // Check if any file items are present
      const fileItems = page.locator('[data-testid="file-item"]');
      const count = await fileItems.count();
      console.log('[Test] Number of file items:', count);
    }

    await expect(dimensionText).toBeVisible({ timeout: 10000 });
  });
});
