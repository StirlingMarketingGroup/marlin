import { spawn, type ChildProcess } from 'node:child_process';
import fs from 'node:fs/promises';
import net from 'node:net';
import path from 'node:path';
import { afterEach, expect } from 'vitest';
import { Key, remote, type Browser } from 'webdriverio';

const appBinary = path.join(import.meta.dirname, '../src-tauri/target/app-test/debug/marlin');

// The browser of the app this file started. Vitest loads this module once per test file.
let runningBrowser: Browser | undefined;

// The window of a failure is the evidence. This must not throw, or it would hide the failure.
async function saveFailureScreenshot(browser: Browser, testFile: string, what: string) {
  try {
    const name = `${path.basename(testFile, '.test.ts')}--${what.replace(/[^a-zA-Z0-9]+/g, '-')}`;
    const file = path.join(import.meta.dirname, '..', 'test-results', 'app', `${name}.png`);
    await fs.mkdir(path.dirname(file), { recursive: true });
    await browser.saveScreenshot(file);
    console.error(`Screenshot of the failure: ${file}`);
  } catch (error) {
    console.error(error);
  }
}

afterEach(async ({ task }) => {
  if (task.result?.state === 'fail' && runningBrowser !== undefined) {
    await saveFailureScreenshot(runningBrowser, task.file.filepath, task.name);
  }
});

/** A local TCP port that nothing listens on right now. */
export function freePort(): Promise<number> {
  return new Promise<number>((resolve, reject) => {
    const server = net
      .createServer()
      .once('error', reject)
      .listen(0, '127.0.0.1', () => {
        const { port } = server.address() as net.AddressInfo;
        server.close(() => resolve(port));
      });
  });
}

// A rejection in the page is a thrown WebDriver error. Return the value, or the error text, so the test asserts it.
export function runCommand(
  browser: Browser,
  command: string,
  args: Record<string, unknown> = {}
): Promise<unknown> {
  return browser.execute(
    `return window.__TAURI_INTERNALS__
      .invoke(arguments[0], arguments[1])
      .then((value) => value ?? null, (error) => String(error))`,
    command,
    args
  );
}

// The modifier of the app's shortcuts: Cmd on macOS, Ctrl elsewhere.
export const commandOrControl = process.platform === 'darwin' ? Key.Command : Key.Ctrl;

// Switches the file view with the app's shortcut, and waits until the view shows.
export async function showView(browser: Browser, view: 'grid' | 'list'): Promise<void> {
  await browser.keys([commandOrControl, view === 'grid' ? '1' : '2']);
  await browser.$(`[data-testid="file-${view}"]`).waitForExist({
    timeout: 10_000,
    timeoutMsg: `the ${view} view never appeared`,
  });
}

export function fileRow(browser: Browser, name: string) {
  return browser.$(`[data-file-item="true"][data-name="${name}"]`);
}

// The names of the rows the view has drawn: the view hides its rows while it loads, and those
// do not count. One script, not a request per row: rows come and go while the view scrolls,
// and on Linux a burst of parallel requests for about 30 rows crashes the test build (#655).
export function rowNames(browser: Browser): Promise<string[]> {
  return browser.execute(() =>
    Array.from(document.querySelectorAll('[data-file-item="true"]'))
      .filter((row) => getComputedStyle(row).visibility === 'visible')
      .map((row) => row.getAttribute('data-name')!)
      .sort()
  );
}

// Whether the window runs animation frames. A hidden window (a locked or covered Mac) runs
// none, so what the app does on a frame (the rename box's fill, a row's fade) never happens.
export function drawsFrames(browser: Browser): Promise<boolean> {
  return browser.execute(() => document.visibilityState === 'visible');
}

// Types a new name into an open rename box. On the frame after the box opens the app fills it
// with the current name, and on the next it focuses it (the grid view writes the name again).
// Either one replaces anything typed before it.
export async function typeNewName(
  browser: Browser,
  input: ReturnType<Browser['$']>,
  name: string
): Promise<void> {
  if (await drawsFrames(browser)) {
    await browser.waitUntil(
      async () => (await input.isFocused()) && (await input.getValue()) !== '',
      { timeout: 10_000, timeoutMsg: 'the rename box never filled with the current name' }
    );
  }
  await input.setValue(name);
}

// A change made before the watcher starts is never reported.
export async function openFolder(browser: Browser, folder: string): Promise<void> {
  const pathInput = browser.$('[data-testid="path-input"]');
  await pathInput.click();
  await pathInput.setValue(folder);
  await browser.keys('Enter');
  await browser.waitUntil(
    async () => (await runCommand(browser, 'is_watching_directory', { path: folder })) === true,
    { timeout: 10_000, timeoutMsg: `the app never started watching ${folder}` }
  );
}

// The title of a status bar segment: the Files count, or the Size, like "1.2 KiB (1,234 B)".
// The list view's column headers repeat the labels in spans too, but their parents have no
// title. Read as an attribute: on Linux, getText on the value span returns the whole segment.
export async function statusTitle(browser: Browser, label: 'Files' | 'Size'): Promise<string> {
  return (await browser.$(`//div[@title][span[text()="${label}"]]`).getAttribute('title')) ?? '';
}

