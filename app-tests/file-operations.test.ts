import { randomUUID } from 'node:crypto';
import fs from 'node:fs/promises';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { isDeepStrictEqual } from 'node:util';
import { afterAll, beforeAll, expect, test } from 'vitest';
import { Key } from 'webdriverio';
import {
  commandOrControl,
  drawsFrames,
  fileRow,
  openFolder,
  runCommand,
  showView,
  startApp,
  typeNewName,
} from './app';

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
  await typeNewName(browser, input, 'Reports');
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
  await typeNewName(browser, input, 'final.txt');
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

// The listing changes while the box is open, as it does in a folder a browser downloads into.
// Each view has its own rename box.
for (const view of ['list', 'grid'] as const) {
  test(`${view}: a name typed into the rename box survives a change to the folder`, async () => {
    const { browser, files } = app;
    const dir = path.join(files, `rename-while-changing-${view}`);
    await fs.mkdir(dir);
    await fs.writeFile(path.join(dir, 'draft.txt'), 'draft\n');
    await openFolder(browser, dir);
    await showView(browser, view);

    await fileRow(browser, 'draft.txt').click();
    await browser.keys(Key.F2);
    const input = fileRow(browser, 'draft.txt').$('input');
    await input.waitForExist({ timeout: 10_000, timeoutMsg: 'the rename box never opened' });
    await typeNewName(browser, input, 'kept.txt');

    await fs.writeFile(path.join(dir, 'other.txt'), 'other\n');
    await fileRow(browser, 'other.txt').waitForExist({
      timeout: 10_000,
      timeoutMsg: 'other.txt never appeared',
    });
    expect(await input.getValue()).toBe('kept.txt');
    await browser.keys(Key.Enter);

    await fileRow(browser, 'kept.txt').waitForExist({
      timeout: 10_000,
      timeoutMsg: 'kept.txt never appeared',
    });
    expect((await fs.readdir(dir)).sort()).toEqual(['kept.txt', 'other.txt']);
  });
}

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
      // Logged, not thrown: an error here would hide the test's own failure.
      await fs
        .rm(path.join(os.homedir(), '.Trash', name), { force: true })
        .catch((error: unknown) => console.error(error));
    }
  }
});

// On macOS the test build copies to a pasteboard of its own. On Linux it uses the display's
// clipboard, which under CI and test-in-docker.sh is Xvfb's.
test('a copied file pastes into a long folder', async () => {
  const { browser, files } = app;
  const from = path.join(files, 'copy-from');
  const to = path.join(files, 'copy-to');
  const notes = path.join(from, 'notes.txt');
  await fs.mkdir(from);
  await fs.mkdir(to);
  await fs.writeFile(notes, 'notes\n');
  // notes.txt sorts after these, far below the window.
  await Promise.all(
    Array.from({ length: 200 }, (_, index) =>
      fs.writeFile(path.join(to, `a-${String(index).padStart(3, '0')}.txt`), '')
    )
  );

  await openFolder(browser, from);
  await fileRow(browser, 'notes.txt').click();
  await browser.keys([commandOrControl, 'c']);
  // The copy runs after the key returns. Paste reads the clipboard, so it waits for the copy.
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
  // The rows of the folder it left show for a moment. a-000.txt exists only here.
  await fileRow(browser, 'a-000.txt').waitForExist({
    timeout: 10_000,
    timeoutMsg: 'the destination never showed',
  });
  await browser.keys([commandOrControl, 'v']);
  const pasted = path.join(to, 'notes.txt');
  await browser.waitUntil(
    async () =>
      fs.access(pasted).then(
        () => true,
        () => false
      ),
    {
      timeout: 10_000,
      timeoutMsg: 'the paste never wrote the file',
    }
  );
  expect(await fs.readFile(pasted, 'utf8')).toBe('notes\n');
  expect(await fs.readFile(notes, 'utf8')).toBe('notes\n');
  // A hidden window runs no animation frames, and the view scrolls on a frame.
  if (await drawsFrames(browser)) {
    const row = fileRow(browser, 'notes.txt');
    await browser.waitUntil(
      async () =>
        (await row.isExisting()) &&
        (await row.isDisplayed({ withinViewport: true })) &&
        ((await row.getAttribute('class')) ?? '').includes('bg-accent-selected'),
      { timeout: 10_000, timeoutMsg: 'the pasted file never scrolled into view' }
    );
  }
});

