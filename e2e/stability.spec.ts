import { test, expect } from '@playwright/test';
import { getTauriMockScript, MOCK_DOWNLOADS_DIR } from './tauri-mocks';

// Type definition for window extensions used in stability tests
interface StabilityTestWindow extends Window {
  __FILE_LIST_MUTATIONS__: Array<{
    time: number;
    type: string;
    target: string;
    addedNodes: number;
    removedNodes: number;
    attributeName: string | null;
  }>;
  __RENDER_TIMESTAMPS__: Array<{
    time: number;
    itemCount: number;
    items: Array<{ path: string | null; top: number }>;
  }>;
  __recordRenderState__: () => void;
  __setupMutationObserver__: () => boolean;
  __mutationObserver__?: MutationObserver;
  __getMutationsSince__: (since: number) => StabilityTestWindow['__FILE_LIST_MUTATIONS__'];
  __clearMutations__: () => void;
}

// Helper to inject render tracking
function getRenderTrackingScript(): string {
  return `
    (function() {
      // Track DOM mutations on file items
      window.__FILE_LIST_MUTATIONS__ = [];
      window.__RENDER_TIMESTAMPS__ = [];

      // Record initial state after load
      window.__recordRenderState__ = function() {
        const items = document.querySelectorAll('[data-testid="file-item"]');
        window.__RENDER_TIMESTAMPS__.push({
          time: performance.now(),
          itemCount: items.length,
          // Store a snapshot of item positions
          items: Array.from(items).map(el => ({
            path: el.getAttribute('data-file-path'),
            top: el.getBoundingClientRect().top
          }))
        });
      };

      // Set up mutation observer on the file grid/list container
      window.__setupMutationObserver__ = function() {
        const container = document.querySelector('[data-testid="file-grid"], [data-testid="file-list"]');
        if (!container) {
          console.warn('No file container found');
          return false;
        }

        const observer = new MutationObserver((mutations) => {
          const now = performance.now();
          mutations.forEach(m => {
            window.__FILE_LIST_MUTATIONS__.push({
              time: now,
              type: m.type,
              target: m.target.nodeName,
              addedNodes: m.addedNodes.length,
              removedNodes: m.removedNodes.length,
              attributeName: m.attributeName
            });
          });
        });

        observer.observe(container, {
          childList: true,
          subtree: true,
          attributes: true,
          attributeFilter: ['style', 'class', 'data-file-path']
        });

        window.__mutationObserver__ = observer;
        return true;
      };

      // Get mutations since a timestamp
      window.__getMutationsSince__ = function(since) {
        return window.__FILE_LIST_MUTATIONS__.filter(m => m.time >= since);
      };

      // Clear mutations
      window.__clearMutations__ = function() {
        window.__FILE_LIST_MUTATIONS__ = [];
      };

      console.log('[Stability] Render tracking injected');
    })();
  `;
}

