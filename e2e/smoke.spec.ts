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

    // Verify files are displayed (mock returns sample.pdf and image.png)
    // This also confirms navigation completed successfully
    await expect(page.getByText('sample.pdf')).toBeVisible({ timeout: 5000 });

    // Verify the path input shows Downloads path
    await expect(pathInput).toHaveValue(MOCK_DOWNLOADS_DIR);
  });

  test('should display files in home directory', async ({ page }) => {
    // Should start in home directory with mock files
    await expect(page.getByText('Documents')).toBeVisible({ timeout: 5000 });
    await expect(page.getByText('Downloads')).toBeVisible({ timeout: 5000 });
  });
});
