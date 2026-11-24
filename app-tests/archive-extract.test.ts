import fs from 'node:fs/promises';
import path from 'node:path';
import { isDeepStrictEqual } from 'node:util';
import { afterAll, beforeAll, expect, test } from 'vitest';
import { runCommand, startApp } from './app';

// `volumes.part1.rar` and `volumes.part2.rar` are one archive in two volumes.
const fixtures = path.join(import.meta.dirname, '../src-tauri/tests/fixtures/rar');

let app: Awaited<ReturnType<typeof startApp>>;

beforeAll(async () => {
  app = await startApp();
});

afterAll(async () => {
  await app?.stop();
});

// "Extract Here" extracts into the folder that holds the archive, so a failed extraction
// must not clean that folder up.
test('a refused "Extract Here" leaves the folder as it was', async () => {
  const { browser, files } = app;
  const laterVolume = path.join(files, 'volumes.part2.rar');
  await fs.copyFile(
    path.join(fixtures, 'volumes.part1.rar'),
    path.join(files, 'volumes.part1.rar')
  );
  await fs.copyFile(path.join(fixtures, 'volumes.part2.rar'), laterVolume);
  await fs.writeFile(path.join(files, 'keep.txt'), 'keep\n');

  const outcome = await browser.execute(
    // This string runs in the page.
    `return window.__TAURI_INTERNALS__
      .invoke('extract_archive', {
        archivePath: arguments[0],
        destinationDir: arguments[1],
        createSubfolder: false,
      })
      .then(() => 'extracted', (error) => String(error))`,
    laterVolume,
    files
  );

  expect(outcome).toContain('Multi-volume RAR archive');
  expect((await fs.readdir(files)).sort()).toEqual([
    'keep.txt',
    'volumes.part1.rar',
    'volumes.part2.rar',
  ]);
});

// A zip of one folder holds what is inside the folder, so the entry is `config/.env`.
test('a copy from inside an archive keeps its name, and a cut is refused', async () => {
  const { browser, files } = app;
  const docs = path.join(files, 'docs');
  await fs.mkdir(path.join(docs, 'config'), { recursive: true });
  await fs.writeFile(path.join(docs, 'config', '.env'), 'SECRET=1\n');

  const compressed = (await runCommand(browser, 'compress_to_zip', {
    sourcePaths: [docs],
    destinationDir: files,
    suggestedName: 'docs.zip',
  })) as { path: string };
  const address = `archive:///?src=${encodeURIComponent(compressed.path)}&path=${encodeURIComponent('/config/.env')}`;

  const copied = (await runCommand(browser, 'download_and_copy_to_clipboard', {
    paths: [address],
    isCut: false,
  })) as string[];
  expect(copied).toHaveLength(1);
  expect(path.basename(copied[0])).toBe('.env');
  await browser.waitUntil(
    async () =>
      isDeepStrictEqual(await runCommand(browser, 'clipboard_get_contents'), {
        hasFiles: true,
        hasImage: false,
        filePaths: copied,
        isCut: false,
      }),
    { timeout: 10_000, timeoutMsg: 'the copy never reached the clipboard' }
  );
  expect(await fs.readFile(copied[0], 'utf8')).toBe('SECRET=1\n');

  const refused = await runCommand(browser, 'download_and_copy_to_clipboard', {
    paths: [address],
    isCut: true,
  });
  expect(refused).toBe('Files inside an archive cannot be cut. Copy them instead.');
  expect(await runCommand(browser, 'clipboard_get_contents')).toEqual({
    hasFiles: true,
    hasImage: false,
    filePaths: copied,
    isCut: false,
  });
});
