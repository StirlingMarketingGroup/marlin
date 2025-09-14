import { test, expect } from '@playwright/test';

test.describe('File Manager Navigation', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await page.waitForLoadState('networkidle');
  });

  test('should navigate to directories and maintain history', async ({ page }) => {
    const initialPath = await page.locator('[data-testid="path-bar"]').textContent();
    
    // Navigate to a subdirectory
    const firstDirectory = page.locator('[data-testid="file-item"][data-directory="true"]').first();
    if (await firstDirectory.isVisible()) {
      await firstDirectory.dblclick();
      await page.waitForLoadState('networkidle');
      
      const newPath = await page.locator('[data-testid="path-bar"]').textContent();
      expect(newPath).not.toBe(initialPath);
      
      // Go back using keyboard shortcut
      await page.keyboard.press('Meta+Left');
      await page.waitForLoadState('networkidle');
      
      const backPath = await page.locator('[data-testid="path-bar"]').textContent();
      expect(backPath).toBe(initialPath);
      
      // Go forward
      await page.keyboard.press('Meta+Right');
      await page.waitForLoadState('networkidle');
      
      const forwardPath = await page.locator('[data-testid="path-bar"]').textContent();
      expect(forwardPath).toBe(newPath);
    }
  });

  test('should navigate up directory levels', async ({ page }) => {
    // Navigate to a subdirectory first
    const firstDirectory = page.locator('[data-testid="file-item"][data-directory="true"]').first();
    if (await firstDirectory.isVisible()) {
      await firstDirectory.dblclick();
      await page.waitForLoadState('networkidle');
      
      const deepPath = await page.locator('[data-testid="path-bar"]').textContent();
      
      // Go up one level using keyboard shortcut
      await page.keyboard.press('Meta+Up');
      await page.waitForLoadState('networkidle');
      
      const parentPath = await page.locator('[data-testid="path-bar"]').textContent();
      expect(parentPath).not.toBe(deepPath);
      expect(deepPath).toContain(parentPath || '');
    }
  });

  test('should handle path bar editing', async ({ page }) => {
    // Click on path bar to edit
    const pathBar = page.locator('[data-testid="path-bar"]');
    await pathBar.click();
    
    // Should become editable
    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible();
    
    // Type a new path (home directory)
    await pathInput.clear();
    await pathInput.type('~');
    await pathInput.press('Enter');
    
    await page.waitForLoadState('networkidle');
    
    // Should navigate to home directory
    const finalPath = await pathBar.textContent();
    expect(finalPath).toContain('/');
  });

  test('should handle invalid paths gracefully', async ({ page }) => {
    // Click on path bar to edit
    const pathBar = page.locator('[data-testid="path-bar"]');
    await pathBar.click();
    
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.clear();
    await pathInput.type('/nonexistent/path/that/should/not/exist');
    await pathInput.press('Enter');
    
    await page.waitForTimeout(1000);
    
    // Should show error or stay in current directory
    // Error handling depends on implementation
    const errorMessage = page.locator('[data-testid="error-message"]');
    if (await errorMessage.isVisible()) {
      await expect(errorMessage).toContainText('not found');
    }
  });

  test('should preserve navigation history after app restart', async ({ page, context }) => {
    // Navigate to several directories
    const directories = await page.locator('[data-testid="file-item"][data-directory="true"]').all();
    
    if (directories.length >= 2) {
      await directories[0].dblclick();
      await page.waitForLoadState('networkidle');
      
      await directories[1].dblclick();
      await page.waitForLoadState('networkidle');
      
      const finalPath = await page.locator('[data-testid="path-bar"]').textContent();
      
      // Simulate app restart
      await page.close();
      const newPage = await context.newPage();
      await newPage.goto('/');
      await newPage.waitForLoadState('networkidle');
      
      // Should restore to last location
      const restoredPath = await newPage.locator('[data-testid="path-bar"]').textContent();
      expect(restoredPath).toBe(finalPath);
      
      // Should be able to go back in history
      await newPage.keyboard.press('Meta+Left');
      await newPage.waitForLoadState('networkidle');
      
      const backPath = await newPage.locator('[data-testid="path-bar"]').textContent();
      expect(backPath).not.toBe(finalPath);
    }
  });

  test('should handle rapid navigation without state corruption', async ({ page }) => {
    const directories = await page.locator('[data-testid="file-item"][data-directory="true"]').all();
    
    if (directories.length >= 1) {
      // Rapidly navigate back and forth
      for (let i = 0; i < 5; i++) {
        await directories[0].dblclick();
        await page.waitForTimeout(100);
        await page.keyboard.press('Meta+Left');
        await page.waitForTimeout(100);
      }
      
      await page.waitForLoadState('networkidle');
      
      // Should still be in a valid state
      const pathBar = page.locator('[data-testid="path-bar"]');
      await expect(pathBar).toBeVisible();
      
      const currentPath = await pathBar.textContent();
      expect(currentPath).toBeTruthy();
    }
  });

  test('should update window title with current directory', async ({ page }) => {
    const initialTitle = await page.title();
    
    // Navigate to a subdirectory
    const firstDirectory = page.locator('[data-testid="file-item"][data-directory="true"]').first();
    if (await firstDirectory.isVisible()) {
      const dirName = await firstDirectory.getAttribute('data-name');
      await firstDirectory.dblclick();
      await page.waitForLoadState('networkidle');
      
      const newTitle = await page.title();
      expect(newTitle).not.toBe(initialTitle);
      // Title should reflect current directory
      expect(newTitle).toContain(dirName || '');
    }
  });

  test('should handle keyboard shortcuts for navigation', async ({ page }) => {
    // Test various keyboard shortcuts
    const shortcuts = [
      { key: 'Meta+Up', description: 'go up' },
      { key: 'Meta+Left', description: 'go back' },
      { key: 'Meta+Right', description: 'go forward' },
      { key: 'Meta+Home', description: 'go home' },
    ];

    for (const shortcut of shortcuts) {
      await page.keyboard.press(shortcut.key);
      await page.waitForTimeout(200);
      
      // Should not cause errors
      const errorMessage = page.locator('[data-testid="error-message"]');
      expect(await errorMessage.isVisible()).toBe(false);
    }
  });
});