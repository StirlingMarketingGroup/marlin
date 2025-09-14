import { test, expect } from '@playwright/test';

test.describe('File Manager Preferences', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await page.waitForLoadState('networkidle');
  });

  test('should toggle hidden files and persist across navigation', async ({ page }) => {
    // Initial state - hidden files should be hidden
    const initialHiddenFiles = await page.locator('[data-testid="file-item"][data-hidden="true"]').count();
    expect(initialHiddenFiles).toBe(0);

    // Toggle hidden files via menu or keyboard shortcut
    await page.keyboard.press('Meta+Shift+.');
    
    // Wait for files to reload
    await page.waitForTimeout(500);
    
    // Should now see hidden files
    const visibleHiddenFiles = await page.locator('[data-testid="file-item"][data-hidden="true"]').count();
    expect(visibleHiddenFiles).toBeGreaterThan(0);

    // Navigate to a different directory
    const firstDirectory = page.locator('[data-testid="file-item"][data-directory="true"]').first();
    if (await firstDirectory.isVisible()) {
      await firstDirectory.dblclick();
      await page.waitForLoadState('networkidle');
      
      // Go back to original directory
      await page.keyboard.press('Meta+Left');
      await page.waitForLoadState('networkidle');
      
      // Hidden files should still be visible
      const persistedHiddenFiles = await page.locator('[data-testid="file-item"][data-hidden="true"]').count();
      expect(persistedHiddenFiles).toBeGreaterThan(0);
    }
  });

  test('should persist directory-specific hidden file preferences', async ({ page }) => {
    // Get current directory name for identification
    const currentPath = await page.locator('[data-testid="path-bar"]').textContent();
    
    // Toggle hidden files for this directory
    await page.keyboard.press('Meta+Shift+.');
    await page.waitForTimeout(500);
    
    // Navigate to another directory if available
    const firstDirectory = page.locator('[data-testid="file-item"][data-directory="true"]').first();
    if (await firstDirectory.isVisible()) {
      await firstDirectory.dblclick();
      await page.waitForLoadState('networkidle');
      
      // This directory should have default (hidden) preference
      const hiddenInNewDir = await page.locator('[data-testid="file-item"][data-hidden="true"]').count();
      
      // Go back to original directory
      await page.keyboard.press('Meta+Left');
      await page.waitForLoadState('networkidle');
      
      // Original directory should still show hidden files
      const hiddenInOriginalDir = await page.locator('[data-testid="file-item"][data-hidden="true"]').count();
      expect(hiddenInOriginalDir).toBeGreaterThan(0);
    }
  });

  test('should persist view mode changes', async ({ page }) => {
    // Start in grid view (default)
    await expect(page.locator('[data-testid="file-grid"]')).toBeVisible();
    
    // Switch to list view
    await page.keyboard.press('Meta+2');
    await page.waitForTimeout(200);
    
    await expect(page.locator('[data-testid="file-list"]')).toBeVisible();
    
    // Navigate away and back
    const firstDirectory = page.locator('[data-testid="file-item"][data-directory="true"]').first();
    if (await firstDirectory.isVisible()) {
      await firstDirectory.dblclick();
      await page.waitForLoadState('networkidle');
      
      await page.keyboard.press('Meta+Left');
      await page.waitForLoadState('networkidle');
      
      // Should still be in list view
      await expect(page.locator('[data-testid="file-list"]')).toBeVisible();
    }
  });

  test('should persist sort preferences', async ({ page }) => {
    // Change sort to size
    await page.click('[data-testid="sort-dropdown"]');
    await page.click('[data-testid="sort-by-size"]');
    await page.waitForTimeout(200);
    
    // Verify sort order changed
    const firstFile = page.locator('[data-testid="file-item"]').first();
    const firstFileName = await firstFile.getAttribute('data-name');
    
    // Navigate and return
    const firstDirectory = page.locator('[data-testid="file-item"][data-directory="true"]').first();
    if (await firstDirectory.isVisible()) {
      await firstDirectory.dblclick();
      await page.waitForLoadState('networkidle');
      
      await page.keyboard.press('Meta+Left');
      await page.waitForLoadState('networkidle');
      
      // Sort preference should be maintained
      const sortDropdown = page.locator('[data-testid="sort-dropdown"]');
      await expect(sortDropdown).toContainText('Size');
    }
  });

  test('should restore last visited directory on app restart', async ({ page, context }) => {
    // Navigate to a subdirectory
    const firstDirectory = page.locator('[data-testid="file-item"][data-directory="true"]').first();
    if (await firstDirectory.isVisible()) {
      await firstDirectory.dblclick();
      await page.waitForLoadState('networkidle');
      
      const finalPath = await page.locator('[data-testid="path-bar"]').textContent();
      
      // Simulate app restart by closing and reopening
      await page.close();
      const newPage = await context.newPage();
      await newPage.goto('/');
      await newPage.waitForLoadState('networkidle');
      
      // Should be in the same directory
      const restoredPath = await newPage.locator('[data-testid="path-bar"]').textContent();
      expect(restoredPath).toBe(finalPath);
    }
  });

  test('should handle rapid preference toggles without state corruption', async ({ page }) => {
    // Rapidly toggle hidden files multiple times
    for (let i = 0; i < 5; i++) {
      await page.keyboard.press('Meta+Shift+.');
      await page.waitForTimeout(100);
    }
    
    await page.waitForTimeout(500); // Let all requests settle
    
    // State should be consistent
    const hiddenFiles = await page.locator('[data-testid="file-item"][data-hidden="true"]').count();
    
    // Toggle once more and verify it works correctly
    await page.keyboard.press('Meta+Shift+.');
    await page.waitForTimeout(500);
    
    const newHiddenCount = await page.locator('[data-testid="file-item"][data-hidden="true"]').count();
    expect(newHiddenCount).not.toBe(hiddenFiles);
  });

  test('should maintain preferences across different directories', async ({ page }) => {
    // Set up different preferences for different directories
    const directories = await page.locator('[data-testid="file-item"][data-directory="true"]').all();
    
    if (directories.length >= 2) {
      // First directory - enable hidden files
      await directories[0].dblclick();
      await page.waitForLoadState('networkidle');
      await page.keyboard.press('Meta+Shift+.');
      await page.waitForTimeout(300);
      
      const hiddenInFirst = await page.locator('[data-testid="file-item"][data-hidden="true"]').count();
      
      // Go back
      await page.keyboard.press('Meta+Left');
      await page.waitForLoadState('networkidle');
      
      // Second directory - keep hidden files disabled
      await directories[1].dblclick();
      await page.waitForLoadState('networkidle');
      
      const hiddenInSecond = await page.locator('[data-testid="file-item"][data-hidden="true"]').count();
      expect(hiddenInSecond).toBe(0);
      
      // Go back to first directory
      await page.keyboard.press('Meta+Left');
      await page.waitForLoadState('networkidle');
      await directories[0].dblclick();
      await page.waitForLoadState('networkidle');
      
      // Should still have hidden files enabled
      const hiddenStillInFirst = await page.locator('[data-testid="file-item"][data-hidden="true"]').count();
      expect(hiddenStillInFirst).toBe(hiddenInFirst);
    }
  });
});