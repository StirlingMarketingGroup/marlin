import { test, expect, type Page } from '@playwright/test';
import { getTauriMockScript, MOCK_DOWNLOADS_DIR } from './tauri-mocks';

const MAC_UA =
  'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15';
const LINUX_UA =
  'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36';
const WINDOWS_UA =
  'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36';

const samplePath = `${MOCK_DOWNLOADS_DIR}/sample.pdf`;
const imagePath = `${MOCK_DOWNLOADS_DIR}/image.png`;

const trashedPaths = (page: Page) =>
  page.evaluate(() => {
    const log =
      (
        window as unknown as {
          __TAURI_INVOKE_LOG__?: { cmd: string; args?: { paths?: string[] } }[];
        }
      ).__TAURI_INVOKE_LOG__ ?? [];
    return log.flatMap((entry) => (entry.cmd === 'trash_paths' ? (entry.args?.paths ?? []) : []));
  });

const dialogMessages = (page: Page) =>
  page.evaluate(() => {
    const log =
      (
        window as unknown as {
          __TAURI_INVOKE_LOG__?: { cmd: string; args?: { message?: string } }[];
        }
      ).__TAURI_INVOKE_LOG__ ?? [];
    return log.flatMap((entry) =>
      entry.cmd === 'plugin:dialog|message' && entry.args?.message ? [entry.args.message] : []
    );
  });

const emitMenu = (page: Page, event: string) =>
  page.evaluate((eventName) => {
    const internals = (
      window as unknown as {
        __TAURI_INTERNALS__: { invoke: (cmd: string, args: unknown) => Promise<unknown> };
      }
    ).__TAURI_INTERNALS__;
    return internals.invoke('plugin:event|emit', { event: eventName, payload: null });
  }, event);

const renameInput = (page: Page) =>
  page.locator('[data-testid="file-item"][data-name="sample.pdf"] input');

test.beforeEach(async ({ page }) => {
  await page.addInitScript(getTauriMockScript());
});

