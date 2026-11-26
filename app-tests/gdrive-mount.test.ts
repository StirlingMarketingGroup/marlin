import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import type { Browser } from 'webdriverio';
import { fileRow, helperPids, openFolder, runCommand, startApp, startFakeGoogle } from './app';

// A mount needs macFUSE on a Mac (the helper loads this libfuse), and /dev/fuse with fusermount3 on Linux. Without them, as on a machine with no FUSE, the test is skipped. CI's runners have FUSE, so there it never skips.
const canMount =
  process.platform === 'darwin'
    ? fs.existsSync('/usr/local/lib/libfuse3.4.dylib')
    : fs.existsSync('/dev/fuse') && spawnSync('fusermount3', ['--version']).status === 0;

describe.skipIf(!canMount && !process.env.CI)('a Google account in a mount', () => {
  let app: Awaited<ReturnType<typeof startApp>>;
  let google: Awaited<ReturnType<typeof startFakeGoogle>>;

  beforeAll(async () => {
    google = await startFakeGoogle();
    app = await startApp({ google: google.address });
  });

  afterAll(async () => {
    try {
      await app?.stop();
    } finally {
      await google?.stop();
    }
  });

  it('adds an account against a fake Google, mounts it, and lists a file from it', async () => {
    const { browser } = app;
    // The app has no saved account, so no helper runs.
    expect(await runCommand(browser, 'get_google_accounts')).toEqual([]);
    expect(helperPids(path.dirname(app.home))).toEqual([]);
    // The fake Google signs in at once, as if the user picked the account in the browser.
    await browser.$('//button[span[text()="Add Google Account..."]]').click();
    await browser.$('//button[span[text()="app-test@example.com"]]').waitForExist({
      timeout: 30_000,
      timeoutMsg: 'the account app-test@example.com never appeared in the sidebar',
    });

    const accounts = (await runCommand(browser, 'get_google_accounts')) as {
      email: string;
      mountName: string;
    }[];
    expect(accounts).toHaveLength(1);
    const { email, mountName } = accounts[0];

    // No control in the window mounts an account (#687), so the test sends the mount command itself.
    const status = (await runCommand(browser, 'mount_account', {
      provider: 'gdrive',
      account: email,
      name: mountName,
    })) as { state: string; mountPoint: string };
    expect(status).toMatchObject({ state: 'mounted' });

    await openFolder(browser, path.join(status.mountPoint, 'My Drive'));
    await fileRow(browser, 'big.bin').waitForExist({
      timeout: 30_000,
      timeoutMsg: 'big.bin from the fake Google never appeared',
    });

    // A file written into the mount shows in the sidebar's list as being written, then waiting, then uploading, then leaves it.
    const waitForText = async (element: ReturnType<Browser['$']>, text: string) => {
      await browser.waitUntil(async () => (await element.getText()).includes(text), {
        timeout: 15_000,
        timeoutMsg: `the unsent list never showed "${text}"`,
      });
    };
    const list = browser.$('[data-testid="unsent-files"]');
    const saved = await fs.promises.open(
      path.join(status.mountPoint, 'My Drive', 'saved.txt'),
      'w'
    );
    try {
      await saved.write('saved from the test');
      const mark = browser.$('[data-testid="unsent-mark"]');
      await mark.waitForExist({
        timeout: 10_000,
        timeoutMsg: 'the sidebar never marked the unsent file',
      });
      await mark.click();
      await waitForText(list, 'Being written');
    } finally {
      await saved.close();
    }
    await waitForText(list, 'Waiting to upload');
    await waitForText(list, 'Uploading');
    // The list must say it is empty: a lost helper would also take saved.txt off it.
    await browser.waitUntil(
      async () => (await list.getText()).includes('No files are waiting to upload.'),
      { timeout: 30_000, timeoutMsg: 'saved.txt never left the unsent list' }
    );
    expect(await list.getText()).not.toContain('saved.txt');
    expect(await browser.$('[data-testid="unsent-mark"]').isExisting()).toBe(false);
  });
});
