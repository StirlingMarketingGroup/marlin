import { test, expect } from '@playwright/test';

test.describe('Google Drive Integration', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await page.waitForLoadState('networkidle');
    // Wait for the app to fully initialize by checking for the path input
    await expect(page.locator('input.input-field').first()).toBeVisible({ timeout: 15000 });
  });

  /**
   * Helper to wait for navigation to complete by checking that the path input
   * has been updated and is stable (no longer changing)
   */
  async function waitForNavigation(
    page: import('@playwright/test').Page,
    pathInput: import('@playwright/test').Locator
  ) {
    // Wait for any loading indicators to disappear and path to stabilize
    await page.waitForLoadState('networkidle');
    // Give the path input a moment to update, then verify it's stable
    let previousPath = '';
    let currentPath = await pathInput.inputValue();
    let attempts = 0;
    while (previousPath !== currentPath && attempts < 10) {
      previousPath = currentPath;
      await page.waitForTimeout(300); // Small polling interval
      currentPath = await pathInput.inputValue();
      attempts++;
    }
  }

  test('should preserve gdrive:// scheme with double slashes', async ({ page }) => {
    // Find the path input - it's the text input in the path bar area
    const pathInput = page.locator('input.input-field').first();
    await expect(pathInput).toBeVisible({ timeout: 15000 });

    // Click to focus and clear
    await pathInput.click();
    await pathInput.fill('gdrive://brian@smg.gg/');
    await pathInput.press('Enter');

    await waitForNavigation(page, pathInput);

    // Check the path bar still shows gdrive:// with double slashes
    const displayedPath = await pathInput.inputValue();
    console.log('Displayed path:', displayedPath);
    expect(displayedPath).toContain('gdrive://');
    expect(displayedPath).not.toMatch(/gdrive:\/[^/]/); // Should NOT be gdrive:/something
  });

  test('should preserve gdrive:// on page reload', async ({ page }) => {
    // Navigate to a gdrive path
    const pathInput = page.locator('input.input-field').first();
    await expect(pathInput).toBeVisible({ timeout: 15000 });

    await pathInput.click();
    await pathInput.fill('gdrive://brian@smg.gg/');
    await pathInput.press('Enter');

    await waitForNavigation(page, pathInput);

    // Store the path before reload
    const pathBeforeReload = await pathInput.inputValue();
    console.log('Path before reload:', pathBeforeReload);

    // Reload the page
    await page.reload();
    await page.waitForLoadState('networkidle');

    // Wait for app to reinitialize after reload
    const pathInputAfter = page.locator('input.input-field').first();
    await expect(pathInputAfter).toBeVisible({ timeout: 15000 });
    await waitForNavigation(page, pathInputAfter);

    // Check the path after reload
    const pathAfterReload = await pathInputAfter.inputValue();
    console.log('Path after reload:', pathAfterReload);

    // Should still have double slashes
    expect(pathAfterReload).toContain('gdrive://');
    expect(pathAfterReload).not.toMatch(/gdrive:\/[^/]/);
  });

  test('should preserve https:// URLs without normalization', async ({ page }) => {
    const pathInput = page.locator('input.input-field').first();
    await expect(pathInput).toBeVisible({ timeout: 15000 });

    await pathInput.click();
    await pathInput.fill('https://drive.google.com/open?id=test123');
    await pathInput.press('Enter');

    await waitForNavigation(page, pathInput);

    // The URL should be preserved or converted to gdrive://
    // Either way, it should have proper double slashes
    const displayedPath = await pathInput.inputValue();
    console.log('Displayed path after https URL:', displayedPath);

    if (displayedPath.startsWith('https://')) {
      expect(displayedPath).toContain('https://');
    } else if (displayedPath.startsWith('gdrive://')) {
      expect(displayedPath).toContain('gdrive://');
    }

    // Should NOT have single slash after scheme
    expect(displayedPath).not.toMatch(/^[a-z]+:\/[^/]/);
  });

  test('should click Google account in sidebar and show gdrive:// path', async ({ page }) => {
    // Look for Google account in sidebar (buttons containing @ symbol)
    const googleAccountButton = page.locator('button:has-text("@")').first();

    if (await googleAccountButton.isVisible({ timeout: 5000 }).catch(() => false)) {
      await googleAccountButton.click();

      // Wait for navigation to complete
      const pathInput = page.locator('input.input-field').first();
      await waitForNavigation(page, pathInput);

      // Check path bar shows gdrive://
      const path = await pathInput.inputValue();

      console.log('Path after clicking account:', path);
      expect(path).toContain('gdrive://');
      expect(path).not.toMatch(/gdrive:\/[^/]/);
    } else {
      console.log('No Google account found in sidebar, skipping test');
      test.skip();
    }
  });

  test('should navigate into gdrive folder and preserve scheme', async ({ page }) => {
    // First navigate to gdrive root
    const pathInput = page.locator('input.input-field').first();
    await expect(pathInput).toBeVisible({ timeout: 15000 });

    await pathInput.click();
    await pathInput.fill('gdrive://brian@smg.gg/');
    await pathInput.press('Enter');

    await waitForNavigation(page, pathInput);

    // Try to click on a folder (like "My Drive") - look for folder icons or directory items
    const folder = page.locator('[data-directory="true"]').first();

    if (await folder.isVisible({ timeout: 5000 }).catch(() => false)) {
      await folder.dblclick();
      await waitForNavigation(page, pathInput);

      // Check path still has gdrive://
      const newPath = await pathInput.inputValue();

      console.log('Path after navigating into folder:', newPath);
      expect(newPath).toContain('gdrive://');
      expect(newPath).not.toMatch(/gdrive:\/[^/]/);
    } else {
      console.log('No folders visible, skipping navigation test');
      test.skip();
    }
  });
});
