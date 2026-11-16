import { test, expect, type Page } from '@playwright/test';
import {
  getTauriMockScript,
  MOCK_DOWNLOADS_DIR,
  MOCK_HOME_DIR,
  MOCK_PERCENT_DIR,
} from './tauri-mocks';

// macOS hands several files to one open_paths call. Every other open is one shell call per file.
const openedPaths = (page: Page) =>
  page.evaluate(() => {
    const log =
      (
        window as unknown as {
          __TAURI_INVOKE_LOG__?: { cmd: string; args?: { path?: string; paths?: string[] } }[];
        }
      ).__TAURI_INVOKE_LOG__ ?? [];
    return log.flatMap((entry) => {
      if (entry.cmd === 'plugin:shell|open') return [entry.args?.path];
      if (entry.cmd === 'open_paths') return entry.args?.paths ?? [];
      return [];
    });
  });

test.beforeEach(async ({ page }) => {
  await page.addInitScript(getTauriMockScript());
});

test.describe('Open selected', () => {
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

  test('opens each selected file', async ({ page }) => {
    await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
    await page.locator('[data-testid="file-item"][data-name="image.png"]').click({
      modifiers: ['ControlOrMeta'],
    });

    await page.keyboard.press('ControlOrMeta+o');

    await expect
      .poll(() => openedPaths(page))
      .toEqual([`${MOCK_DOWNLOADS_DIR}/sample.pdf`, `${MOCK_DOWNLOADS_DIR}/image.png`]);
  });

  test.describe('on Linux', () => {
    // The Enter shortcut reads the platform from the user agent.
    test.use({
      userAgent:
        'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36',
    });

    test('Enter in the path bar goes to the typed folder and opens nothing', async ({ page }) => {
      await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();

      const pathInput = page.locator('[data-testid="path-input"]');
      await page.keyboard.press('ControlOrMeta+l');
      await expect(pathInput).toBeFocused();
      await pathInput.fill(MOCK_HOME_DIR);
      await pathInput.press('Enter');

      await expect(page.locator('[data-testid="file-item"][data-name="Documents"]')).toBeVisible();
      await expect(pathInput).toHaveValue(MOCK_HOME_DIR);
      expect(await openedPaths(page)).toEqual([]);
    });

    test('Enter opens the selection, and a focused button keeps its own Enter', async ({
      page,
    }) => {
      await page.locator('[data-testid="file-item"][data-name="sample.pdf"]').click();
      await page.keyboard.press('Enter');
      await expect.poll(() => openedPaths(page)).toEqual([`${MOCK_DOWNLOADS_DIR}/sample.pdf`]);

      // Tab reaches a button: Enter presses it.
      await page.locator('button[title^="Refresh"]').focus();
      await page.keyboard.press('Enter');
      await expect(page.locator('[data-testid="file-item"][data-name="sample.pdf"]')).toBeVisible();
      expect(await openedPaths(page)).toEqual([`${MOCK_DOWNLOADS_DIR}/sample.pdf`]);

      // A clicked button keeps the focus in Chromium. The arrow keys give the keyboard to the files.
      await page.locator('button[title^="Refresh"]').click();
      await expect(page.locator('[data-testid="file-item"][data-name="sample.pdf"]')).toBeVisible();
      await page.keyboard.press('ArrowDown');
      await page.keyboard.press('Enter');
      await expect.poll(async () => (await openedPaths(page)).length).toBe(2);
    });
  });
});

test.describe('Window start path', () => {
  test('loads a folder whose name contains % and &', async ({ page }) => {
    await page.goto('/?path=' + encodeURIComponent(MOCK_PERCENT_DIR));

    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });
    await expect(pathInput).toHaveValue(MOCK_PERCENT_DIR);
    await expect(page.locator('[data-testid="file-item"][data-name="note.txt"]')).toBeVisible({
      timeout: 5000,
    });
  });
});