test.describe('Delete keys', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');

    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });
    await pathInput.click();
    await pathInput.fill(MOCK_DOWNLOADS_DIR);
    await pathInput.press('Enter');

    await expect(
      page.locator('[data-testid="file-item"][data-name="sample.pdf"]').first()
    ).toBeVisible({ timeout: 5000 });
    await expect(
      page.locator('[data-testid="file-item"][data-name="image.png"]').first()
    ).toBeVisible({ timeout: 5000 });
  });

  test.describe('on macOS', () => {
    // The delete shortcuts read the platform from the user agent.
    test.use({ userAgent: MAC_UA });

    test('Cmd+Backspace moves the selection to the Trash', async ({ page }) => {
      await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
      await page.keyboard.press('Meta+Backspace');

      await expect.poll(() => trashedPaths(page)).toEqual([samplePath]);
      expect(await dialogMessages(page)).toEqual([]);
    });

    test('Cmd+Option+Backspace asks before it deletes permanently', async ({ page }) => {
      await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
      await page.keyboard.press('Meta+Alt+Backspace');

      await expect
        .poll(async () => (await dialogMessages(page)).join('\n'))
        .toContain('Permanently delete sample.pdf');
      expect(await trashedPaths(page)).toEqual([]);
    });

    test('Backspace, Shift+Backspace, Delete, and Cmd+Shift+Backspace do nothing to files', async ({
      page,
    }) => {
      await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
      await page.keyboard.press('Backspace');
      await page.keyboard.press('Shift+Backspace');
      await page.keyboard.press('Delete');
      await page.keyboard.press('Meta+Shift+Backspace');

      expect(await trashedPaths(page)).toEqual([]);
      expect(await dialogMessages(page)).toEqual([]);
    });

    test('the keys do nothing while the rename box has focus', async ({ page }) => {
      await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
      await page.keyboard.press('F2');
      await expect(renameInput(page)).toBeFocused();

      await page.keyboard.press('Meta+Backspace');
      await page.keyboard.press('Meta+Alt+Backspace');

      expect(await trashedPaths(page)).toEqual([]);
      expect(await dialogMessages(page)).toEqual([]);
      await expect(renameInput(page)).toBeVisible();
      await expect(renameInput(page)).toBeFocused();
    });

    test('the keys do nothing while the filter box has focus', async ({ page }) => {
      await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
      await page.keyboard.press('Meta+f');
      await expect(page.locator('[data-testid="filter-input"]')).toBeFocused();

      await page.keyboard.press('Meta+Backspace');
      await page.keyboard.press('Meta+Alt+Backspace');

      expect(await trashedPaths(page)).toEqual([]);
      expect(await dialogMessages(page)).toEqual([]);
    });

    test('Cmd+Backspace is taken even with nothing selected or on a key repeat', async ({
      page,
    }) => {
      // macOS runs the File menu's own Cmd+Backspace when the page leaves the key alone.
      await page.evaluate(() => {
        const log: boolean[] = [];
        (window as unknown as { __backspacePrevented: boolean[] }).__backspacePrevented = log;
        window.addEventListener('keydown', (event) => {
          if (event.key === 'Backspace') log.push(event.defaultPrevented);
        });
      });
      const prevented = () =>
        page.evaluate(
          () => (window as unknown as { __backspacePrevented: boolean[] }).__backspacePrevented
        );

      await page.keyboard.press('Meta+Backspace');
      expect(await prevented()).toEqual([true]);

      await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
      await page.keyboard.down('Meta');
      await page.keyboard.down('Backspace');
      await page.keyboard.down('Backspace');
      await page.keyboard.up('Backspace');
      await page.keyboard.up('Meta');

      expect(await prevented()).toEqual([true, true, true]);
      expect(await trashedPaths(page)).toEqual([samplePath]);
    });

    test('Cmd+Backspace trashes while type-to-filter text shows', async ({ page }) => {
      await page.keyboard.press('s');
      await expect(page.locator('[data-testid="file-item"][data-name="sample.pdf"]')).toBeVisible();
      await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
      await page.keyboard.press('Meta+Backspace');

      await expect.poll(() => trashedPaths(page)).toEqual([samplePath]);
      await expect(page.locator('[data-testid="filter-input"]')).toHaveValue('s');
    });
  });

  for (const [platform, userAgent] of [
    ['Linux', LINUX_UA],
    ['Windows', WINDOWS_UA],
  ] as const) {
    test.describe(`on ${platform}`, () => {
      test.use({ userAgent });

      test('Delete moves the selection to the Trash', async ({ page }) => {
        await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
        await page.keyboard.press('Delete');

        await expect.poll(() => trashedPaths(page)).toEqual([samplePath]);
        expect(await dialogMessages(page)).toEqual([]);
      });

      test('Shift+Delete asks before it deletes permanently', async ({ page }) => {
        await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
        await page.keyboard.press('Shift+Delete');

        await expect
          .poll(async () => (await dialogMessages(page)).join('\n'))
          .toContain('Permanently delete sample.pdf');
        expect(await trashedPaths(page)).toEqual([]);
      });

      test('Backspace, Ctrl+Delete, and Alt+Delete do nothing to files', async ({ page }) => {
        await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
        await page.keyboard.press('Backspace');
        await page.keyboard.press('Control+Delete');
        await page.keyboard.press('Alt+Delete');

        expect(await trashedPaths(page)).toEqual([]);
        expect(await dialogMessages(page)).toEqual([]);
      });

      test('Delete edits the name while the rename box has focus', async ({ page }) => {
        await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
        await page.keyboard.press('F2');
        await expect(renameInput(page)).toBeFocused();
        await renameInput(page).evaluate((input: HTMLInputElement) =>
          input.setSelectionRange(0, 0)
        );
        await page.keyboard.press('Delete');
        await expect(renameInput(page)).toHaveValue('ample.pdf');
        await page.keyboard.press('Shift+Delete');

        expect(await trashedPaths(page)).toEqual([]);
        expect(await dialogMessages(page)).toEqual([]);
      });

      test('the keys do nothing while the path bar has focus', async ({ page }) => {
        await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
        const pathInput = page.locator('[data-testid="path-input"]');
        await pathInput.click();
        await pathInput.fill('abc');
        await pathInput.evaluate((input: HTMLInputElement) => input.setSelectionRange(0, 0));
        await page.keyboard.press('Delete');
        await expect(pathInput).toHaveValue('bc');
        await page.keyboard.press('Shift+Delete');

        expect(await trashedPaths(page)).toEqual([]);
        expect(await dialogMessages(page)).toEqual([]);
      });

      test('Delete trashes every selected file', async ({ page }) => {
        await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
        await page.locator('[data-testid="file-item"][data-name="image.png"]').click({
          modifiers: ['ControlOrMeta'],
        });
        await page.keyboard.press('Delete');

        await expect.poll(() => trashedPaths(page)).toEqual([samplePath, imagePath]);
      });
    });
  }

  test('the menu:trash_selected event moves the selection to the Trash', async ({ page }) => {
    await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
    await emitMenu(page, 'menu:trash_selected');

    await expect.poll(() => trashedPaths(page)).toEqual([samplePath]);
  });

  test('the menu:delete_selected_permanently event asks first', async ({ page }) => {
    await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
    await emitMenu(page, 'menu:delete_selected_permanently');

    await expect
      .poll(async () => (await dialogMessages(page)).join('\n'))
      .toContain('Permanently delete sample.pdf');
  });

  test('the menu events delete nothing while the rename box has focus', async ({ page }) => {
    await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
    await page.keyboard.press('F2');
    await expect(renameInput(page)).toBeFocused();

    await emitMenu(page, 'menu:trash_selected');
    await emitMenu(page, 'menu:delete_selected_permanently');

    expect(await trashedPaths(page)).toEqual([]);
    expect(await dialogMessages(page)).toEqual([]);

    await page.keyboard.press('Escape');
    await expect(renameInput(page)).toHaveCount(0);
    await emitMenu(page, 'menu:trash_selected');

    await expect.poll(() => trashedPaths(page)).toEqual([samplePath]);
  });
});
