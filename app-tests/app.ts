import { spawn, type ChildProcess } from 'node:child_process';
import fs from 'node:fs/promises';
import net from 'node:net';
import path from 'node:path';
import { remote, type Browser } from 'webdriverio';

const appBinary = path.join(import.meta.dirname, '../src-tauri/target/app-test/debug/marlin');

/**
 * Starts the test build of the app with its own home folder and WebDriver port.
 * `files` is an empty folder for the files a test wants the app to show.
 */
export async function startApp(): Promise<{
  browser: Browser;
  files: string;
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

    const port = await new Promise<number>((resolve, reject) => {
      const server = net
        .createServer()
        .once('error', reject)
        .listen(0, '127.0.0.1', () => {
          const { port } = server.address() as net.AddressInfo;
          server.close(() => resolve(port));
        });
    });

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

    return { browser, files, stop };
  } catch (error) {
    await stop().catch((cleanupError) => console.error(cleanupError));
    throw error;
  }
}
