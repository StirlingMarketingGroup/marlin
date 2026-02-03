import { test, expect } from '@playwright/test';
import { getTauriMockScript, MOCK_HOME_DIR } from './tauri-mocks';

// Inject Tauri mocks before each test
test.beforeEach(async ({ page }) => {
  await page.addInitScript(getTauriMockScript());
});

test.describe('Toast Visual Tests', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');

    // Wait for app to initialize
    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });
  });

  test('should display error toast with readable text', async ({ page }) => {
    // Trigger an error toast via the store (exposed on window)
    await page.evaluate(() => {
      const store = (window as any).__TOAST_STORE__;
      if (store) {
        store.getState().addToast({
          type: 'error',
          message: 'Drop failed: Failed to copy file: Read-only file system (os error 30)',
        });
      }
    });

    // Wait for toast to appear
    await page.waitForTimeout(500);

    // Take a screenshot to see the toast
    await page.screenshot({ path: 'e2e/screenshots/toast-error.png', fullPage: true });

    // Check that toast is visible
    const toast = page.locator('text=Drop failed');
    await expect(toast).toBeVisible({ timeout: 5000 });
  });

  test('should display all toast types', async ({ page }) => {
    // Trigger all toast types via the store
    await page.evaluate(() => {
      const store = (window as any).__TOAST_STORE__;
      if (store) {
        const { addToast } = store.getState();
        addToast({ type: 'success', message: 'Successfully moved 3 items' });
        addToast({
          type: 'error',
          message: 'Drop failed: Failed to copy file: Read-only file system (os error 30)',
        });
        addToast({ type: 'info', message: 'This is an informational message' });
      }
    });

    await page.waitForTimeout(500);
    await page.screenshot({ path: 'e2e/screenshots/toast-all-types.png', fullPage: true });
  });
});
