import { test, expect } from '@playwright/test';
import { getTauriMockScript, MOCK_DOWNLOADS_DIR } from './tauri-mocks';

test.describe('Double-click on empty space', () => {
  for (const viewMode of ['list', 'grid'] as const) {
    test(`does nothing in the ${viewMode} view`, async ({ page }) => {
      await page.addInitScript(getTauriMockScript(viewMode));
      await page.goto('/');

      const pathInput = page.locator('[data-testid="path-input"]');
      await expect(pathInput).toBeVisible({ timeout: 15000 });
      await pathInput.fill(MOCK_DOWNLOADS_DIR);
      await pathInput.press('Enter');
      await expect(
        page.locator('[data-testid="file-item"][data-name="sample.pdf"]').first()
      ).toBeVisible({ timeout: 5000 });
      await expect(page.locator(`[data-testid="file-${viewMode}"]`)).toBeVisible();

      const box = await page.locator('[data-main-content]').boundingBox();
      if (!box) throw new Error('the file view is not on screen');
      // Below the last item, in the file view's bottom padding.
      const x = box.x + 40;
      const y = box.y + box.height - 20;
      const hitsEmptySpace = await page.evaluate(
        ({ x, y }) => {
          const el = document.elementFromPoint(x, y);
          return !!el?.closest('[data-main-content]') && !el.closest('[data-file-item="true"]');
        },
        { x, y }
      );
      expect(hitsEmptySpace).toBe(true);

      await page.mouse.dblclick(x, y);

      const after = await page.evaluate(() => {
        const w = window as unknown as {
          __TAURI_INVOKE_LOG__: { cmd: string }[];
          __TOAST_STORE__: { getState: () => { toasts: unknown[] } };
        };
        return {
          createFileCalls: w.__TAURI_INVOKE_LOG__.filter((entry) => entry.cmd === 'create_file')
            .length,
          toasts: w.__TOAST_STORE__.getState().toasts.length,
        };
      });
      expect(after).toEqual({ createFileCalls: 0, toasts: 0 });
    });
  }
});
