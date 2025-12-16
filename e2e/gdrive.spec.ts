import { test, expect } from '@playwright/test';

/**
 * Google Drive E2E Tests using Service Account
 *
 * Prerequisites:
 * - Set GOOGLE_SERVICE_ACCOUNT_KEY_FILE env var to path of service account JSON
 * - Share test folders with the service account email
 *
 * Run with:
 *   GOOGLE_SERVICE_ACCOUNT_KEY_FILE=/path/to/key.json npm run test:e2e
 */

// Service account email - this will be auto-detected by the app when configured
const SERVICE_ACCOUNT_EMAIL = 'marlin-e2e-testing@marlin-480721.iam.gserviceaccount.com';

test.describe('Google Drive Integration', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await page.waitForLoadState('networkidle');
    // Wait for the app to fully initialize by checking for the path bar
    await expect(page.locator('[data-testid="path-bar"]')).toBeVisible({ timeout: 30000 });
  });

  /**
   * Helper to get the path input by clicking on the path bar first
   */
  async function getPathInput(page: import('@playwright/test').Page) {
    const pathBar = page.locator('[data-testid="path-bar"]');
    await pathBar.click();
    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 5000 });
    return pathInput;
  }

  /**
   * Helper to wait for navigation to complete
   */
  async function waitForNavigation(page: import('@playwright/test').Page) {
    await page.waitForLoadState('networkidle');
    // Give the app time to process navigation
    await page.waitForTimeout(500);
  }

  /**
   * Helper to wait for file list to load
   */
  async function waitForFileList(page: import('@playwright/test').Page) {
    await page.waitForLoadState('networkidle');
    await page.waitForTimeout(500);
  }

  /**
   * Helper to get current path from path bar
   */
  async function getCurrentPath(page: import('@playwright/test').Page) {
    const pathBar = page.locator('[data-testid="path-bar"]');
    return await pathBar.textContent();
  }

  test('should preserve gdrive:// scheme with double slashes', async ({ page }) => {
    const pathInput = await getPathInput(page);

    // Navigate to service account's drive root
    await pathInput.clear();
    await pathInput.fill(`gdrive://${SERVICE_ACCOUNT_EMAIL}/`);
    await pathInput.press('Enter');

    await waitForNavigation(page);

    // Check the path bar shows gdrive:// path
    const displayedPath = await getCurrentPath(page);
    console.log('Displayed path:', displayedPath);
    expect(displayedPath).toContain('gdrive://');
    expect(displayedPath).not.toMatch(/gdrive:\/[^/]/);
  });

  test('should load shared folders (Shared with me)', async ({ page }) => {
    const pathInput = await getPathInput(page);

    // Navigate to Shared with me
    await pathInput.clear();
    await pathInput.fill(`gdrive://${SERVICE_ACCOUNT_EMAIL}/Shared with me`);
    await pathInput.press('Enter');

    await waitForNavigation(page);
    await waitForFileList(page);

    // Verify we're in the shared folder
    const displayedPath = await getCurrentPath(page);
    console.log('Shared folder path:', displayedPath);
    expect(displayedPath).toContain('Shared with me');

    // Check if there are any shared items visible
    const fileItems = page.locator('[data-testid="file-item"], [data-directory]');
    const count = await fileItems.count();
    console.log(`Found ${count} items in Shared with me`);

    if (count > 0) {
      const firstItem = fileItems.first();
      const itemName = await firstItem.textContent();
      console.log('First shared item:', itemName);
    }
  });

  test('should navigate into subfolders of shared folders', async ({ page }) => {
    const pathInput = await getPathInput(page);

    // Navigate to Shared with me
    await pathInput.clear();
    await pathInput.fill(`gdrive://${SERVICE_ACCOUNT_EMAIL}/Shared with me`);
    await pathInput.press('Enter');

    await waitForNavigation(page);
    await waitForFileList(page);

    // Find a folder to navigate into
    const folder = page.locator('[data-testid="file-item"][data-directory="true"]').first();

    if (await folder.isVisible({ timeout: 5000 }).catch(() => false)) {
      const folderName = await folder.textContent();
      console.log('Navigating into shared folder:', folderName);

      await folder.dblclick();
      await waitForNavigation(page);
      await waitForFileList(page);

      // Verify path changed
      const newPath = await getCurrentPath(page);
      console.log('Path after navigating into shared folder:', newPath);

      // Should still have proper gdrive:// scheme
      expect(newPath).toContain('gdrive://');
      expect(newPath).not.toMatch(/gdrive:\/[^/]/);

      // Check for subfolder contents
      const subItems = page.locator('[data-testid="file-item"], [data-directory]');
      const subCount = await subItems.count();
      console.log(`Found ${subCount} items in subfolder`);
    } else {
      console.log('No shared folders available, skipping subfolder test');
      test.skip();
    }
  });

  test('should load My Drive for service account', async ({ page }) => {
    const pathInput = await getPathInput(page);

    // Navigate to My Drive
    await pathInput.clear();
    await pathInput.fill(`gdrive://${SERVICE_ACCOUNT_EMAIL}/My Drive`);
    await pathInput.press('Enter');

    await waitForNavigation(page);
    await waitForFileList(page);

    // Verify we're in My Drive
    const displayedPath = await getCurrentPath(page);
    console.log('My Drive path:', displayedPath);
    expect(displayedPath).toContain('My Drive');

    // Service account's own drive may be empty, that's ok
    const fileItems = page.locator('[data-testid="file-item"], [data-directory]');
    const count = await fileItems.count();
    console.log(`Found ${count} items in My Drive`);
  });

  test('should load Google Drive URL directly', async ({ page }) => {
    const pathInput = await getPathInput(page);

    // Try navigating with a Google Drive URL format
    await pathInput.clear();
    await pathInput.fill('https://drive.google.com/open?id=test123');
    await pathInput.press('Enter');

    await waitForNavigation(page);

    // The URL should be preserved or converted to gdrive://
    const displayedPath = await getCurrentPath(page);
    console.log('Displayed path after https URL:', displayedPath);

    // Should have proper double slashes in scheme
    if (displayedPath?.startsWith('https://')) {
      expect(displayedPath).toContain('https://');
    } else if (displayedPath?.startsWith('gdrive://')) {
      expect(displayedPath).toContain('gdrive://');
    }

    // Should NOT have single slash after scheme
    expect(displayedPath).not.toMatch(/^[a-z]+:\/[^/]/);
  });

  test('should show service account in sidebar when configured', async ({ page }) => {
    // Look for any Google account button in sidebar (contains @)
    const googleAccountButton = page.locator('button:has-text("@")').first();

    if (await googleAccountButton.isVisible({ timeout: 10000 }).catch(() => false)) {
      const buttonText = await googleAccountButton.textContent();
      console.log('Found Google account in sidebar:', buttonText);

      await googleAccountButton.click();
      await waitForNavigation(page);

      // Should navigate to the account root
      const displayedPath = await getCurrentPath(page);
      console.log('Path after clicking account:', displayedPath);

      expect(displayedPath).toContain('gdrive://');
    } else {
      console.log(
        'No Google account visible in sidebar - ensure GOOGLE_SERVICE_ACCOUNT_KEY_FILE is set'
      );
      test.skip();
    }
  });

  test('should preserve gdrive:// on page reload', async ({ page }) => {
    const pathInput = await getPathInput(page);

    // Navigate to service account drive
    await pathInput.clear();
    await pathInput.fill(`gdrive://${SERVICE_ACCOUNT_EMAIL}/`);
    await pathInput.press('Enter');

    await waitForNavigation(page);

    const pathBeforeReload = await getCurrentPath(page);
    console.log('Path before reload:', pathBeforeReload);

    // Reload the page
    await page.reload();
    await page.waitForLoadState('networkidle');

    // Wait for app to reinitialize
    await expect(page.locator('[data-testid="path-bar"]')).toBeVisible({ timeout: 30000 });

    const pathAfterReload = await getCurrentPath(page);
    console.log('Path after reload:', pathAfterReload);

    // Should still have double slashes
    expect(pathAfterReload).toContain('gdrive://');
    expect(pathAfterReload).not.toMatch(/gdrive:\/[^/]/);
  });
});
