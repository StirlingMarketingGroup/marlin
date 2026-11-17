import fs from 'node:fs/promises';
import path from 'node:path';
import { afterAll, beforeAll, expect, test } from 'vitest';
import { startApp } from './app';

// macOS can hand the app its file events minutes late when the machine is busy (#577).
const eventWait = { timeout: 120_000 };

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
  { timeout: 300_000 },
  async () => {
    const { browser, files } = app;
    const row = (name: string) => browser.$(`[data-file-item="true"][data-name="${name}"]`);

    await fs.writeFile(path.join(files, 'first.txt'), 'first\n');

    const pathInput = browser.$('[data-testid="path-input"]');
    await pathInput.click();
    await pathInput.setValue(files);
    await browser.keys('Enter');
    await row('first.txt').waitForExist({ timeout: 10_000 });

    // A write that lands before the watcher starts is never reported.
    await browser.waitUntil(
      // This string runs in the page.
      () =>
        browser.execute(
          `return window.__TAURI_INTERNALS__.invoke('is_watching_directory', { path: arguments[0] })`,
          files
        ),
      { timeout: 10_000, timeoutMsg: 'the app never started watching the folder' }
    );

    await fs.writeFile(path.join(files, 'report.pdf.crdownload'), 'report\n');
    await row('report.pdf.crdownload').waitForExist(eventWait);

    await fs.rename(path.join(files, 'report.pdf.crdownload'), path.join(files, 'report.pdf'));
    await row('report.pdf').waitForExist(eventWait);

    const names = await browser
      .$$('[data-file-item="true"]')
      .map((item) => item.getAttribute('data-name'));
    expect(names.sort()).toEqual(['first.txt', 'report.pdf']);
  }
);
