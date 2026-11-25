import { test, expect, type Page } from '@playwright/test';
import { getTauriMockScript } from './tauri-mocks';

const savedServers = [
  {
    hostname: 'files.example',
    port: 2222,
    username: 'deploy',
    authMethod: 'key',
    keyPath: '~/.ssh/id_deploy',
  },
  { hostname: 'files.example', port: 22, username: 'ops', authMethod: 'agent', keyPath: null },
];

// The connection test fails with a passphrase request unless `connects` is set.
async function openConnectWindow(
  page: Page,
  payload: Record<string, unknown>,
  { connects = false, hold = false } = {}
) {
  // One script, so the saved servers wrap the base mock whatever order Playwright runs scripts in.
  await page.addInitScript(`
    ${getTauriMockScript()}
    (function() {
      const baseInvoke = window.__TAURI_INTERNALS__.invoke;
      window.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
        const answer = await baseInvoke(cmd, args);
        if (cmd === 'get_sftp_servers') return ${JSON.stringify(savedServers)};
        if (cmd === 'test_sftp_connection') {
          if (${hold}) {
            await new Promise((resolve) => {
              window.__RELEASE_SFTP_TEST__ = resolve;
            });
          }
          if (${connects}) return true;
          throw '[SFTP_NO_CREDENTIALS] The SSH key needs its passphrase.';
        }
        if (cmd === 'add_sftp_server') {
          const { hostname, port, username, authMethod, keyPath } = args;
          return { hostname, port, username, authMethod, keyPath };
        }
        return answer;
      };
    })();
  `);
  await page.goto('/?view=sftp-connect');

  // The window listens for its payload, then says it is ready.
  await expect
    .poll(() =>
      page.evaluate(() =>
        (
          window as unknown as { __TAURI_INVOKE_LOG__: { cmd: string }[] }
        ).__TAURI_INVOKE_LOG__.some((call) => call.cmd === 'sftp_connect_window_ready')
      )
    )
    .toBe(true);
  await page.evaluate((initPayload) => {
    const internals = (
      window as unknown as {
        __TAURI_INTERNALS__: { invoke: (cmd: string, args: unknown) => Promise<unknown> };
      }
    ).__TAURI_INTERNALS__;
    return internals.invoke('plugin:event|emit', {
      event: 'sftp-connect:init',
      payload: initPayload,
    });
  }, payload);
}

