import { test, expect, type Locator, type Page } from '@playwright/test';
import {
  getTauriMockScript,
  MOCK_DOWN_HOST,
  MOCK_DOWNLOADS_DIR,
  MOCK_HOME_DIR,
  MOCK_MISSING_DIR,
} from './tauri-mocks';

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

  test('a typed file address opens its folder with the file selected', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.fill(`${MOCK_DOWNLOADS_DIR}/sample.pdf`);
    await pathInput.press('Enter');

    await expect(pathInput).toHaveValue(MOCK_DOWNLOADS_DIR);
    const selected = page.locator('[data-testid="file-item"].bg-accent-selected');
    await expect(selected).toHaveCount(1);
    await expect(selected).toHaveAttribute('data-name', 'sample.pdf');
    await expect(page.locator('[data-testid="path-bar-failure"]')).toHaveCount(0);

    // History holds no entry for the file, so Back returns to the folder before.
    await page.getByTitle(/^Back/).click();
    await expect(pathInput).toHaveValue(MOCK_HOME_DIR);
  });

  test.describe('an SFTP address with no user', () => {
    type SftpTestWindow = Window & {
      __SIGNED_IN__?: boolean;
      __TAURI_INVOKE_LOG__?: { cmd: string; args?: Record<string, unknown> }[];
      __TAURI_INTERNALS__: {
        invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
      };
    };
    const invokeLog = (page: Page) =>
      page.evaluate(() => (window as SftpTestWindow).__TAURI_INVOKE_LOG__ ?? []);

    // No server is saved for 10.5.0.247, so a read of it asks for a sign-in. Once the test sets
    // `__SIGNED_IN__`, a read lists one file as the saved user, `admin`.
    test.beforeEach(async ({ page }) => {
      await page.evaluate(() => {
        const w = window as SftpTestWindow;
        const original = w.__TAURI_INTERNALS__.invoke.bind(w.__TAURI_INTERNALS__);
        w.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
          const path = String(args?.path ?? '');
          if (cmd !== 'read_directory' || !path.startsWith('sftp://')) return original(cmd, args);
          const url = new URL(path);
          if (url.hostname !== '10.5.0.247') return original(cmd, args);
          const port = Number(url.port || 22);
          if (!w.__SIGNED_IN__) {
            throw `[SFTP_NO_CREDENTIALS] No credentials stored for server: 10.5.0.247:${port}`;
          }
          if (url.pathname.endsWith('/report.txt')) {
            throw `[ENOTDIR] Not a folder: ${path}`;
          }
          const folder = url.pathname || '/';
          const raw = `sftp://admin@10.5.0.247:${port}${folder}`;
          return {
            entries: [
              {
                name: 'report.txt',
                path: `${raw}/report.txt`,
                is_directory: false,
                is_hidden: false,
                is_symlink: false,
                is_git_repo: false,
                size: 128,
                modified: new Date().toISOString(),
                created: new Date().toISOString(),
                extension: 'txt',
                permissions: { read: true, write: true, execute: false },
              },
            ],
            location: {
              scheme: 'sftp',
              authority: `admin@10.5.0.247:${port}`,
              path: folder,
              displayPath: raw,
              raw,
            },
            capabilities: { canCreate: true, canDelete: true, canRename: true },
          };
        };
      });
    });

    test('opens the connect window with the host and port 22, and no dialog', async ({ page }) => {
      const pathInput = page.locator('[data-testid="path-input"]');
      await pathInput.fill('sftp://10.5.0.247');
      await pathInput.press('Enter');

      await expect
        .poll(
          async () =>
            (await invokeLog(page)).find(({ cmd }) => cmd === 'open_sftp_connect_window')?.args
        )
        .toEqual({
          initialHostname: '10.5.0.247',
          initialPort: 22,
          initialUsername: undefined,
          targetPath: 'sftp://10.5.0.247',
        });
      expect((await invokeLog(page)).some(({ cmd }) => cmd === 'plugin:dialog|message')).toBe(
        false
      );
      await expect(page.getByTestId('path-bar-failure')).toHaveCount(0);
      await expect(page.locator('[data-testid="file-item"][data-name="Documents"]')).toBeVisible();
    });

    test('opens the typed folder after the window connects', async ({ page }) => {
      const targetPath = 'sftp://10.5.0.247:2222/some/folder';
      const pathInput = page.locator('[data-testid="path-input"]');
      await pathInput.fill(targetPath);
      await pathInput.press('Enter');

      await expect
        .poll(
          async () =>
            (await invokeLog(page)).find(({ cmd }) => cmd === 'open_sftp_connect_window')?.args
        )
        .toEqual(expect.objectContaining({ initialPort: 2222, targetPath }));

      await page.evaluate(
        (payload) => {
          const w = window as SftpTestWindow;
          w.__SIGNED_IN__ = true;
          return w.__TAURI_INTERNALS__.invoke('plugin:event|emit', {
            event: 'sftp-connect:success',
            payload,
          });
        },
        { hostname: '10.5.0.247', port: 2222, username: 'admin', targetPath }
      );

      await expect(page.locator('[data-testid="file-item"][data-name="report.txt"]')).toBeVisible();
      await expect(pathInput).toHaveValue('sftp://admin@10.5.0.247:2222/some/folder');
    });

    test('a file address opens its folder as the listing names it, with the file selected', async ({
      page,
    }) => {
      await page.evaluate(() => {
        (window as SftpTestWindow).__SIGNED_IN__ = true;
      });
      const pathInput = page.locator('[data-testid="path-input"]');
      await pathInput.fill('sftp://10.5.0.247/some/folder/report.txt');
      await pathInput.press('Enter');

      await expect(pathInput).toHaveValue('sftp://admin@10.5.0.247:22/some/folder');
      const selected = page.locator('[data-testid="file-item"].bg-accent-selected');
      await expect(selected).toHaveCount(1);
      await expect(selected).toHaveAttribute('data-name', 'report.txt');
    });
  });
});

