import { test, expect } from '@playwright/test';
import { getTauriMockScript } from './tauri-mocks';

// Inject Tauri mocks before each test
test.beforeEach(async ({ page }) => {
  await page.addInitScript(getTauriMockScript());
});

test.describe('Undo Store Tests', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');

    // Wait for app to initialize
    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });
  });

  test('should show "Nothing to undo" when undo stack is empty', async ({ page }) => {
    // Trigger undo via the store
    await page.evaluate(async () => {
      const store = (window as any).__UNDO_STORE__;
      if (store) {
        await store.getState().executeUndo();
      }
    });

    // Wait for toast to appear
    await page.waitForTimeout(500);

    // Check that the "Nothing to undo" toast is visible
    const toast = page.locator('text=Nothing to undo');
    await expect(toast).toBeVisible({ timeout: 5000 });
  });

  test('should push and pop undo entries', async ({ page }) => {
    // Push an entry and verify it's in the stack
    const result = await page.evaluate(() => {
      const store = (window as any).__UNDO_STORE__;
      if (store) {
        store
          .getState()
          .pushUndo(
            { type: 'rename', originalPath: '/test/old.txt', newPath: '/test/new.txt' },
            'Rename old.txt to new.txt'
          );
        return store.getState().stack.length;
      }
      return 0;
    });

    expect(result).toBe(1);

    // Pop the entry and verify stack is empty
    const afterPop = await page.evaluate(() => {
      const store = (window as any).__UNDO_STORE__;
      if (store) {
        const entry = store.getState().popUndo();
        return {
          stackLength: store.getState().stack.length,
          hasEntry: !!entry,
          entryDescription: entry?.description,
        };
      }
      return { stackLength: 0, hasEntry: false, entryDescription: '' };
    });

    expect(afterPop.stackLength).toBe(0);
    expect(afterPop.hasEntry).toBe(true);
    expect(afterPop.entryDescription).toBe('Rename old.txt to new.txt');
  });

  test('should enforce max stack size', async ({ page }) => {
    const stackLength = await page.evaluate(() => {
      const store = (window as any).__UNDO_STORE__;
      if (store) {
        // Push 15 entries (max is 10)
        for (let i = 0; i < 15; i++) {
          store.getState().pushUndo(
            {
              type: 'rename',
              originalPath: `/test/file${i}.txt`,
              newPath: `/test/renamed${i}.txt`,
            },
            `Rename file${i}.txt`
          );
        }
        return store.getState().stack.length;
      }
      return 0;
    });

    // Should be capped at 10
    expect(stackLength).toBe(10);
  });

  test('should remove entry by ID', async ({ page }) => {
    const result = await page.evaluate(() => {
      const store = (window as any).__UNDO_STORE__;
      if (store) {
        const id = store
          .getState()
          .pushUndo({ type: 'copy', copiedPaths: ['/test/copy.txt'] }, 'Copy 1 item');
        const beforeRemove = store.getState().stack.length;
        store.getState().removeById(id);
        const afterRemove = store.getState().stack.length;
        return { beforeRemove, afterRemove };
      }
      return { beforeRemove: 0, afterRemove: 0 };
    });

    expect(result.beforeRemove).toBe(1);
    expect(result.afterRemove).toBe(0);
  });

  test('should expire entries after TTL', async ({ page }) => {
    // This test manipulates timestamps to test TTL without waiting 5 minutes
    const result = await page.evaluate(() => {
      const store = (window as any).__UNDO_STORE__;
      if (store) {
        // Push an entry
        store
          .getState()
          .pushUndo(
            { type: 'trash', undoToken: 'test-token', trashedPaths: ['/test/deleted.txt'] },
            'Trash deleted.txt'
          );

        // Manually expire the entry by backdating it
        const state = store.getState();
        if (state.stack.length > 0) {
          // Set createdAt to 6 minutes ago (TTL is 5 minutes)
          state.stack[0].createdAt = Date.now() - 6 * 60 * 1000;
        }

        // Now getValidStack should return empty
        const validStack = store.getState().getValidStack();
        return validStack.length;
      }
      return -1;
    });

    expect(result).toBe(0);
  });
});
