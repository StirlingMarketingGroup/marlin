import { test, expect, type Locator, type Page } from '@playwright/test';
import { getTauriMockScript, MOCK_HOME_DIR, MOCK_DOWNLOADS_DIR } from './tauri-mocks';

async function typeAtEnd(pathInput: Locator, text: string, options?: { delay: number }) {
  await pathInput.click();
  await pathInput.press('End');
  await pathInput.pressSequentially(text, options);
}

// Makes every `read_directory` call answer 300 ms late and records its path in `window.__READ_DIR_PATHS__`.
async function recordSlowListings(page: Page) {
  await page.evaluate(() => {
    const w = window as Window & {
      __READ_DIR_PATHS__?: string[];
      __TAURI_INTERNALS__: {
        invoke: (cmd: string, args?: { path?: string }) => Promise<unknown>;
      };
    };
    const paths: string[] = [];
    w.__READ_DIR_PATHS__ = paths;
    const original = w.__TAURI_INTERNALS__.invoke.bind(w.__TAURI_INTERNALS__);
    w.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
      if (cmd === 'read_directory') {
        paths.push(args?.path ?? '');
        await new Promise((resolve) => setTimeout(resolve, 300));
      }
      return original(cmd, args);
    };
  });
}

test.describe('Path bar', () => {
  test.beforeEach(async ({ page }) => {
    await page.addInitScript(getTauriMockScript());
    await page.goto('/');

    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });
  });

  test('typing at the end lists the folders that match', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await typeAtEnd(pathInput, '/D');

    await expect(page.getByRole('option')).toHaveText(['Documents/', 'Downloads/']);

    await pathInput.pressSequentially('ow');
    await expect(page.getByRole('option')).toHaveText(['Downloads/']);
  });

  test('an edit in the middle of the path opens no list and loses no text', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');

    await pathInput.click();
    await pathInput.fill(`${MOCK_HOME_DIR}\\tail/more`);
    await pathInput.evaluate((el, start: number) => {
      (el as HTMLInputElement).setSelectionRange(start, start + 1);
    }, MOCK_HOME_DIR.length);
    await page.keyboard.type('/');
    await page.waitForTimeout(400);

    await expect(page.getByRole('listbox')).toHaveCount(0);
    await expect(pathInput).toHaveValue(`${MOCK_HOME_DIR}/tail/more`);
  });

  test('pointing at a suggestion highlights it and leaves the text alone', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await typeAtEnd(pathInput, '/D');

    const first = page.getByRole('option').first();
    await first.hover();

    await expect(pathInput).toHaveValue(`${MOCK_HOME_DIR}/D`);
    await expect(first).toHaveAttribute('aria-selected', 'true');
  });

  test('arrow keys highlight without changing the text, and Enter opens the highlighted folder', async ({
    page,
  }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await typeAtEnd(pathInput, '/D');

    await pathInput.press('ArrowDown');
    await pathInput.press('ArrowDown');
    await expect(pathInput).toHaveValue(`${MOCK_HOME_DIR}/D`);
    await expect(page.getByRole('option').nth(1)).toHaveAttribute('aria-selected', 'true');

    await pathInput.press('ArrowUp');
    await pathInput.press('ArrowUp');
    await pathInput.press('ArrowUp');
    await expect(page.getByRole('option', { selected: true })).toHaveCount(0);

    await pathInput.press('ArrowDown');
    await pathInput.press('ArrowDown');
    await pathInput.press('Enter');

    await expect(
      page.locator('[data-testid="file-item"][data-name="sample.pdf"]').first()
    ).toBeVisible({ timeout: 5000 });
    await expect(pathInput).toHaveValue(MOCK_DOWNLOADS_DIR);
  });

  test('Tab completes the name and lists the next level', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await typeAtEnd(pathInput, '/Doc');
    await pathInput.press('Tab');

    await expect(pathInput).toHaveValue(`${MOCK_HOME_DIR}/Documents/`);
    await expect(page.getByRole('option')).toHaveText(['Invoices/', 'Projects/']);

    await pathInput.pressSequentially('P');
    await expect(page.getByRole('option')).toHaveText(['Projects/']);

    await pathInput.press('Tab');
    await expect(pathInput).toHaveValue(`${MOCK_HOME_DIR}/Documents/Projects/`);
  });

  test('clicking a suggestion completes it and keeps the input focused', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await typeAtEnd(pathInput, '/D');

    await page.getByRole('option', { name: 'Documents/' }).click();

    await expect(pathInput).toHaveValue(`${MOCK_HOME_DIR}/Documents/`);
    await expect(pathInput).toBeFocused();
  });

  test('the list stays closed after Enter', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await typeAtEnd(pathInput, '/Downloads');
    await pathInput.press('Enter');
    await page.waitForTimeout(600);

    await expect(page.getByRole('listbox')).toHaveCount(0);
    await expect(pathInput).toHaveValue(MOCK_DOWNLOADS_DIR);
  });

  test('Escape closes the list and keeps the text; a second Escape restores the current path', async ({
    page,
  }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await typeAtEnd(pathInput, '/D');

    await pathInput.press('Escape');
    await expect(page.getByRole('listbox')).toHaveCount(0);
    await expect(pathInput).toHaveValue(`${MOCK_HOME_DIR}/D`);

    await pathInput.press('Escape');
    await expect(pathInput).toHaveValue(MOCK_HOME_DIR);
  });

  test('clicking away drops the draft', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.click();
    await pathInput.fill('/nowhere');
    await page.getByTestId('file-list').click();
    await pathInput.click();

    await expect(pathInput).toHaveValue(MOCK_HOME_DIR);
  });

  test('switching apps keeps the draft', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.click();
    await pathInput.fill('/nowhere');
    await pathInput.evaluate((el) =>
      el.dispatchEvent(new FocusEvent('focusout', { bubbles: true }))
    );

    await expect(pathInput).toHaveValue('/nowhere');
  });

  test('a folder is listed once, even when the user types on before a slow listing lands', async ({
    page,
  }) => {
    await recordSlowListings(page);

    const pathInput = page.locator('[data-testid="path-input"]');
    await typeAtEnd(pathInput, '/Documents/Pr', { delay: 20 });

    await expect(page.getByRole('option')).toHaveText(['Projects/']);
    const paths = await page.evaluate(
      () => (window as Window & { __READ_DIR_PATHS__?: string[] }).__READ_DIR_PATHS__ ?? []
    );
    expect(paths).toHaveLength(1);
    expect(paths[0]?.endsWith('/Documents/')).toBe(true);
  });

  test('Escape before a slow listing lands leaves no list under the restored path', async ({
    page,
  }) => {
    await recordSlowListings(page);

    const pathInput = page.locator('[data-testid="path-input"]');
    await typeAtEnd(pathInput, '/Documents/');
    await pathInput.press('Escape');
    await page.waitForTimeout(600);

    await expect(pathInput).toHaveValue(MOCK_HOME_DIR);
    await expect(page.getByRole('listbox')).toHaveCount(0);
  });

  test('hidden folders list only when the typed name starts with a dot', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await typeAtEnd(pathInput, '/Documents/');

    await expect(page.getByRole('option')).toHaveText(['Invoices/', 'Projects/']);

    await pathInput.pressSequentially('.');
    await expect(page.getByRole('option')).toHaveText(['.archive/']);
  });

  test('the shortcut focuses the bar and selects the whole path', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await page.getByTestId('file-list').click();
    await page.keyboard.press('ControlOrMeta+l');

    await expect(pathInput).toBeFocused();
    await page.keyboard.type('/x');
    await expect(pathInput).toHaveValue('/x');
  });

  test('the first click selects the whole path', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.click();
    await page.keyboard.type('/x');
    await expect(pathInput).toHaveValue('/x');
  });

  test('a second click places a caret', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.click();
    await pathInput.click();

    const collapsed = await pathInput.evaluate((el) => {
      const input = el as HTMLInputElement;
      return input.selectionStart === input.selectionEnd;
    });
    expect(collapsed).toBe(true);
  });

  test('a row the pointer only passed over does not decide what Enter opens', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await typeAtEnd(pathInput, '/Do');

    const first = page.getByRole('option').first();
    await first.hover();
    await expect(first).toHaveAttribute('aria-selected', 'true');
    await page.mouse.move(640, 600);
    await expect(page.getByRole('option', { selected: true })).toHaveCount(0);

    await pathInput.press('Enter');
    await expect(pathInput).toHaveValue(`${MOCK_HOME_DIR}/Do`);
  });

  test('Enter gives the focus to the files', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await typeAtEnd(pathInput, '/Downloads');
    await pathInput.press('Enter');

    await expect(pathInput).not.toBeFocused();
    await expect(pathInput).toHaveValue(MOCK_DOWNLOADS_DIR);
  });

  test('Down with a draft and no list keeps the focus and the draft', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.fill('/nowhere/x');
    await pathInput.press('ArrowDown');

    await expect(pathInput).toBeFocused();
    await expect(pathInput).toHaveValue('/nowhere/x');
  });

  test('Tab with nothing to complete keeps the focus and the draft', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.fill('/nowhere/x');
    await pathInput.press('Tab');

    await expect(pathInput).toBeFocused();
    await expect(pathInput).toHaveValue('/nowhere/x');
  });

  test('a path that starts with ~ gets suggestions from the home folder', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.fill('~/Documents/');
    await expect(page.getByRole('option')).toHaveText(['Invoices/', 'Projects/']);

    await pathInput.pressSequentially('P');
    await pathInput.press('Tab');
    await expect(pathInput).toHaveValue('~/Documents/Projects/');
  });

  test('~ opens the home folder', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.fill('~/Downloads');
    await pathInput.press('Enter');

    await expect(pathInput).toHaveValue(MOCK_DOWNLOADS_DIR);
    await expect(
      page.locator('[data-testid="file-item"][data-name="sample.pdf"]').first()
    ).toBeVisible({ timeout: 5000 });
  });
});
