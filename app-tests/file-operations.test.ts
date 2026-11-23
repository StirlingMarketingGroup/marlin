import { randomUUID } from 'node:crypto';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { isDeepStrictEqual } from 'node:util';
import { afterAll, beforeAll, expect, test } from 'vitest';
import { Key } from 'webdriverio';
import { commandOrControl, fileRow, openFolder, runCommand, startApp } from './app';

let app: Awaited<ReturnType<typeof startApp>>;

beforeAll(async () => {
  app = await startApp();
});

afterAll(async () => {
  await app?.stop();
});

test('a new folder gets the name typed into its rename box', async () => {
  const { browser, files } = app;
  const dir = path.join(files, 'new-folder');
  await fs.mkdir(dir);
  await openFolder(browser, dir);

  await browser.keys([commandOrControl, Key.Shift, 'n']);
  const input = browser.$('[data-file-item="true"] input');
  await input.waitForExist({
    timeout: 10_000,
    timeoutMsg: 'the new folder never opened its rename box',
  });
  await input.setValue('Reports');
  await browser.keys(Key.Enter);

  await fileRow(browser, 'Reports').waitForExist({
    timeout: 10_000,
    timeoutMsg: 'the Reports folder never appeared',
  });
  expect(await fs.readdir(dir)).toEqual(['Reports']);
  expect((await fs.stat(path.join(dir, 'Reports'))).isDirectory()).toBe(true);
});

test('a renamed file can be renamed back with undo', async () => {
  const { browser, files } = app;
  const dir = path.join(files, 'rename');
  await fs.mkdir(dir);
  await fs.writeFile(path.join(dir, 'draft.txt'), 'draft\n');
  await openFolder(browser, dir);

  await fileRow(browser, 'draft.txt').click();
  await browser.keys(Key.F2);
  const input = fileRow(browser, 'draft.txt').$('input');
  await input.waitForExist({ timeout: 10_000, timeoutMsg: 'the rename box never opened' });
  await input.setValue('final.txt');
  await browser.keys(Key.Enter);

  await fileRow(browser, 'final.txt').waitForExist({
    timeout: 10_000,
    timeoutMsg: 'final.txt never appeared',
  });
  await fileRow(browser, 'draft.txt').waitForExist({
    timeout: 10_000,
    reverse: true,
    timeoutMsg: 'draft.txt stayed in the list',
  });
  expect(await fs.readdir(dir)).toEqual(['final.txt']);
  expect(await fs.readFile(path.join(dir, 'final.txt'), 'utf8')).toBe('draft\n');

  await browser.keys([commandOrControl, 'z']);
  await fileRow(browser, 'draft.txt').waitForExist({
    timeout: 10_000,
    timeoutMsg: 'draft.txt never came back',
  });
  await fileRow(browser, 'final.txt').waitForExist({
    timeout: 10_000,
    reverse: true,
    timeoutMsg: 'final.txt stayed in the list',
  });
  expect(await fs.readdir(dir)).toEqual(['draft.txt']);
  expect(await fs.readFile(path.join(dir, 'draft.txt'), 'utf8')).toBe('draft\n');
});

test('a file moved to the trash comes back with undo', async () => {
  const name = `trash-test-${randomUUID()}.txt`;
  try {
    const { browser, files } = app;
    const dir = path.join(files, 'trash');
    await fs.mkdir(dir);
    await fs.writeFile(path.join(dir, name), 'trash\n');
    await fs.writeFile(path.join(dir, 'keep.txt'), 'keep\n');
    await openFolder(browser, dir);

    await fileRow(browser, name).click();
    if (process.platform === 'darwin') await browser.keys([Key.Command, Key.Backspace]);
    else await browser.keys(Key.Delete);

    await fileRow(browser, name).waitForExist({
      timeout: 10_000,
      reverse: true,
      timeoutMsg: 'the trashed file stayed in the list',
    });
    expect(await fs.readdir(dir)).toEqual(['keep.txt']);

    await browser.keys([commandOrControl, 'z']);
    await fileRow(browser, name).waitForExist({
      timeout: 10_000,
      timeoutMsg: 'the trashed file never came back',
    });
    expect((await fs.readdir(dir)).sort()).toEqual(['keep.txt', name].sort());
    expect(await fs.readFile(path.join(dir, name), 'utf8')).toBe('trash\n');
  } finally {
    if (process.platform === 'darwin') {
      // macOS puts the app's trash in the developer's real Trash, not under the temporary
      // home, so a failed undo would leave the file there.
      await fs.rm(path.join(os.homedir(), '.Trash', name), { force: true });
    }
  }
});

// This uses the real system clipboard, so it replaces whatever the developer copied.
test('a copied file pastes into another folder', async () => {
  const { browser, files } = app;
  const from = path.join(files, 'copy-from');
  const to = path.join(files, 'copy-to');
  const notes = path.join(from, 'notes.txt');
  await fs.mkdir(from);
  await fs.mkdir(to);
  await fs.writeFile(notes, 'notes\n');

  await openFolder(browser, from);
  await fileRow(browser, 'notes.txt').click();
  await browser.keys([commandOrControl, 'c']);
  // The copy runs after the key returns, and paste reads the live system clipboard. Pasting
  // before the copy lands would paste what the developer copied, or move a file they cut.
  await browser.waitUntil(
    async () =>
      isDeepStrictEqual(await runCommand(browser, 'clipboard_get_contents'), {
        hasFiles: true,
        hasImage: false,
        filePaths: [notes],
        isCut: false,
      }),
    { timeout: 10_000, timeoutMsg: 'the copy never reached the clipboard' }
  );

  await openFolder(browser, to);
  await browser.keys([commandOrControl, 'v']);
  await fileRow(browser, 'notes.txt').waitForExist({
    timeout: 10_000,
    timeoutMsg: 'the pasted file never appeared',
  });
  expect(await fs.readFile(path.join(to, 'notes.txt'), 'utf8')).toBe('notes\n');
  expect(await fs.readFile(notes, 'utf8')).toBe('notes\n');
});
