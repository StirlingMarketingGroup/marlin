import { test, expect } from '@playwright/test';
import { getTauriMockScript, MOCK_DOWNLOADS_DIR } from './tauri-mocks';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(getTauriMockScript());
});

test.describe('Same-directory drops', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');

    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });
    await pathInput.click();
    await pathInput.fill(MOCK_DOWNLOADS_DIR);
    await pathInput.press('Enter');

    await expect(page.locator('[data-testid="file-item"][data-name="image.png"]')).toBeVisible({
      timeout: 5000,
    });

    await page.evaluate(() => {
      const internals = window.__TAURI_INTERNALS__;
      const originalInvoke = internals.invoke;
      window.__DROP_TEST__ = { pasteCalls: [] };

      internals.invoke = async (cmd: string, args?: unknown) => {
        if (cmd === 'paste_items_to_location') {
          window.__DROP_TEST__.pasteCalls.push(args);
          throw new Error('same-directory drops should not paste');
        }

        return originalInvoke(cmd, args);
      };
    });
  });

  test('dropping a file back into its current folder is a no-op', async ({ page }) => {
    await page.evaluate((filePath) => {
      const listeners = window.__TAURI_EVENT_LISTENERS__?.get('drag-drop-event') ?? [];
      const payload = {
        paths: [filePath],
        location: {
          x: 200,
          y: 200,
          targetId: 'file-panel',
        },
        eventType: 'drop',
        modifiers: {
          optionAlt: false,
          cmdCtrl: false,
        },
      };

      for (const handler of listeners) {
        window.__TAURI_INTERNALS__.runCallback(handler, {
          event: 'drag-drop-event',
          payload,
        });
      }
    }, `${MOCK_DOWNLOADS_DIR}/image.png`);

    await page.waitForTimeout(300);

    await expect(page.getByText('Drop failed')).toHaveCount(0);

    const pasteCalls = await page.evaluate(() => window.__DROP_TEST__.pasteCalls);
    expect(pasteCalls).toEqual([]);
  });
});
