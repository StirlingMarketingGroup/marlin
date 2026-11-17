import { test, expect } from '@playwright/test';
import { getTauriMockScript, MOCK_ARTWORK_DIR, MOCK_PERCENT_DIR } from './tauri-mocks';

declare global {
  interface Window {
    __MOCK_DIR_PREFS__?: Record<string, { viewMode?: string }>;
  }
}

test.describe('Smart view defaults', () => {
  test('a local folder of pictures opens in grid view', async ({ page }) => {
    await page.addInitScript(getTauriMockScript());
    await page.goto('/');

    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });
    await pathInput.click();
    await pathInput.fill(MOCK_ARTWORK_DIR);
    await pathInput.press('Enter');

    await expect(page.locator('[data-testid="file-grid"]')).toBeVisible({ timeout: 5000 });
  });

  test('a local folder of text files stays in list view', async ({ page }) => {
    await page.addInitScript(getTauriMockScript());
    await page.goto('/');

    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });
    await pathInput.click();
    await pathInput.fill(MOCK_PERCENT_DIR);
    await pathInput.press('Enter');

    await expect(page.locator('[data-testid="file-item"][data-name="note.txt"]')).toBeVisible({
      timeout: 5000,
    });
    // The default is applied after the files appear, so give a wrong one time to show.
    await page.waitForTimeout(300);
    await expect(page.locator('[data-testid="file-grid"]')).toHaveCount(0);
  });

  test('a folder with a saved view keeps it', async ({ page }) => {
    await page.addInitScript(getTauriMockScript());
    await page.addInitScript((path) => {
      window.__MOCK_DIR_PREFS__ = { [path]: { viewMode: 'list' } };
    }, MOCK_ARTWORK_DIR);
    await page.goto('/');

    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });
    await pathInput.click();
    await pathInput.fill(MOCK_ARTWORK_DIR);
    await pathInput.press('Enter');

    await expect(page.locator('[data-testid="file-item"][data-name="notes.txt"]')).toBeVisible({
      timeout: 5000,
    });
    // The default is applied after the files appear, so give a wrong one time to show.
    await page.waitForTimeout(300);
    await expect(page.locator('[data-testid="file-grid"]')).toHaveCount(0);
  });

  test('the startup folder gets its default', async ({ page }) => {
    await page.addInitScript(getTauriMockScript());
    await page.goto('/?path=' + encodeURIComponent(MOCK_ARTWORK_DIR));

    await expect(page.locator('[data-testid="file-grid"]')).toBeVisible({ timeout: 15000 });
  });
});
