import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { readFileSync } from 'node:fs';
import fs from 'node:fs/promises';
import net from 'node:net';
import path from 'node:path';
import { afterEach, expect } from 'vitest';
import { Key, remote, type Browser } from 'webdriverio';

const appBinary = path.join(import.meta.dirname, '../src-tauri/target/app-test/debug/marlin');
const fakeGoogleBinary = path.join(
  import.meta.dirname,
  '../src-tauri/target/app-test/debug/marlin-test-google'
);

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
      .then((value) => value ?? null, (error) => (typeof error === 'string' ? error : JSON.stringify(error)))`,
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

/** Starts the fake Google (`marlin-test-google`) for a test app to use in place of Google. It stops on `stop`, or when this process ends. */
export async function startFakeGoogle(): Promise<{ address: string; stop: () => Promise<void> }> {
  const fake = spawn(fakeGoogleBinary, [], { stdio: ['pipe', 'pipe', 'inherit'] });
  const exited = new Promise((resolve) => fake.once('exit', resolve));
  const address = await new Promise<string>((resolve, reject) => {
    let output = '';
    fake.stdout.on('data', (chunk: Buffer) => {
      output += chunk.toString();
      if (output.includes('\n')) resolve(output.split('\n')[0]);
    });
    fake.once('error', reject);
    fake.once('exit', (code, signal) =>
      reject(
        new Error(
          `The fake Google ended before it gave its address: code ${code}, signal ${signal}`
        )
      )
    );
  });
  // It stops when its stdin closes.
  const stop = async () => {
    fake.stdin.end();
    await exited;
  };
  return { address, stop };
}

// The helper's command line holds its socket, which is under `root`. `root` comes from mkdtemp,
// so it holds no regex character. pgrep exits with 1 when nothing matches.
function helperPids(root: string): string[] {
  const found = spawnSync('pgrep', ['-f', `marlin-mount --socket ${root}/`], { encoding: 'utf8' });
  if (found.status !== 0 && found.status !== 1) {
    throw new Error(`pgrep failed (${found.status ?? found.error}): ${found.stderr}`);
  }
  return found.stdout.split('\n').filter((line) => line !== '');
}

// The mount points under `root`. Linux lists them in mountinfo, as field 5 with a space
// written `\040`. macOS lists them in the output of `mount`. A list that fails throws: read as
// empty, it would let the caller remove a folder that holds a mount.
function mountPoints(root: string): string[] {
  let points: string[];
  if (process.platform === 'linux') {
    points = readFileSync('/proc/self/mountinfo', 'utf8')
      .split('\n')
      .map((line) =>
        (line.split(' ')[4] ?? '').replace(/\\([0-7]{3})/g, (_, octal: string) =>
          String.fromCharCode(parseInt(octal, 8))
        )
      );
  } else {
    const found = spawnSync('mount', { encoding: 'utf8' });
    if (found.status !== 0) {
      throw new Error(`mount failed (${found.status ?? found.error}): ${found.stderr}`);
    }
    points = found.stdout.split('\n').map((line) => / on (\/.*) \([^()]*\)$/.exec(line)?.[1] ?? '');
  }
  return points.filter((point) => point.startsWith(`${root}/`));
}

// The harness kills the app, so the app never tells its mount helper to stop. This stops it and
// clears what it left mounted. It throws when anything is left, before the caller removes `root`:
// removing a folder that holds a live mount would delete the remote's files through it.
async function stopMountHelper(root: string): Promise<void> {
  // On SIGTERM the helper unmounts what it can and exits. One still there after 10 seconds gets
  // SIGKILL, which leaves a dead mount.
  for (const [signal, limitMs] of [
    ['SIGTERM', 10_000],
    ['SIGKILL', 2_000],
  ] as const) {
    for (const pid of helperPids(root)) {
      try {
        process.kill(Number(pid), signal);
      } catch {
        // It exited after pgrep saw it.
      }
    }
    const deadline = Date.now() + limitMs;
    while (helperPids(root).length > 0 && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  }
  // The commands the helper uses to clear a dead mount.
  for (const point of mountPoints(root)) {
    if (process.platform === 'linux') {
      spawnSync('fusermount3', ['-u', '-z', point]);
    } else {
      spawnSync('umount', ['-f', point]);
    }
  }
  const helpers = helperPids(root);
  const mounts = mountPoints(root);
  if (helpers.length > 0 || mounts.length > 0) {
    throw new Error(
      `The test app's mount helper (${helpers.join(', ')}) or mounts (${mounts.join(', ')}) outlived it, so ${root} is left in place`
    );
  }
}

/**
 * Starts the test build of the app with its own home folder and WebDriver port.
 * `files` is an empty folder for the files a test wants the app to show.
 * `home` is the app's temporary home folder.
 * `google`, when set, is the fake Google the app and its mount helper use.
 */
export async function startApp(options: { google?: string } = {}): Promise<{
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
      await stopMountHelper(root);
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
    // keeps secrets in the test keychain below, never the OS keychain.
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
      // The app and the mount helper it starts share this test keychain.
      MARLIN_TEST_KEYCHAIN: path.join(root, 'keychain.json'),
      ...(options.google !== undefined ? { MARLIN_TEST_GOOGLE: options.google } : {}),
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
