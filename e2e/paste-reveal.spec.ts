import { test, expect, type Page } from '@playwright/test';
import { getTauriMockScript, MOCK_BIG_DIR, MOCK_HOME_DIR } from './tauri-mocks';

declare global {
  interface Window {
    __MOCK_CLIPBOARD__?: {
      hasFiles: boolean;
      hasImage: boolean;
      filePaths: string[];
      isCut: boolean;
    };
    __TAURI_EVENT_LISTENERS__?: Map<string, number[]>;
    __TAURI_INVOKE_LOG__: { cmd: string; args: { paths?: string[] } }[];
    __TAURI_INTERNALS__: {
      invoke: (cmd: string, args?: unknown) => Promise<unknown>;
      runCallback: (handler: number, data: unknown) => void;
    };
  }
}

// Opens a folder of 600 subfolders, so a pasted file lands far below the window. Enter in the
// path bar moves the focus to the files, so the paste key reaches the app.
async function openBigFolder(page: Page, view: 'list' | 'grid' = 'list') {
  await page.addInitScript(getTauriMockScript(view));
  await page.goto('/');
  const pathInput = page.locator('[data-testid="path-input"]');
  await expect(pathInput).toBeVisible({ timeout: 15000 });
  await pathInput.click();
  await pathInput.fill(MOCK_BIG_DIR);
  await pathInput.press('Enter');
  await expect(page.getByTestId(`file-${view}`)).toBeVisible();
  await expect(page.locator('[data-testid="file-item"][data-name="dir-000"]')).toBeVisible();
}

function copyFiles(page: Page, filePaths: string[]) {
  return page.evaluate((paths) => {
    window.__MOCK_CLIPBOARD__ = { hasFiles: true, hasImage: false, filePaths: paths, isCut: false };
  }, filePaths);
}

// Drops on the list's column header: inside the file panel, but not on a folder row.
async function dropIntoOpenFolder(page: Page, paths: string[]) {
  const header = page.locator('[data-testid="file-list"] button', { hasText: 'Name' });
  const box = await header.boundingBox();
  expect(box).not.toBeNull();
  await page.evaluate(
    ({ paths, x, clientY }) => {
      const payload = {
        paths,
        location: { x, y: window.innerHeight - clientY, targetId: 'file-panel' },
        eventType: 'drop',
        modifiers: { optionAlt: false, cmdCtrl: false },
      };
      for (const handler of window.__TAURI_EVENT_LISTENERS__?.get('drag-drop-event') ?? []) {
        window.__TAURI_INTERNALS__.runCallback(handler, { event: 'drag-drop-event', payload });
      }
    },
    { paths, x: box!.x + box!.width / 2, clientY: box!.y + box!.height / 2 }
  );
}

const item = (page: Page, name: string) =>
  page.locator(`[data-testid="file-item"][data-name="${name}"]`);

test.describe('Pasting reveals the new files', () => {
  for (const view of ['list', 'grid'] as const) {
    test(`a pasted file in a long folder is selected and in view, in ${view} view`, async ({
      page,
    }) => {
      await openBigFolder(page, view);
      await copyFiles(page, [`${MOCK_HOME_DIR}/Documents/notes.txt`]);

      await page.keyboard.press('ControlOrMeta+v');

      await expect(item(page, 'notes.txt')).toHaveClass(/bg-accent-selected/);
      await expect(item(page, 'notes.txt')).toBeInViewport();
      await expect(page.locator('[data-testid="file-item"].bg-accent-selected')).toHaveCount(1);
    });
  }

  test('a paste of several items selects them all and shows the first in the view', async ({
    page,
  }) => {
    await openBigFolder(page);
    // The view lists the folder dir-300a in the middle and zeta.txt last. The clipboard lists
    // zeta.txt first.
    await copyFiles(page, [
      `${MOCK_HOME_DIR}/Documents/zeta.txt`,
      `${MOCK_HOME_DIR}/Documents/dir-300a`,
    ]);

    await page.keyboard.press('ControlOrMeta+v');

    await expect(item(page, 'dir-300a')).toHaveClass(/bg-accent-selected/);
    await expect(item(page, 'dir-300a')).toBeInViewport();
    // The list draws only the rows near the window, so scroll to the end to see zeta.txt.
    await page.locator('[data-main-content]').evaluate((scroller) => {
      scroller.scrollTop = scroller.scrollHeight;
    });
    await expect(item(page, 'zeta.txt')).toHaveClass(/bg-accent-selected/);
  });

  test('a file dropped into the open folder is selected and in view', async ({ page }) => {
    await openBigFolder(page);

    await dropIntoOpenFolder(page, ['/private/var/folders/mock/dropped.txt']);

    await expect(item(page, 'dropped.txt')).toHaveClass(/bg-accent-selected/);
    await expect(item(page, 'dropped.txt')).toBeInViewport();
  });

  test('a drop where one item fails still shows and undoes the item that arrived', async ({
    page,
  }) => {
    await openBigFolder(page);
    // For an error it keeps per item, such as a name it cannot read, the backend goes on with the
    // other items and reports what arrived with the error. Here the second item fails.
    await page.evaluate(() => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      window.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
        if (cmd !== 'paste_items_to_location') return invoke(cmd, args);
        const paste = args as { sourcePaths: string[] };
        const result = await invoke(cmd, { ...paste, sourcePaths: paste.sourcePaths.slice(0, 1) });
        return {
          ...(result as object),
          skippedCount: 1,
          errorMessage: 'Unable to determine file name',
        };
      };
    });

    await dropIntoOpenFolder(page, [
      '/private/var/folders/mock/dropped.txt',
      '/private/var/folders/mock/unreadable.txt',
    ]);

    await expect(item(page, 'dropped.txt')).toHaveClass(/bg-accent-selected/);
    await expect(item(page, 'dropped.txt')).toBeInViewport();
    await expect(page.getByText('Unable to determine file name')).toBeVisible();

    // Undo of a copy moves what it pasted to the Trash: only the item that arrived.
    await page.getByRole('button', { name: 'Undo' }).click();
    await expect
      .poll(() =>
        page.evaluate(() =>
          window.__TAURI_INVOKE_LOG__
            .filter((call) => call.cmd === 'trash_paths')
            .map((call) => call.args.paths)
        )
      )
      .toEqual([[`${MOCK_BIG_DIR}/dropped.txt`]]);
  });
});
