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
    await expect(page.getByText('image.png')).toBeVisible({ timeout: 5000 });
  });

  test('should display image dimensions on initial load', async ({ page }) => {
    // Look for the dimension text (1920×1080 for image.png)
    // Using the × character as displayed in FileNameDisplay.tsx
    const dimensionText = page.getByText('1920×1080');
    await expect(dimensionText).toBeVisible({ timeout: 10000 });

    // Also check for photo.jpg dimensions
    const photoDimensionText = page.getByText('3024×4032');
    await expect(photoDimensionText).toBeVisible({ timeout: 10000 });
  });

  test('should preserve image dimensions after page reload', async ({ page }) => {
    // First verify dimensions are visible
    const dimensionText = page.getByText('1920×1080');
    await expect(dimensionText).toBeVisible({ timeout: 10000 });
    console.log('[Test] Dimensions visible before reload');

    // Take screenshot before reload
    await page.screenshot({ path: 'test-results/before-reload.png' });

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
    await expect(page.getByText('image.png')).toBeVisible({ timeout: 5000 });
    console.log('[Test] Files visible after reload');

    // Take screenshot after reload
    await page.screenshot({ path: 'test-results/after-reload.png' });

    // Check if dimensions are still visible after reload
    // This is the key assertion - if this fails, we've reproduced the bug
    const dimensionTextAfterReload = page.getByText('1920×1080');
    await expect(dimensionTextAfterReload).toBeVisible({ timeout: 10000 });
    console.log('[Test] Dimensions still visible after reload - TEST PASSED');
  });

  test('should show dimensions in grid view', async ({ page }) => {
    // Make sure we're in grid view (thumbnail view)
    // The dimensions should be displayed below the file name

    // Wait for thumbnail to load (uses the mock request_thumbnail)
    await page.waitForTimeout(500);

    // Check for dimension display
    const dimensionText = page.getByText('1920×1080');

    // Take a screenshot for debugging
    await page.screenshot({ path: 'test-results/grid-view-dimensions.png' });

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

  test('debug: trace what happens during reload', async ({ page }) => {
    // This test is specifically for debugging the reload issue
    console.log('[Test] === Debug test starting ===');

    // Verify initial state
    const pathInput = page.locator('[data-testid="path-input"]');
    const initialPath = await pathInput.inputValue();
    console.log('[Test] Initial path:', initialPath);

    // Check for files
    const fileItems = page.locator('[data-testid="file-item"]');
    const initialCount = await fileItems.count();
    console.log('[Test] Initial file count:', initialCount);

    // Wait a bit more for thumbnails to load and dimensions to update
    await page.waitForTimeout(1000);

    // Inspect the file item HTML to see what's being rendered
    const fileItemHtml = await page.locator('[data-testid="file-item"]').first().innerHTML();
    console.log('[Test] First file item HTML:', fileItemHtml.slice(0, 500));

    // Check if any element contains dimension-like text
    const allDivs = await page.locator('[data-testid="file-item"] div').allTextContents();
    console.log('[Test] All div text in file items:', allDivs.filter((t) => t.trim()).slice(0, 10));

    // Check for dimensions
    const allText = await page.locator('body').textContent();
    console.log('[Test] Page contains "1920":', allText?.includes('1920'));
    console.log('[Test] Page contains "×":', allText?.includes('×'));

    // Log all dimension-like text on page
    const dimensionPattern = /\d{3,4}×\d{3,4}/g;
    const dimensions = allText?.match(dimensionPattern) || [];
    console.log('[Test] Found dimension patterns:', dimensions);

    // Now reload
    console.log('[Test] === Reloading ===');
    await page.reload();

    // Wait for app
    await expect(pathInput).toBeVisible({ timeout: 15000 });
    console.log('[Test] App reloaded');

    // Check path after reload
    const afterPath = await pathInput.inputValue();
    console.log('[Test] Path after reload:', afterPath);

    // If path is different, that's the issue - app doesn't persist location
    if (afterPath !== initialPath) {
      console.log('[Test] PATH CHANGED AFTER RELOAD - this might be why dimensions disappear');

      // Navigate back
      await pathInput.click();
      await pathInput.fill(MOCK_DOWNLOADS_DIR);
      await pathInput.press('Enter');
      await expect(page.getByText('image.png')).toBeVisible({ timeout: 5000 });
    }

    // Check files after reload
    const afterCount = await fileItems.count();
    console.log('[Test] File count after reload:', afterCount);

    // Check for dimensions after reload
    const afterText = await page.locator('body').textContent();
    console.log('[Test] After reload - Page contains "1920":', afterText?.includes('1920'));
    console.log('[Test] After reload - Page contains "×":', afterText?.includes('×'));

    // Log all dimension-like text on page after reload
    const dimensionsAfter = afterText?.match(dimensionPattern) || [];
    console.log('[Test] Found dimension patterns after reload:', dimensionsAfter);

    // This is just a debug test, always pass
    expect(true).toBe(true);
  });
});
