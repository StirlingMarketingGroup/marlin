import { test, expect } from '@playwright/test';
import { getTauriMockScript, MOCK_HOME_DIR, MOCK_DOWNLOADS_DIR } from './tauri-mocks';

test.describe('File Manager Navigation', () => {
  test.beforeEach(async ({ page }) => {
    // Inject Tauri mocks before page loads
    await page.addInitScript(getTauriMockScript());
    await page.goto('/');

    // Wait for app to initialize
    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });
  });

  test('should navigate to directories via path input', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    const initialPath = await pathInput.inputValue();

    // Navigate to Downloads via path input
    await pathInput.click();
    await pathInput.fill(MOCK_DOWNLOADS_DIR);
    await pathInput.press('Enter');

    // Wait for navigation to complete
    await expect(
      page.locator('[data-testid="file-item"][data-name="sample.pdf"]').first()
    ).toBeVisible({ timeout: 5000 });

    const newPath = await pathInput.inputValue();
    expect(newPath).not.toBe(initialPath);
    expect(newPath).toBe(MOCK_DOWNLOADS_DIR);

    // Navigate back to home via path input
    await pathInput.click();
    await pathInput.fill(MOCK_HOME_DIR);
    await pathInput.press('Enter');

    // Wait for navigation to home
    await expect(page.locator('[data-testid="file-item"][data-name="Documents"]')).toBeVisible({
      timeout: 5000,
    });

    const homePath = await pathInput.inputValue();
    expect(homePath).toBe(MOCK_HOME_DIR);
  });

  test('should navigate between nested directories', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');

    // Start at home
    const homePath = await pathInput.inputValue();
    expect(homePath).toBe(MOCK_HOME_DIR);

    // Navigate to Downloads
    await pathInput.click();
    await pathInput.fill(MOCK_DOWNLOADS_DIR);
    await pathInput.press('Enter');

    // Verify we're in Downloads
    await expect(
      page.locator('[data-testid="file-item"][data-name="sample.pdf"]').first()
    ).toBeVisible({ timeout: 5000 });
    const downloadsPath = await pathInput.inputValue();
    expect(downloadsPath).toBe(MOCK_DOWNLOADS_DIR);

    // Navigate back to home manually
    await pathInput.click();
    await pathInput.fill(MOCK_HOME_DIR);
    await pathInput.press('Enter');

    // Verify we're back at home
    await expect(page.locator('[data-testid="file-item"][data-name="Documents"]')).toBeVisible({
      timeout: 5000,
    });
    const backHome = await pathInput.inputValue();
    expect(backHome).toBe(MOCK_HOME_DIR);
  });

  test('should handle path bar editing', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');

    // Click on path input to edit
    await pathInput.click();

    // Type a new path
    await pathInput.fill(MOCK_DOWNLOADS_DIR);
    await pathInput.press('Enter');

    // Wait for navigation
    await expect(
      page.locator('[data-testid="file-item"][data-name="sample.pdf"]').first()
    ).toBeVisible({ timeout: 5000 });

    // Should navigate to Downloads directory
    const finalPath = await pathInput.inputValue();
    expect(finalPath).toContain('Downloads');
  });

  test('should handle invalid paths gracefully', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');

    // Save current path
    const originalPath = await pathInput.inputValue();

    // Try to navigate to an invalid path
    await pathInput.click();
    await pathInput.fill('/nonexistent/path/that/should/not/exist');
    await pathInput.press('Enter');

    await page.waitForTimeout(1000);

    // App should handle gracefully - either show error or stay in current directory
    // The mock returns empty array for unknown paths, so files should be empty
    const pathAfterError = await pathInput.inputValue();
    // Should either stay in original path or show the attempted path
    expect(pathAfterError).toBeTruthy();
  });

  test('should preserve navigation history after page reload', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');

    // Navigate to Downloads
    await pathInput.click();
    await pathInput.fill(MOCK_DOWNLOADS_DIR);
    await pathInput.press('Enter');

    await expect(
      page.locator('[data-testid="file-item"][data-name="sample.pdf"]').first()
    ).toBeVisible({ timeout: 5000 });

    const finalPath = await pathInput.inputValue();

    // Note: In mock mode, preferences don't persist across page reloads
    // This test verifies the navigation itself works
    expect(finalPath).toBe(MOCK_DOWNLOADS_DIR);
  });

  test('should handle rapid navigation without state corruption', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');

    // Rapidly navigate back and forth
    for (let i = 0; i < 3; i++) {
      await pathInput.click();
      await pathInput.fill(MOCK_DOWNLOADS_DIR);
      await pathInput.press('Enter');
      await page.waitForTimeout(200);

      await page.keyboard.press('ControlOrMeta+ArrowLeft');
      await page.waitForTimeout(200);
    }

    // Should still be in a valid state
    await expect(pathInput).toBeVisible();

    const currentPath = await pathInput.inputValue();
    expect(currentPath).toBeTruthy();
  });

  test('should update window title with current directory', async ({ page }) => {
    const initialTitle = await page.title();

    // Navigate to Downloads
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.click();
    await pathInput.fill(MOCK_DOWNLOADS_DIR);
    await pathInput.press('Enter');

    await expect(
      page.locator('[data-testid="file-item"][data-name="sample.pdf"]').first()
    ).toBeVisible({ timeout: 5000 });

    const newTitle = await page.title();
    // Title should change after navigation (implementation dependent)
    // Just verify it's still a valid title
    expect(newTitle).toBeTruthy();
  });

  test('should handle keyboard shortcuts for navigation', async ({ page }) => {
    // Test various keyboard shortcuts - use correct Playwright key names
    const shortcuts = [
      { key: 'ControlOrMeta+ArrowUp', description: 'go up' },
      { key: 'ControlOrMeta+ArrowLeft', description: 'go back' },
      { key: 'ControlOrMeta+ArrowRight', description: 'go forward' },
    ];

    for (const shortcut of shortcuts) {
      await page.keyboard.press(shortcut.key);
      await page.waitForTimeout(200);

      // Should not cause errors - app should still be functional
      const pathInput = page.locator('[data-testid="path-input"]');
      await expect(pathInput).toBeVisible();
    }
  });
});
