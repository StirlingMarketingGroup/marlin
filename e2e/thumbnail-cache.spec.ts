import { test, expect } from '@playwright/test';
import { getTauriMockScript, getTestUtilsScript, MOCK_DOWNLOADS_DIR } from './tauri-mocks';

// Inject Tauri mocks and test utilities before each test
test.beforeEach(async ({ page }) => {
  await page.addInitScript(getTauriMockScript());
  await page.addInitScript(getTestUtilsScript());
});

test.describe('Thumbnail Cache Invalidation', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');

    // Wait for app to initialize
    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });

    // Navigate to Downloads (which has image files)
    await pathInput.click();
    await pathInput.fill(MOCK_DOWNLOADS_DIR);
    await pathInput.press('Enter');

    // Wait for files to load
    await expect(page.locator('[data-testid="file-item"][data-name="image.png"]')).toBeVisible({
      timeout: 5000,
    });
  });

  test('should refresh thumbnail when file is modified', async ({ page }) => {
    const imagePath = `${MOCK_DOWNLOADS_DIR}/image.png`;

    // Clear any existing thumbnail requests
    await page.evaluate(() => {
      (
        window as unknown as { __TEST_UTILS__: { clearThumbnailRequests: () => void } }
      ).__TEST_UTILS__.clearThumbnailRequests();
    });

    // Get initial thumbnail request count for the image
    const initialRequestCount = await page.evaluate((path) => {
      return (
        window as unknown as {
          __TEST_UTILS__: { getThumbnailRequestCount: (path: string) => number };
        }
      ).__TEST_UTILS__.getThumbnailRequestCount(path);
    }, imagePath);

    // Simulate file modification (like rotating in Preview and saving)
    await page.evaluate((path) => {
      (
        window as unknown as {
          __TEST_UTILS__: { simulateFileModification: (path: string) => void };
        }
      ).__TEST_UTILS__.simulateFileModification(path);
    }, imagePath);

    // Wait for the debounced refresh (500ms debounce + processing time)
    await page.waitForTimeout(1000);

    // Check that a new thumbnail was requested for the modified file
    const finalRequestCount = await page.evaluate((path) => {
      return (
        window as unknown as {
          __TEST_UTILS__: { getThumbnailRequestCount: (path: string) => number };
        }
      ).__TEST_UTILS__.getThumbnailRequestCount(path);
    }, imagePath);

    // Should have at least one more thumbnail request after modification
    expect(finalRequestCount).toBeGreaterThan(initialRequestCount);
  });

  test('should not affect thumbnails of unmodified files', async ({ page }) => {
    const modifiedImagePath = `${MOCK_DOWNLOADS_DIR}/image.png`;
    const unmodifiedImagePath = `${MOCK_DOWNLOADS_DIR}/photo.jpg`;

    // Clear any existing thumbnail requests
    await page.evaluate(() => {
      (
        window as unknown as { __TEST_UTILS__: { clearThumbnailRequests: () => void } }
      ).__TEST_UTILS__.clearThumbnailRequests();
    });

    // Get initial request counts
    const initialUnmodifiedCount = await page.evaluate((path) => {
      return (
        window as unknown as {
          __TEST_UTILS__: { getThumbnailRequestCount: (path: string) => number };
        }
      ).__TEST_UTILS__.getThumbnailRequestCount(path);
    }, unmodifiedImagePath);

    // Simulate modification of only one file
    await page.evaluate((path) => {
      (
        window as unknown as {
          __TEST_UTILS__: { simulateFileModification: (path: string) => void };
        }
      ).__TEST_UTILS__.simulateFileModification(path);
    }, modifiedImagePath);

    // Wait for the debounced refresh
    await page.waitForTimeout(1000);

    // Check that the unmodified file didn't get a new thumbnail request
    const finalUnmodifiedCount = await page.evaluate((path) => {
      return (
        window as unknown as {
          __TEST_UTILS__: { getThumbnailRequestCount: (path: string) => number };
        }
      ).__TEST_UTILS__.getThumbnailRequestCount(path);
    }, unmodifiedImagePath);

    // Unmodified file should not have additional requests
    // (might have 0 or initial requests, but no NEW requests from the modification)
    expect(finalUnmodifiedCount).toBe(initialUnmodifiedCount);
  });

  test('should handle multiple rapid modifications', async ({ page }) => {
    const imagePath = `${MOCK_DOWNLOADS_DIR}/image.png`;

    // Clear any existing thumbnail requests
    await page.evaluate(() => {
      (
        window as unknown as { __TEST_UTILS__: { clearThumbnailRequests: () => void } }
      ).__TEST_UTILS__.clearThumbnailRequests();
    });

    // Simulate multiple rapid modifications (like saving multiple times quickly)
    for (let i = 0; i < 3; i++) {
      await page.evaluate((path) => {
        (
          window as unknown as {
            __TEST_UTILS__: { simulateFileModification: (path: string) => void };
          }
        ).__TEST_UTILS__.simulateFileModification(path);
      }, imagePath);
      await page.waitForTimeout(100); // Small delay between modifications
    }

    // Wait for debounce to settle
    await page.waitForTimeout(1000);

    // Should have at least one thumbnail request (debouncing may combine multiple events)
    const finalRequestCount = await page.evaluate((path) => {
      return (
        window as unknown as {
          __TEST_UTILS__: { getThumbnailRequestCount: (path: string) => number };
        }
      ).__TEST_UTILS__.getThumbnailRequestCount(path);
    }, imagePath);

    expect(finalRequestCount).toBeGreaterThan(0);
  });
});