// Copies `name` from `from` and pastes it into `to`, which holds an item of the same name, picking Replace in the conflict window.
async function pasteWithReplace(from: string, to: string, name: string): Promise<void> {
  const { browser } = app;
  await openFolder(browser, from);
  await fileRow(browser, name).click();
  await browser.keys([commandOrControl, 'c']);
  await browser.waitUntil(
    async () =>
      isDeepStrictEqual(await runCommand(browser, 'clipboard_get_contents'), {
        hasFiles: true,
        hasImage: false,
        filePaths: [path.join(from, name)],
        isCut: false,
      }),
    { timeout: 10_000, timeoutMsg: 'the copy never reached the clipboard' }
  );

  await openFolder(browser, to);
  await fileRow(browser, name).waitForExist({
    timeout: 10_000,
    timeoutMsg: 'the destination never showed',
  });
  await browser.keys([commandOrControl, 'v']);
  await browser.waitUntil(
    async () => (await browser.getWindowHandles()).includes('conflict-dialog'),
    { timeout: 10_000, timeoutMsg: 'the conflict window never opened' }
  );
  await browser.switchWindow('conflict-dialog');
  const replaceOption = browser.$('input[name="conflict-action"][value="replace"]');
  await replaceOption.waitForExist({
    timeout: 10_000,
    timeoutMsg: 'the replace choice never showed',
  });
  await replaceOption.click();
  await browser.$('//button[text()="Continue"]').click();
  await browser.switchWindow('main');
}

test('a pasted file replaces the file of the same name when Replace is picked', async () => {
  const from = path.join(app.files, 'replace-from');
  const to = path.join(app.files, 'replace-to');
  await fs.mkdir(from);
  await fs.mkdir(to);
  await fs.writeFile(path.join(from, 'notes.txt'), 'new\n');
  await fs.writeFile(path.join(to, 'notes.txt'), 'old\n');

  await pasteWithReplace(from, to, 'notes.txt');

  await app.browser.waitUntil(
    async () => (await fs.readFile(path.join(to, 'notes.txt'), 'utf8')) === 'new\n',
    { timeout: 10_000, timeoutMsg: 'the paste never replaced the file' }
  );
  expect(await fs.readdir(to)).toEqual(['notes.txt']);
  expect(await fs.readFile(path.join(from, 'notes.txt'), 'utf8')).toBe('new\n');
});

test('a Replace whose copy fails keeps the file it would replace', async () => {
  const from = path.join(app.files, 'replace-fail-from');
  const to = path.join(app.files, 'replace-fail-to');
  await fs.mkdir(from);
  await fs.mkdir(to);
  await fs.writeFile(path.join(to, 'data'), 'old\n');
  // A socket is not a file, a folder, or a link, so its copy always fails, for root too.
  const socket = net.createServer();
  await new Promise<void>((resolve, reject) => {
    socket.once('error', reject);
    socket.listen(path.join(from, 'data'), resolve);
  });
  try {
    await pasteWithReplace(from, to, 'data');

    await app.browser.waitUntil(
      async () =>
        (await app.browser.execute(() => document.body.innerText)).includes('could not be pasted'),
      { timeout: 10_000, timeoutMsg: 'the failed paste never said so' }
    );
    expect(await fs.readFile(path.join(to, 'data'), 'utf8')).toBe('old\n');
    expect(await fs.readdir(to)).toEqual(['data']);
  } finally {
    socket.close();
  }
});
