import fs from 'node:fs/promises';
import path from 'node:path';
import { afterAll, beforeAll, expect, test } from 'vitest';
import { fileRow, openFolder, rowNames, startApp } from './app';

let app: Awaited<ReturnType<typeof startApp>>;

beforeAll(async () => {
  app = await startApp();
});

afterAll(async () => {
  await app?.stop();
});

// The #194 case: a browser downloads to a temporary name and renames the file when it is done.
test(
  'a file written under a temporary name and then renamed shows its final name without a refresh',
  { timeout: 60_000 },
  async () => {
    const { browser, files } = app;

    await fs.writeFile(path.join(files, 'first.txt'), 'first\n');
    await openFolder(browser, files);
    await fileRow(browser, 'first.txt').waitForExist({
      timeout: 10_000,
      timeoutMsg: 'first.txt never appeared',
    });

    await fs.writeFile(path.join(files, 'report.pdf.crdownload'), 'report\n');
    await fileRow(browser, 'report.pdf.crdownload').waitForExist({
      timeout: 10_000,
      timeoutMsg: 'the temporary download never appeared',
    });

    await fs.rename(path.join(files, 'report.pdf.crdownload'), path.join(files, 'report.pdf'));
    await fileRow(browser, 'report.pdf').waitForExist({
      timeout: 10_000,
      timeoutMsg: 'the renamed download never appeared',
    });

    expect(await rowNames(browser)).toEqual(['first.txt', 'report.pdf']);
  }
);
