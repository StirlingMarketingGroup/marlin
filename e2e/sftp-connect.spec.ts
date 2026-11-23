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

async function openConnectWindow(page: Page, payload: Record<string, unknown>) {
  // One script, so the saved servers wrap the base mock whatever order Playwright runs scripts in.
  await page.addInitScript(`
    ${getTauriMockScript()}
    (function() {
      const baseInvoke = window.__TAURI_INTERNALS__.invoke;
      window.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
        const answer = await baseInvoke(cmd, args);
        if (cmd === 'get_sftp_servers') return ${JSON.stringify(savedServers)};
        if (cmd === 'test_sftp_connection') {
          throw '[SFTP_NO_CREDENTIALS] The SSH key needs its passphrase.';
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
      targetPath: 'sftp://files.example:2222/srv',
    });

    await expect(page.locator('#sftp-auth-method')).toHaveValue('key');
    await expect(page.locator('#sftp-key-path')).toHaveValue('~/.ssh/id_deploy');
    await expect(page.locator('#sftp-username')).toHaveValue('deploy');
    await expect(page.locator('#sftp-key-passphrase')).toBeVisible();
    await expect(page.locator('#sftp-password')).toHaveCount(0);
  });

  test('matches the saved server by port too', async ({ page }) => {
    await openConnectWindow(page, { initialHostname: 'files.example', initialPort: 22 });

    await expect(page.locator('#sftp-auth-method')).toHaveValue('agent');
    await expect(page.locator('#sftp-username')).toHaveValue('ops');
  });

  test('keeps the user name the app passed', async ({ page }) => {
    await openConnectWindow(page, {
      initialHostname: 'files.example',
      initialPort: 2222,
      initialUsername: 'other',
    });

    await expect(page.locator('#sftp-auth-method')).toHaveValue('key');
    await expect(page.locator('#sftp-username')).toHaveValue('other');
  });

  test('never carries a passphrase over to the password field', async ({ page }) => {
    await openConnectWindow(page, { initialHostname: 'files.example', initialPort: 2222 });
    await expect(page.locator('#sftp-auth-method')).toHaveValue('key');

    await page.locator('#sftp-key-passphrase').fill('key passphrase');
    await page.locator('#sftp-auth-method').selectOption('password');

    await expect(page.locator('#sftp-password')).toHaveValue('');
  });

  test('shows a passphrase request without its tag', async ({ page }) => {
    await openConnectWindow(page, { initialHostname: 'files.example', initialPort: 2222 });
    await expect(page.locator('#sftp-auth-method')).toHaveValue('key');

    await page.getByRole('button', { name: 'Connect', exact: true }).click();

    await expect(
      page.getByText('The SSH key needs its passphrase.', { exact: true })
    ).toBeVisible();
  });
});