test.describe('Path bar: an address that cannot be opened', () => {
  test.beforeEach(async ({ page }) => {
    await page.addInitScript(getTauriMockScript());
    await page.goto('/');

    // The home folder's rows show once the app has opened its first folder.
    await expect(page.locator('[data-testid="file-item"][data-name="Documents"]')).toBeVisible({
      timeout: 15000,
    });
  });

  async function rowNames(page: Page) {
    return page
      .locator('[data-testid="file-item"]')
      .evaluateAll((rows) => rows.map((row) => row.getAttribute('data-name')));
  }

  for (const address of [
    MOCK_MISSING_DIR,
    `smb://${MOCK_DOWN_HOST}/share`,
    `sftp://me@${MOCK_DOWN_HOST}/home`,
    `gdrive://me@${MOCK_DOWN_HOST}/My Drive`,
  ]) {
    test(`${address} stays in the bar, and the list and history stay`, async ({ page }) => {
      const pathInput = page.locator('[data-testid="path-input"]');
      await pathInput.fill(MOCK_DOWNLOADS_DIR);
      await pathInput.press('Enter');
      await expect(page.locator('[data-testid="file-item"][data-name="sample.pdf"]')).toBeVisible();

      const names = await rowNames(page);
      await pathInput.click();
      await pathInput.fill(address);
      await pathInput.press('Enter');

      const failure = page.getByTestId('path-bar-failure');
      await expect(failure).toBeVisible();
      await expect(failure).toContainText(
        address === MOCK_MISSING_DIR
          ? 'Path does not exist'
          : `Could not connect to ${MOCK_DOWN_HOST}`
      );
      await expect(pathInput).toHaveValue(address);
      await expect(pathInput).toBeFocused();
      await expect(page.locator('[data-testid="file-item"][data-name="sample.pdf"]')).toBeVisible();
      await expect.poll(() => rowNames(page)).toEqual(names);
      await expect(page.getByTitle(/^Forward/)).toBeDisabled();
      await page.getByTitle(/^Back/).click();
      await expect(pathInput).toHaveValue(MOCK_HOME_DIR);
    });
  }

  test('fixing one wrong character and pressing Enter opens the folder', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.fill(MOCK_MISSING_DIR);
    await pathInput.press('Enter');
    await expect(page.getByTestId('path-bar-failure')).toBeVisible();

    await pathInput.press('Backspace');
    await pathInput.press('s');
    await expect(pathInput).toHaveValue(MOCK_DOWNLOADS_DIR);
    await expect(page.getByTestId('path-bar-failure')).toHaveCount(0);

    await pathInput.press('Enter');
    await expect(page.locator('[data-testid="file-item"][data-name="sample.pdf"]')).toBeVisible();
    await expect(pathInput).toHaveValue(MOCK_DOWNLOADS_DIR);
  });

  test("Escape shows the current folder's path again", async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.fill(MOCK_MISSING_DIR);
    await pathInput.press('Enter');
    await expect(page.getByTestId('path-bar-failure')).toBeVisible();

    await pathInput.press('Escape');
    await expect(pathInput).toHaveValue(MOCK_HOME_DIR);
    await expect(page.getByTestId('path-bar-failure')).toHaveCount(0);
  });

  test('clicking away drops a failed address', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.fill(MOCK_MISSING_DIR);
    await pathInput.press('Enter');
    await expect(page.getByTestId('path-bar-failure')).toBeVisible();

    await page.getByTestId('file-list').click();
    await expect(page.getByTestId('path-bar-failure')).toHaveCount(0);
    await pathInput.click();
    await expect(pathInput).toHaveValue(MOCK_HOME_DIR);
  });

  test('a pasted Google Drive link that cannot be opened stays in the bar', async ({ page }) => {
    const link = 'https://drive.google.com/drive/folders/1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS';
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.fill(link);
    await pathInput.press('Enter');

    await expect(page.getByTestId('path-bar-failure')).toContainText(
      'No Google Drive accounts are connected'
    );
    await expect(pathInput).toHaveValue(link);
    await expect(pathInput).toBeFocused();
  });

  test('the filter shortcut leaves a failed address for the filter box', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.fill(MOCK_MISSING_DIR);
    await pathInput.press('Enter');
    await expect(page.getByTestId('path-bar-failure')).toBeVisible();

    await page.keyboard.press('ControlOrMeta+f');
    const filterInput = page.getByTestId('filter-input');
    await expect(filterInput).toBeFocused();
    await page.keyboard.type('ments');

    await expect(filterInput).toHaveValue('ments');
    await expect(page.getByTestId('path-bar-failure')).toHaveCount(0);
    await expect(pathInput).toHaveValue(MOCK_HOME_DIR);
  });

  test('an open filter comes back, and the failed address keeps the focus', async ({ page }) => {
    await page.getByTestId('file-list').click();
    await page.keyboard.type('ments');
    const filterInput = page.getByTestId('filter-input');
    await expect(filterInput).toHaveValue('ments');

    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.click();
    await pathInput.fill(MOCK_MISSING_DIR);
    await pathInput.press('Enter');

    await expect(page.getByTestId('path-bar-failure')).toBeVisible();
    await expect(pathInput).toBeFocused();
    await expect(pathInput).toHaveValue(MOCK_MISSING_DIR);
    await expect(filterInput).toHaveValue('ments');
    await expect(page.locator('[data-testid="file-item"][data-name="Documents"]')).toBeVisible();
    await expect(page.locator('[data-testid="file-item"][data-name="Downloads"]')).toHaveCount(0);
  });

  test('a folder opened from the list that fails shows a dialog and keeps the list', async ({
    page,
  }) => {
    // Listing Documents fails, as it does for a folder deleted after the list was read.
    await page.evaluate((folder: string) => {
      const w = window as Window & {
        __TAURI_INTERNALS__: {
          invoke: (cmd: string, args?: { path?: string }) => Promise<unknown>;
        };
      };
      const original = w.__TAURI_INTERNALS__.invoke.bind(w.__TAURI_INTERNALS__);
      w.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
        if (cmd === 'read_directory_streaming_command' && args?.path === folder) {
          throw 'Path does not exist';
        }
        return original(cmd, args);
      };
    }, `${MOCK_HOME_DIR}/Documents`);
    const documents = page.locator('[data-testid="file-item"][data-name="Documents"]');
    await expect(documents).toBeVisible();
    const names = await rowNames(page);
    type LoggingWindow = Window & {
      __TAURI_INVOKE_LOG__?: {
        cmd: string;
        args?: { path?: string; title?: string; message?: string };
      }[];
    };
    const invokeLog = () =>
      page.evaluate(() => (window as LoggingWindow).__TAURI_INVOKE_LOG__ ?? []);
    const logBefore = (await invokeLog()).length;

    await documents.dblclick();

    // The failure shows the dialog of a navigation that was not typed.
    await expect
      .poll(
        async () =>
          (await invokeLog()).slice(logBefore).find(({ cmd }) => cmd === 'plugin:dialog|message')
            ?.args
      )
      .toEqual(
        expect.objectContaining({
          title: 'Directory Error',
          message: expect.stringMatching(/Documents[\s\S]*Path does not exist/),
        })
      );
    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toHaveValue(MOCK_HOME_DIR);
    await expect(page.getByTestId('path-bar-failure')).toHaveCount(0);
    await expect(documents).toBeVisible();
    await expect.poll(() => rowNames(page)).toEqual(names);
    // A new read of the home folder would start in the effects of the frame that restored it.
    // Two frames later it would be in the log. Its list was complete, so none starts.
    await page.evaluate(
      () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)))
    );
    const homeReads = (await invokeLog())
      .slice(logBefore)
      .filter(
        ({ cmd, args }) =>
          cmd === 'read_directory_streaming_command' && args?.path === MOCK_HOME_DIR
      );
    expect(homeReads).toHaveLength(0);
    await expect(page.getByTitle(/^Back/)).toBeDisabled();
    await expect(page.getByTitle(/^Forward/)).toBeDisabled();
  });

  // Holds each `read_directory` call for `address` until `release` runs, so a test can act while
  // the address is still loading. `loading` waits until one is held. Suggestion reads of its
  // server are not held.
  async function holdReads(page: Page, address: string) {
    type HoldingWindow = Window & {
      __HELD_READS__?: number;
      __RELEASE_READS__?: () => void;
      __TAURI_INTERNALS__: {
        invoke: (cmd: string, args?: { path?: string }) => Promise<unknown>;
      };
    };
    await page.evaluate((address: string) => {
      const w = window as HoldingWindow;
      const released = new Promise<void>((resolve) => {
        w.__RELEASE_READS__ = resolve;
      });
      w.__HELD_READS__ = 0;
      const original = w.__TAURI_INTERNALS__.invoke.bind(w.__TAURI_INTERNALS__);
      w.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
        if (cmd === 'read_directory' && args?.path === address) {
          w.__HELD_READS__! += 1;
          await released;
        }
        return original(cmd, args);
      };
    }, address);
    return {
      loading: () =>
        expect
          .poll(() => page.evaluate(() => (window as HoldingWindow).__HELD_READS__ ?? 0))
          .toBeGreaterThan(0),
      release: () => page.evaluate(() => (window as HoldingWindow).__RELEASE_READS__?.()),
    };
  }

  test('a draft typed while a slow address fails stays', async ({ page }) => {
    const address = `smb://${MOCK_DOWN_HOST}/share`;
    const reads = await holdReads(page, address);
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.fill(address);
    await pathInput.press('Enter');
    await reads.loading();
    await pathInput.click();
    await page.keyboard.type('/x');
    await reads.release();

    // History holds only the home folder again once the failure is handled.
    await expect(page.getByTitle(/^Back/)).toBeDisabled();
    await expect(page.locator('[data-testid="file-item"][data-name="Documents"]')).toBeVisible();
    await expect(pathInput).toHaveValue('/x');
    await expect(pathInput).toBeFocused();
    await expect(page.getByTestId('path-bar-failure')).toHaveCount(0);
  });

  test('a click into the bar while an address loads keeps its failure', async ({ page }) => {
    const address = `smb://${MOCK_DOWN_HOST}/share`;
    const reads = await holdReads(page, address);
    const pathInput = page.locator('[data-testid="path-input"]');
    await pathInput.fill(address);
    await pathInput.press('Enter');
    await reads.loading();
    // The first click selects the whole path.
    await pathInput.click();
    await reads.release();

    await expect(page.getByTestId('path-bar-failure')).toBeVisible();
    await expect(pathInput).toHaveValue(address);
    await expect(pathInput).toBeFocused();
    // The caret sits at the end, so the next key edits the address instead of replacing it.
    const selection = await pathInput.evaluate((el) => {
      const input = el as HTMLInputElement;
      return [input.selectionStart, input.selectionEnd];
    });
    expect(selection).toEqual([address.length, address.length]);
  });
});
