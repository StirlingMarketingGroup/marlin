import fs from 'node:fs/promises';
import path from 'node:path';
import { afterAll, beforeAll, expect, test } from 'vitest';
import { fileRow, openFolder, rowNames, runCommand, startApp } from './app';

let app: Awaited<ReturnType<typeof startApp>>;

beforeAll(async () => {
  app = await startApp();
});

afterAll(async () => {
  await app?.stop();
});

test('a typed folder that does not exist stays in the path bar, and the list stays', async () => {
  const { browser, files } = app;
  await fs.writeFile(path.join(files, 'keep.txt'), 'keep\n');
  await openFolder(browser, files);
  await browser.waitUntil(async () => (await rowNames(browser)).join() === 'keep.txt', {
    timeout: 10_000,
    timeoutMsg: 'keep.txt never appeared',
  });

  const missing = path.join(files, 'missing');
  const pathInput = browser.$('[data-testid="path-input"]');
  await pathInput.click();
  await pathInput.setValue(missing);
  await browser.keys('Enter');

  const failure = browser.$('[data-testid="path-bar-failure"]');
  await failure.waitForDisplayed({ timeout: 10_000 });
  const text = await browser.execute(
    () => document.querySelector('[data-testid="path-bar-failure"]')?.textContent ?? ''
  );
  expect(text).toContain('does not exist');
  expect(await pathInput.getValue()).toBe(missing);
  expect(await browser.execute(() => document.activeElement?.getAttribute('data-testid'))).toBe(
    'path-input'
  );
  // The folder is read again when its listing was still arriving as the user left it.
  await browser.waitUntil(async () => (await rowNames(browser)).join() === 'keep.txt', {
    timeout: 10_000,
    timeoutMsg: 'keep.txt never came back',
  });
  await browser.waitUntil(
    async () => (await runCommand(browser, 'is_watching_directory', { path: files })) === true,
    { timeout: 10_000, timeoutMsg: 'the window never went back to the folder' }
  );
});

test('a typed file address opens its folder with the file selected, and a path under the file fails', async () => {
  const { browser, files } = app;
  const sub = path.join(files, 'sub');
  await fs.mkdir(sub);
  await fs.writeFile(path.join(sub, 'pick.txt'), 'pick\n');
  await fs.writeFile(path.join(sub, 'other.txt'), 'other\n');
  await openFolder(browser, files);

  const pathInput = browser.$('[data-testid="path-input"]');
  const fileAddress = path.join(sub, 'pick.txt');
  await pathInput.click();
  await pathInput.setValue(fileAddress);
  await browser.keys('Enter');

  await browser.waitUntil(async () => (await pathInput.getValue()) === sub, {
    timeout: 10_000,
    timeoutMsg: `the path bar never showed ${sub}`,
  });
  await browser.waitUntil(
    async () => (await runCommand(browser, 'is_watching_directory', { path: sub })) === true,
    { timeout: 10_000, timeoutMsg: 'the window never opened the folder' }
  );
  await browser.waitUntil(
    async () =>
      (await fileRow(browser, 'pick.txt').getAttribute('class'))?.includes('bg-accent-selected') ===
      true,
    { timeout: 10_000, timeoutMsg: 'pick.txt was never selected' }
  );

  const under = path.join(fileAddress, 'more');
  await pathInput.click();
  await pathInput.setValue(under);
  await browser.keys('Enter');

  const failure = browser.$('[data-testid="path-bar-failure"]');
  await failure.waitForDisplayed({ timeout: 10_000 });
  const text = await browser.execute(
    () => document.querySelector('[data-testid="path-bar-failure"]')?.textContent ?? ''
  );
  expect(text).toContain('does not exist');
  expect(await pathInput.getValue()).toBe(under);
});
