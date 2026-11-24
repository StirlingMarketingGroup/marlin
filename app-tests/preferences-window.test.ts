import fs from 'node:fs/promises';
import path from 'node:path';
import { afterAll, beforeAll, expect, test } from 'vitest';
import { Key } from 'webdriverio';
import { commandOrControl, startApp } from './app';

let app: Awaited<ReturnType<typeof startApp>>;

beforeAll(async () => {
  app = await startApp();
});

afterAll(async () => {
  await app?.stop();
});

// The WebDriver server lists windows by their Tauri label.
test('a theme picked in Preferences changes the file browser and is saved', async () => {
  const { browser, home } = app;
  const current = await browser.execute(() => document.documentElement.dataset.theme ?? '');
  // The background is the theme's appDark color, from src/themes/dracula.ts and github.ts. It
  // tells the picked theme from the default one of the same scheme.
  const picked =
    current === 'light'
      ? { name: 'Dracula', id: 'dracula', scheme: 'dark', background: '#282a36' }
      : { name: 'GitHub Light', id: 'github-light', scheme: 'light', background: '#f6f8fa' };

  await browser.keys([commandOrControl, ',']);
  await browser.waitUntil(async () => (await browser.getWindowHandles()).includes('preferences'), {
    timeout: 10_000,
    timeoutMsg: 'the Preferences window never opened',
  });
  await browser.switchWindow('preferences');
  const themeButton = browser.$(`//button[.//div[text()="${picked.name}"]]`);
  await themeButton.waitForExist({
    timeout: 10_000,
    timeoutMsg: `${picked.name} never appeared in Preferences`,
  });
  await themeButton.click();

  await browser.switchWindow('main');
  await browser.waitUntil(
    async () => {
      const shown = await browser.execute(() => ({
        scheme: document.documentElement.dataset.theme,
        background: document.documentElement.style.getPropertyValue('--color-app-dark'),
      }));
      return shown.scheme === picked.scheme && shown.background === picked.background;
    },
    { timeout: 10_000, timeoutMsg: 'the file browser never took the theme picked in Preferences' }
  );

  const prefsPath = path.join(
    home,
    ...(process.platform === 'darwin'
      ? ['Library', 'Application Support', 'Marlin']
      : ['.config', 'Marlin']),
    'preferences.json'
  );
  const saved = JSON.parse(await fs.readFile(prefsPath, 'utf8')) as {
    globalPreferences?: { theme?: string; darkThemeId?: string; lightThemeId?: string };
  };
  expect(saved.globalPreferences?.theme).toBe(picked.scheme);
  expect(
    picked.scheme === 'dark'
      ? saved.globalPreferences?.darkThemeId
      : saved.globalPreferences?.lightThemeId
  ).toBe(picked.id);

  // Escape is the one way to close it from the page on every platform (macOS has no Close
  // button in the page). The window closes before the key is released, so the release finds
  // no window.
  await browser.switchWindow('preferences');
  await browser.keys(Key.Escape).catch((error: unknown) => {
    if (!String(error).includes('No window could be found')) throw error;
  });
  await browser.waitUntil(async () => !(await browser.getWindowHandles()).includes('preferences'), {
    timeout: 10_000,
    timeoutMsg: 'Preferences never closed',
  });
  await browser.switchWindow('main');
});
