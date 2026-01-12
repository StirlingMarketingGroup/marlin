import { test, expect } from '@playwright/test';
import { getTauriMockScript } from './tauri-mocks';

// Inject Tauri mocks before each test
test.beforeEach(async ({ page }) => {
  await page.addInitScript(getTauriMockScript());
});

test.describe('Type-to-Filter', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');

    // Wait for app to initialize and files to load
    await expect(page.locator('[data-testid="file-item"]').first()).toBeVisible({
      timeout: 15000,
    });
  });

  test('should show filter input when typing', async ({ page }) => {
    // Type a letter to trigger filter
    await page.keyboard.type('d');

    // Filter input should appear
    const filterInput = page.locator('[data-testid="filter-input"]');
    await expect(filterInput).toBeVisible({ timeout: 2000 });
    await expect(filterInput).toHaveValue('d');
  });

  test('should filter files by name', async ({ page }) => {
    // Verify Documents and Downloads are both visible initially
    await expect(page.locator('[data-testid="file-item"][data-name="Documents"]')).toBeVisible();
    await expect(page.locator('[data-testid="file-item"][data-name="Downloads"]')).toBeVisible();

    // Type to filter - "ments" should only match "Documents"
    await page.keyboard.type('ments');

    // Documents should still be visible, Downloads should be hidden
    await expect(page.locator('[data-testid="file-item"][data-name="Documents"]')).toBeVisible();
    await expect(
      page.locator('[data-testid="file-item"][data-name="Downloads"]')
    ).not.toBeVisible();
  });

  test('should clear filter on Escape', async ({ page }) => {
    // Type to create a filter
    await page.keyboard.type('doc');
    await expect(page.locator('[data-testid="filter-input"]')).toBeVisible();

    // Press Escape to clear
    await page.keyboard.press('Escape');

    // Filter input should disappear
    await expect(page.locator('[data-testid="filter-input"]')).not.toBeVisible();

    // Both files should be visible again
    await expect(page.locator('[data-testid="file-item"][data-name="Documents"]')).toBeVisible();
    await expect(page.locator('[data-testid="file-item"][data-name="Downloads"]')).toBeVisible();
  });

  test('should show match count in filter input', async ({ page }) => {
    // Type to filter - "Do" matches both Documents and Downloads
    await page.keyboard.type('Do');

    // Should show "2 matches"
    await expect(page.getByText('2 matches')).toBeVisible();

    // Add more characters to narrow down
    await page.keyboard.type('cu');

    // Should show "1 match" (only Documents)
    await expect(page.getByText('1 match')).toBeVisible();
  });

  test('should show empty state when no matches', async ({ page }) => {
    // Type something that won't match any files
    await page.keyboard.type('zzzzz');

    // Should show "0 matches"
    await expect(page.getByText('0 matches')).toBeVisible();

    // Should show empty state message
    await expect(page.getByText(/No files match/)).toBeVisible();
  });
});
