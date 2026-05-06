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
      window.__DROP_TEST__ = {
        nativeDropPathsOverride: null,
        nativeDropTargetName: null,
        pasteCalls: [],
        startDragCalls: [],
      };

      internals.invoke = async (cmd: string, args?: unknown) => {
        if (cmd === 'start_native_drag') {
          window.__DROP_TEST__.startDragCalls.push(args);

          return await new Promise((resolve) => {
            setTimeout(() => {
              const targetName = window.__DROP_TEST__.nativeDropTargetName;
              const target =
                targetName === null
                  ? null
                  : document.querySelector<HTMLElement>(
                      `[data-testid="file-item"][data-name="${targetName}"]`
                    );
              const rect = target?.getBoundingClientRect();
              const x = rect ? rect.left + rect.width / 2 : 200;
              const clientY = rect ? rect.top + rect.height / 2 : 200;
              const listeners = window.__TAURI_EVENT_LISTENERS__?.get('drag-drop-event') ?? [];
              const dragArgs = args as { paths?: string[] } | undefined;
              const payload = {
                paths: window.__DROP_TEST__.nativeDropPathsOverride ?? dragArgs?.paths ?? [],
                location: {
                  x,
                  y: window.innerHeight - clientY,
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

              resolve(undefined);
            }, 50);
          });
        }

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

  test('dragging a local file over another file in the same folder is a no-op', async ({
    page,
  }) => {
    await page.evaluate(() => {
      window.__DROP_TEST__.nativeDropTargetName = 'sample.pdf';
      window.__DROP_TEST__.nativeDropPathsOverride = ['/private/var/folders/mock/image.png'];
    });

    const source = page.locator('[data-testid="file-item"][data-name="image.png"]');
    const target = page.locator('[data-testid="file-item"][data-name="sample.pdf"]');
    await expect(source).toBeVisible();
    await expect(target).toBeVisible();

    const sourceBox = await source.boundingBox();
    const targetBox = await target.boundingBox();
    expect(sourceBox).not.toBeNull();
    expect(targetBox).not.toBeNull();

    await page.mouse.move(
      sourceBox!.x + sourceBox!.width / 2,
      sourceBox!.y + sourceBox!.height / 2
    );
    await page.mouse.down();
    await page.mouse.move(
      sourceBox!.x + sourceBox!.width / 2 + 12,
      sourceBox!.y + sourceBox!.height / 2 + 12
    );
    await page.mouse.move(
      targetBox!.x + targetBox!.width / 2,
      targetBox!.y + targetBox!.height / 2
    );
    await page.mouse.up();

    await expect
      .poll(() => page.evaluate(() => window.__DROP_TEST__.startDragCalls.length))
      .toBe(1);
    await page.waitForTimeout(300);

    await expect(page.getByText('Drop failed')).toHaveCount(0);
    await expect(page.getByText('Failed to validate drop')).toHaveCount(0);

    const pasteCalls = await page.evaluate(() => window.__DROP_TEST__.pasteCalls);
    expect(pasteCalls).toEqual([]);
  });
});