test.describe('SFTP connect window', () => {
  test('asks a saved key server for its key passphrase, not a password', async ({ page }) => {
    await openConnectWindow(page, {
      initialHostname: 'Files.Example',
      initialPort: 2222,
      requestId: 1,
    });

    await expect(page.locator('#sftp-auth-method')).toHaveValue('key');
    await expect(page.locator('#sftp-key-path')).toHaveValue('~/.ssh/id_deploy');
    await expect(page.locator('#sftp-username')).toHaveValue('deploy');
    await expect(page.locator('#sftp-key-passphrase')).toBeVisible();
    await expect(page.locator('#sftp-password')).toHaveCount(0);
  });

  test('matches the saved server by port too', async ({ page }) => {
    await openConnectWindow(page, {
      initialHostname: 'files.example',
      initialPort: 22,
      requestId: 1,
    });

    await expect(page.locator('#sftp-auth-method')).toHaveValue('agent');
    await expect(page.locator('#sftp-username')).toHaveValue('ops');
  });

  test('keeps the user name the app passed', async ({ page }) => {
    await openConnectWindow(page, {
      initialHostname: 'files.example',
      initialPort: 2222,
      initialUsername: 'other',
      requestId: 1,
    });

    await expect(page.locator('#sftp-auth-method')).toHaveValue('key');
    await expect(page.locator('#sftp-username')).toHaveValue('other');
  });

  test('never carries a passphrase over to the password field', async ({ page }) => {
    await openConnectWindow(page, {
      initialHostname: 'files.example',
      initialPort: 2222,
      requestId: 1,
    });
    await expect(page.locator('#sftp-auth-method')).toHaveValue('key');

    await page.locator('#sftp-key-passphrase').fill('key passphrase');
    await page.locator('#sftp-auth-method').selectOption('password');

    await expect(page.locator('#sftp-password')).toHaveValue('');
  });

  test('an address with no user asks for the user, then answers its request with the server and closes', async ({
    page,
  }) => {
    await openConnectWindow(
      page,
      { initialHostname: '10.5.0.247', initialPort: 2222, requestId: 1 },
      { connects: true }
    );

    await expect(page.locator('#sftp-hostname')).toHaveValue('10.5.0.247');
    await expect(page.locator('#sftp-hostname')).toBeDisabled();
    await expect(page.locator('#sftp-port')).toHaveValue('2222');
    await expect(page.locator('#sftp-port')).toBeDisabled();
    await expect(page.locator('#sftp-username')).toHaveValue('');
    await expect(page.locator('#sftp-username')).toBeFocused();

    await page.locator('#sftp-username').fill('admin');
    await page.locator('#sftp-password').fill('secret');
    await page.getByRole('button', { name: 'Connect', exact: true }).click();

    type LoggingWindow = Window & {
      __TAURI_INVOKE_LOG__: { cmd: string; args?: { event?: string; payload?: unknown } }[];
    };
    await expect
      .poll(() =>
        page.evaluate(() => {
          const log = (window as unknown as LoggingWindow).__TAURI_INVOKE_LOG__;
          return {
            connected: log.find(({ cmd }) => cmd === 'sftp_connect_window_connected')?.args,
            emitted: log.some(
              ({ cmd, args }) =>
                cmd === 'plugin:event|emit' && args?.event === 'sftp-connect:success'
            ),
            closed: log.some(({ cmd }) => cmd === 'plugin:window|close'),
          };
        })
      )
      .toEqual({
        connected: { requestId: 1, address: 'sftp://admin@10.5.0.247:2222/' },
        emitted: true,
        closed: true,
      });
  });

  test('a newer request that takes the window while Connect runs keeps the window open for it', async ({
    page,
  }) => {
    await openConnectWindow(
      page,
      { initialHostname: '10.5.0.247', initialPort: 2222, requestId: 1 },
      { connects: true, hold: true }
    );

    // The window moves the focus to the user field once it has the request.
    await expect(page.locator('#sftp-username')).toBeFocused();
    await page.locator('#sftp-username').fill('admin');
    await page.locator('#sftp-password').fill('secret');
    await page.getByRole('button', { name: 'Connect', exact: true }).click();

    type HeldWindow = Window & {
      __RELEASE_SFTP_TEST__?: () => void;
      __TAURI_INTERNALS__: { invoke: (cmd: string, args: unknown) => Promise<unknown> };
      __TAURI_INVOKE_LOG__: { cmd: string }[];
    };
    // The connection test is held only once its release function exists. The invoke log appears earlier.
    await expect
      .poll(() =>
        page.evaluate(
          () => typeof (window as unknown as HeldWindow).__RELEASE_SFTP_TEST__ === 'function'
        )
      )
      .toBe(true);

    await page.evaluate(
      (initPayload) => {
        const w = window as unknown as HeldWindow;
        void w.__TAURI_INTERNALS__.invoke('plugin:event|emit', {
          event: 'sftp-connect:init',
          payload: initPayload,
        });
        w.__RELEASE_SFTP_TEST__?.();
      },
      { requestId: 2, initialHostname: 'other.example', initialPort: 22 }
    );

    await expect
      .poll(() =>
        page.evaluate(() =>
          (window as unknown as HeldWindow).__TAURI_INVOKE_LOG__.some(
            ({ cmd }) => cmd === 'add_sftp_server'
          )
        )
      )
      .toBe(true);
    // Past the 450 ms a connected window waits before it closes.
    await page.waitForTimeout(600);

    const called = await page.evaluate(() =>
      (window as unknown as HeldWindow).__TAURI_INVOKE_LOG__.map(({ cmd }) => cmd)
    );
    expect(called).not.toContain('sftp_connect_window_connected');
    expect(called).not.toContain('plugin:window|close');
    await expect(page.locator('#sftp-hostname')).toHaveValue('other.example');
    await expect(page.getByText('Connected.', { exact: true })).toHaveCount(0);
  });

  test('shows a passphrase request without its tag', async ({ page }) => {
    await openConnectWindow(page, {
      initialHostname: 'files.example',
      initialPort: 2222,
      requestId: 1,
    });
    await expect(page.locator('#sftp-auth-method')).toHaveValue('key');

    await page.getByRole('button', { name: 'Connect', exact: true }).click();

    await expect(
      page.getByText('The SSH key needs its passphrase.', { exact: true })
    ).toBeVisible();
  });
});