test.describe('Render Stability', () => {
  test.beforeEach(async ({ page }) => {
    // Inject Tauri mocks
    await page.addInitScript(getTauriMockScript());
    // Inject render tracking
    await page.addInitScript(getRenderTrackingScript());
  });

  test('clicking a file should not cause file list to re-render', async ({ page }) => {
    await page.goto('/');

    // Wait for app to initialize
    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });

    // Navigate to Downloads
    await pathInput.click();
    await pathInput.fill(MOCK_DOWNLOADS_DIR);
    await pathInput.press('Enter');

    // Wait for files to load
    await expect(
      page.locator('[data-testid="file-item"][data-name="sample.pdf"]').first()
    ).toBeVisible({ timeout: 5000 });
    await expect(
      page.locator('[data-testid="file-item"][data-name="image.png"]').first()
    ).toBeVisible({ timeout: 5000 });

    // Give time for any initial settling
    await page.waitForTimeout(500);

    // Set up mutation observer
    const observerReady = await page.evaluate(() => {
      return (window as unknown as StabilityTestWindow).__setupMutationObserver__();
    });
    expect(observerReady).toBe(true);

    // Clear any existing mutations
    await page.evaluate(() => (window as unknown as StabilityTestWindow).__clearMutations__());

    // Record the timestamp before clicking
    const beforeClick = await page.evaluate(() => performance.now());

    // Get initial positions of file items
    const initialPositions = await page.evaluate(() => {
      const items = document.querySelectorAll('[data-testid="file-item"]');
      return Array.from(items).map((el) => ({
        path: el.getAttribute('data-file-path'),
        top: Math.round(el.getBoundingClientRect().top),
        left: Math.round(el.getBoundingClientRect().left),
      }));
    });

    console.log('Initial positions:', initialPositions);

    // Click on a file (single click to select)
    const fileItem = page.locator('[data-testid="file-item"][data-name="image.png"]');
    await fileItem.click();

    // Wait a moment for any re-renders to occur
    await page.waitForTimeout(300);

    // Get positions after click
    const afterPositions = await page.evaluate(() => {
      const items = document.querySelectorAll('[data-testid="file-item"]');
      return Array.from(items).map((el) => ({
        path: el.getAttribute('data-file-path'),
        top: Math.round(el.getBoundingClientRect().top),
        left: Math.round(el.getBoundingClientRect().left),
      }));
    });

    console.log('After positions:', afterPositions);

    // Get mutations that occurred
    const mutations = await page.evaluate((since) => {
      return (window as unknown as StabilityTestWindow).__getMutationsSince__(since);
    }, beforeClick);

    console.log(`Mutations detected: ${mutations.length}`);
    if (mutations.length > 0) {
      console.log('Mutation details:', JSON.stringify(mutations.slice(0, 10), null, 2));
    }

    // Check that positions didn't change (no flash/redraw)
    for (let i = 0; i < initialPositions.length; i++) {
      const before = initialPositions[i];
      const after = afterPositions.find((p) => p.path === before.path);

      if (after) {
        const topDiff = Math.abs(after.top - before.top);
        const leftDiff = Math.abs(after.left - before.left);

        if (topDiff > 1 || leftDiff > 1) {
          console.log(
            `Position changed for ${before.path}: top ${before.top} -> ${after.top}, left ${before.left} -> ${after.left}`
          );
        }

        // Allow 1px tolerance for subpixel rendering
        expect(topDiff).toBeLessThanOrEqual(1);
        expect(leftDiff).toBeLessThanOrEqual(1);
      }
    }

    // Check that no major DOM mutations occurred (childList additions/removals)
    // Selection class changes are expected, but element recreation is not
    const structuralMutations = mutations.filter(
      (m) => m.type === 'childList' && (m.addedNodes > 0 || m.removedNodes > 0)
    );

    console.log(`Structural mutations (childList): ${structuralMutations.length}`);

    // There shouldn't be any file items being added/removed on a simple click
    // (Some minor mutations for selection state are OK)
    expect(structuralMutations.length).toBeLessThan(5);
  });

  test('double-clicking to open a file should not cause flash', async ({ page }) => {
    await page.goto('/');

    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });

    // Navigate to Downloads
    await pathInput.click();
    await pathInput.fill(MOCK_DOWNLOADS_DIR);
    await pathInput.press('Enter');

    // Wait for files to load
    await expect(
      page.locator('[data-testid="file-item"][data-name="image.png"]').first()
    ).toBeVisible({ timeout: 5000 });

    // Give time for settling
    await page.waitForTimeout(500);

    // Set up mutation observer
    const observerReady = await page.evaluate(() => {
      return (window as unknown as StabilityTestWindow).__setupMutationObserver__();
    });
    expect(observerReady).toBe(true);

    await page.evaluate(() => (window as unknown as StabilityTestWindow).__clearMutations__());

    const beforeClick = await page.evaluate(() => performance.now());

    // Double-click to open the file
    const fileItem = page.locator('[data-testid="file-item"][data-name="image.png"]');
    await fileItem.dblclick();

    // Wait for any re-renders
    await page.waitForTimeout(300);

    // Get mutations
    const mutations = await page.evaluate((since) => {
      return (window as unknown as StabilityTestWindow).__getMutationsSince__(since);
    }, beforeClick);

    console.log(`\n=== DOUBLE-CLICK STABILITY TEST ===`);
    console.log(`Total mutations: ${mutations.length}`);

    // Categorize mutations
    const childListMutations = mutations.filter((m) => m.type === 'childList');
    const attributeMutations = mutations.filter((m) => m.type === 'attributes');

    console.log(`\nChildList mutations: ${childListMutations.length}`);
    console.log(`Attribute mutations: ${attributeMutations.length}`);

    if (childListMutations.length > 0) {
      console.log('ChildList details:', JSON.stringify(childListMutations.slice(0, 5), null, 2));
    }

    // For a double-click that opens a file externally, there should be
    // minimal DOM changes - just selection state updates
    // If we see many childList mutations with addedNodes/removedNodes, that's a re-render
    const heavyMutations = childListMutations.filter((m) => m.addedNodes > 2 || m.removedNodes > 2);

    console.log(`Heavy mutations (>2 nodes): ${heavyMutations.length}`);

    // This is the key assertion - opening a file shouldn't cause major DOM churn
    expect(heavyMutations.length).toBe(0);
  });
});
