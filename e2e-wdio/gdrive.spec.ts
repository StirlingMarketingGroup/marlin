/**
 * Google Drive E2E Tests using Service Account
 *
 * These tests run against the actual Tauri app using tauri-driver (WebDriver).
 *
 * Prerequisites:
 * - Build the app: npm run tauri build -- --debug
 * - Set GOOGLE_SERVICE_ACCOUNT_KEY_FILE env var
 * - Have folders shared with the service account
 *
 * Run with:
 *   GOOGLE_SERVICE_ACCOUNT_KEY_FILE=/path/to/key.json npm run test:e2e:tauri
 */

const SERVICE_ACCOUNT_EMAIL = 'marlin-e2e-testing@marlin-480721.iam.gserviceaccount.com';

describe('Google Drive Integration', () => {
  it('should load the app and show path bar', async () => {
    // Wait for app to fully load
    await browser.pause(5000);

    // Check for path bar
    const pathBar = await $('[data-testid="path-bar"]');
    await pathBar.waitForDisplayed({ timeout: 30000 });

    const isDisplayed = await pathBar.isDisplayed();
    expect(isDisplayed).toBe(true);
  });

  it('should navigate to service account Google Drive root', async () => {
    // Click path bar to enable editing
    const pathBar = await $('[data-testid="path-bar"]');
    await pathBar.click();

    // Wait for path input to appear
    const pathInput = await $('[data-testid="path-input"]');
    await pathInput.waitForDisplayed({ timeout: 5000 });

    // Clear and enter gdrive path
    await pathInput.clearValue();
    await pathInput.setValue(`gdrive://${SERVICE_ACCOUNT_EMAIL}/`);
    await browser.keys('Enter');

    // Wait for navigation
    await browser.pause(3000);

    // Should show virtual folders
    const myDrive = await $('*=My Drive');
    const isMyDriveDisplayed = await myDrive.isDisplayed().catch(() => false);

    const shared = await $('*=Shared with me');
    const isSharedDisplayed = await shared.isDisplayed().catch(() => false);

    // At least one should be visible
    expect(isMyDriveDisplayed || isSharedDisplayed).toBe(true);
  });

  it('should load Shared with me folder', async () => {
    // Navigate to Shared with me
    const pathBar = await $('[data-testid="path-bar"]');
    await pathBar.click();

    const pathInput = await $('[data-testid="path-input"]');
    await pathInput.waitForDisplayed({ timeout: 5000 });

    await pathInput.clearValue();
    await pathInput.setValue(`gdrive://${SERVICE_ACCOUNT_EMAIL}/Shared with me`);
    await browser.keys('Enter');

    // Wait for navigation
    await browser.pause(5000);

    // Check that path bar shows "Shared with me"
    const pathBarText = await pathBar.getText();
    console.log('Path bar text:', pathBarText);

    expect(pathBarText).toContain('Shared with me');
  });

  it('should navigate into a shared subfolder', async () => {
    // First go to Shared with me
    const pathBar = await $('[data-testid="path-bar"]');
    await pathBar.click();

    const pathInput = await $('[data-testid="path-input"]');
    await pathInput.waitForDisplayed({ timeout: 5000 });

    await pathInput.clearValue();
    await pathInput.setValue(`gdrive://${SERVICE_ACCOUNT_EMAIL}/Shared with me`);
    await browser.keys('Enter');

    await browser.pause(5000);

    // Find a folder to click
    const folder = await $('[data-testid="file-item"][data-directory="true"]');
    const folderExists = await folder.isExisting();

    if (folderExists) {
      const folderName = await folder.getText();
      console.log('Found folder:', folderName);

      // Double-click to navigate into folder
      await folder.doubleClick();
      await browser.pause(3000);

      // Check path changed
      const newPathBarText = await pathBar.getText();
      console.log('Path after navigation:', newPathBarText);

      // Should still have gdrive:// scheme
      expect(newPathBarText).toContain('gdrive://');
    } else {
      console.log('No shared folders found, skipping subfolder test');
    }
  });

  it('should load My Drive for service account', async () => {
    const pathBar = await $('[data-testid="path-bar"]');
    await pathBar.click();

    const pathInput = await $('[data-testid="path-input"]');
    await pathInput.waitForDisplayed({ timeout: 5000 });

    await pathInput.clearValue();
    await pathInput.setValue(`gdrive://${SERVICE_ACCOUNT_EMAIL}/My Drive`);
    await browser.keys('Enter');

    await browser.pause(3000);

    // Check path bar shows My Drive
    const pathBarText = await pathBar.getText();
    console.log('My Drive path:', pathBarText);

    expect(pathBarText).toContain('My Drive');
  });

  it('should show service account in sidebar', async () => {
    // Look for Google account button (contains @)
    const accountButton = await $('button*=@');
    const accountExists = await accountButton.isExisting();

    if (accountExists) {
      const buttonText = await accountButton.getText();
      console.log('Found account button:', buttonText);

      // Click to navigate
      await accountButton.click();
      await browser.pause(3000);

      // Check path bar shows gdrive://
      const pathBar = await $('[data-testid="path-bar"]');
      const pathBarText = await pathBar.getText();

      expect(pathBarText).toContain('gdrive://');
    } else {
      console.log('No Google account button found in sidebar');
    }
  });

  it('should preserve gdrive:// path on reload', async () => {
    // Navigate to gdrive path
    const pathBar = await $('[data-testid="path-bar"]');
    await pathBar.click();

    const pathInput = await $('[data-testid="path-input"]');
    await pathInput.waitForDisplayed({ timeout: 5000 });

    await pathInput.clearValue();
    await pathInput.setValue(`gdrive://${SERVICE_ACCOUNT_EMAIL}/`);
    await browser.keys('Enter');

    await browser.pause(3000);

    // Get path before reload
    const pathBefore = await pathBar.getText();
    console.log('Path before reload:', pathBefore);

    // Reload the app
    await browser.refresh();
    await browser.pause(5000);

    // Check path after reload
    const pathBarAfter = await $('[data-testid="path-bar"]');
    await pathBarAfter.waitForDisplayed({ timeout: 30000 });

    const pathAfter = await pathBarAfter.getText();
    console.log('Path after reload:', pathAfter);

    // Should still have gdrive://
    expect(pathAfter).toContain('gdrive://');
  });
});