// The exact byte count: the digits in the Size title's last parentheses.
export async function sizeBytes(browser: Browser): Promise<number> {
  const groups = (await statusTitle(browser, 'Size')).match(/\([^()]*\)/g);
  const last = groups?.[groups.length - 1] ?? '';
  return Number(last.replace(/\D/g, ''));
}

// Where the app keeps its preferences under the home folder `startApp` gives it.
export function preferencesPath(home: string): string {
  return path.join(
    home,
    ...(process.platform === 'darwin'
      ? ['Library', 'Application Support', 'Marlin']
      : ['.config', 'Marlin']),
    'preferences.json'
  );
}

/**
 * Starts the test build of the app with its own home folder and WebDriver port.
 * `files` is an empty folder for the files a test wants the app to show.
 * `home` is the app's temporary home folder.
 */
export async function startApp(): Promise<{
  browser: Browser;
  files: string;
  home: string;
  stop: () => Promise<void>;
}> {
  // Not os.tmpdir(): the mount helper's socket lives under the home folder, and macOS
  // limits a socket path to 104 bytes. The temp folder of a Mac is too long for that.
  const root = await fs.realpath(await fs.mkdtemp('/tmp/marlin-app-'));
  const home = path.join(root, 'home');
  const files = path.join(root, 'files');
  const run = path.join(root, 'run');
  const tmp = path.join(root, 'tmp');

  let app: ChildProcess | undefined;
  // Why the app process ended. Unset while it runs.
  let exit: string | undefined;
  let exited: Promise<void> | undefined;
  let browser: Browser | undefined;

  const stop = async () => {
    runningBrowser = undefined;
    try {
      await browser?.deleteSession();
    } finally {
      if (app && exit === undefined) {
        app.kill();
        const forceKill = setTimeout(() => app?.kill('SIGKILL'), 5_000);
        await exited;
        clearTimeout(forceKill);
      }
      await fs.rm(root, { recursive: true, force: true });
    }
  };

  try {
    await fs.mkdir(home);
    await fs.mkdir(files);
    await fs.mkdir(run, { mode: 0o700 });
    await fs.mkdir(tmp);

    const port = await freePort();

    // Everything the app may see. The developer's other variables stay out, so the app
    // cannot reach their settings, Google key file, or SSH agent. The `app-test` feature
    // keeps it out of the keychain.
    const env: NodeJS.ProcessEnv = {
      PATH: process.env.PATH,
      LANG: process.env.LANG,
      DISPLAY: process.env.DISPLAY,
      XAUTHORITY: process.env.XAUTHORITY,
      HOME: home,
      // macOS reads this one for the webview's data folder.
      CFFIXED_USER_HOME: home,
      // The app prunes its archive cache here when it starts.
      TMPDIR: tmp,
      // On Linux the mount helper's socket is here, and the app tells the helper on that
      // socket to stop when it quits. GTK also looks here for a Wayland display, so use X11.
      XDG_RUNTIME_DIR: run,
      GDK_BACKEND: 'x11',
      // On Linux, WebKit's helper processes write Mesa's shader cache under HOME as they
      // exit, after `stop` has removed the folder. With no cache, nothing is left behind.
      MESA_SHADER_CACHE_DISABLE: 'true',
      TAURI_WEBDRIVER_PORT: String(port),
    };

    app = spawn(appBinary, [], { stdio: ['ignore', 'inherit', 'inherit'], env });
    exited = new Promise((resolve) => {
      const ended = (why: string) => {
        exit = why;
        resolve();
      };
      app?.once('error', (error) => ended(error.message));
      app?.once('exit', (code, signal) => ended(`code ${code}, signal ${signal}`));
    });

    const deadline = Date.now() + 30_000;
    for (;;) {
      if (exit !== undefined) {
        throw new Error(`The test app ended before WebDriver was ready: ${exit}`);
      }
      const up = await fetch(`http://127.0.0.1:${port}/status`).then(
        (response) => response.ok,
        () => false
      );
      if (up) break;
      if (Date.now() > deadline) {
        throw new Error(`The test app did not answer on port ${port} within 30 seconds`);
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }

    browser = await remote({
      hostname: '127.0.0.1',
      port,
      logLevel: 'warn',
      capabilities: { 'wdio:enforceWebDriverClassic': true },
    });
    runningBrowser = browser;
    // A new session starts on whichever window the plugin lists first, and on macOS
    // the permissions window can be open beside the file browser.
    await browser.switchWindow('main');
    // The path bar is empty until the app opens its first folder. A path typed before
    // that is lost when the startup sets its own.
    const pathInput = browser.$('[data-testid="path-input"]');
    await browser.waitUntil(async () => (await pathInput.getValue()) !== '', {
      timeout: 30_000,
      timeoutMsg: 'The test app never opened its first folder',
    });

    return { browser, files, home, stop };
  } catch (error) {
    // A window that started but never got ready shows why.
    if (browser !== undefined) {
      await saveFailureScreenshot(browser, expect.getState().testPath ?? 'unknown', 'startup');
    }
    await stop().catch((cleanupError) => console.error(cleanupError));
    throw error;
  }
}
