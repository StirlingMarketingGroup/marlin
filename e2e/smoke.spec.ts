import { test, expect } from '@playwright/test';
import { getTauriMockScript, MOCK_DOWNLOADS_DIR } from './tauri-mocks';

// Inject Tauri mocks before each test
test.beforeEach(async ({ page }) => {
  // Inject mocks before page loads
  await page.addInitScript(getTauriMockScript());
});

test.describe('Smoke Tests', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');

    // Wait for app to initialize
    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });
  });

  test('should navigate to Downloads folder', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');

    // Navigate to Downloads
    await pathInput.click();
    await pathInput.fill(MOCK_DOWNLOADS_DIR);
    await pathInput.press('Enter');

    // Verify files are displayed (mock returns sample.pdf, image.png, photo.jpg)
    // This also confirms navigation completed successfully
    // Use file-item data-name attribute since text may be combined with size in grid view
    await expect(page.locator('[data-testid="file-item"][data-name="sample.pdf"]')).toBeVisible({
      timeout: 5000,
    });

    // Verify the path input shows Downloads path
    await expect(pathInput).toHaveValue(MOCK_DOWNLOADS_DIR);
  });

  test('should display files in home directory', async ({ page }) => {
    // Should start in home directory with mock files
    // Use data-testid selectors to avoid matching sidebar items
    await expect(page.locator('[data-testid="file-item"][data-name="Documents"]')).toBeVisible({
      timeout: 5000,
    });
    await expect(page.locator('[data-testid="file-item"][data-name="Downloads"]')).toBeVisible({
      timeout: 5000,
    });
  });
});
