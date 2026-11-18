import fs from 'node:fs/promises';
import path from 'node:path';
import { afterAll, beforeAll, expect, test } from 'vitest';
import { startApp } from './app';

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
