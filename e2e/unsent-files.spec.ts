import { test, expect, type Page } from '@playwright/test';
import { getTauriMockScript } from './tauri-mocks';
import { formatBytes } from '../src/utils/formatBytes';

const ACCOUNT = 'ana@example.com';
const MOUNT_POINT = `/Users/testuser/Marlin/gdrive-${ACCOUNT}`;
const NOT_IN_VIEW = 'Marlin cannot see this account’s unsent files right now.';

const MOUNT_COMMANDS = [
  'list_mounts',
  'get_mount_status',
  'mount_account',
  'unmount_account',
  'list_dead_mounts',
  'list_unsent_files',
  'retry_unsent_file',
  'discard_unsent_file',
  'locate_in_mount',
  'identify_in_mount',
  'refresh_mount',
];

type HelperFile = {
  node: number;
  name: string;
  size: number;
  path?: string;
  state: string;
  sent?: number;
  error?: string;
};

type HelperCall = { cmd: string; args: unknown };

type UnsentList = {
  provider: string;
  account: string;
  mountId: string;
  files: HelperFile[];
  unplaced: string[];
};

function mountStatus(state: string) {
  return {
    provider: 'gdrive',
    account: ACCOUNT,
    name: `gdrive-${ACCOUNT}`,
    mountPoint: MOUNT_POINT,
    state,
  };
}

function unsentList(mountId: string, files: HelperFile[], unplaced: string[] = []): UnsentList {
  return { provider: 'gdrive', account: ACCOUNT, mountId, files, unplaced };
}

function getFakeHelperScript(files: HelperFile[], unplaced: string[] = []): string {
  const unsent = JSON.stringify(unsentList('mount-1', files, unplaced));
  return `
    (function() {
      const base = window.__TAURI_INTERNALS__.invoke;
      const helper = {
        mounts: [${JSON.stringify(mountStatus('mounted'))}],
        unsent: ${unsent},
        // A command named in hold waits, once, until the test calls held[cmd]. The answer is what the helper held when the command came.
        hold: {},
        held: {},
        answered: {},
        errors: {},
        discardAnswer: 'Discard',
        calls: [],
        emit(event, payload) {
          const handlers = window.__TAURI_EVENT_LISTENERS__?.get(event) || [];
          for (const handler of handlers) {
            window.__TAURI_INTERNALS__.runCallback(handler, { event, payload });
          }
        },
      };
      window.__FAKE_HELPER__ = helper;
      window.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
        if (cmd === 'get_google_accounts') {
          return [{ email: '${ACCOUNT}', displayName: null, mountName: 'gdrive-${ACCOUNT}' }];
        }
        const mountCommand = cmd === 'list_mounts' || cmd === 'list_unsent_files' || cmd === 'retry_unsent_file' || cmd === 'discard_unsent_file';
        const discardQuestion = cmd === 'plugin:dialog|message' && typeof args?.title === 'string' && args.title.startsWith('Discard the changes');
        if (mountCommand || discardQuestion) helper.calls.push({ cmd, args });
        if (helper.errors[cmd]) {
          const error = helper.errors[cmd];
          delete helper.errors[cmd];
          throw error;
        }
        if (cmd === 'list_mounts' || cmd === 'list_unsent_files') {
          const answer = JSON.parse(JSON.stringify(cmd === 'list_mounts' ? helper.mounts : helper.unsent));
          if (helper.hold[cmd]) {
            delete helper.hold[cmd];
            await new Promise((resolve) => { helper.held[cmd] = resolve; });
            delete helper.held[cmd];
          }
          helper.answered[cmd] = (helper.answered[cmd] || 0) + 1;
          return answer;
        }
        if (cmd === 'retry_unsent_file' || cmd === 'discard_unsent_file') return null;
        if (discardQuestion) return helper.discardAnswer;
        return base(cmd, args);
      };
    })();
  `;
}

function helperCalls(page: Page, cmd: string): Promise<HelperCall[]> {
  return page.evaluate(
    (cmd) =>
      (
        window as unknown as { __FAKE_HELPER__: { calls: HelperCall[] } }
      ).__FAKE_HELPER__.calls.filter((call) => call.cmd === cmd),
    cmd
  );
}

function emit(page: Page, event: string, payload: unknown): Promise<void> {
  return page.evaluate(
    ({ event, payload }) => {
      (
        window as unknown as {
          __FAKE_HELPER__: { emit: (event: string, payload: unknown) => void };
        }
      ).__FAKE_HELPER__.emit(event, payload);
    },
    { event, payload }
  );
}

function setHelper(page: Page, patch: object): Promise<void> {
  return page.evaluate((patch) => {
    const helper = (window as unknown as { __FAKE_HELPER__: Record<string, unknown> })
      .__FAKE_HELPER__;
    Object.assign(helper, patch);
  }, patch);
}

type HeldHelper = {
  __FAKE_HELPER__: { held: Record<string, () => void>; answered: Record<string, number> };
};

// Answers a held command once the store has sent it, and returns once the store has its answer and has drawn it.
async function release(page: Page, cmd: string): Promise<void> {
  const helper = () =>
    page.evaluate((cmd) => {
      const { held, answered } = (window as unknown as HeldHelper).__FAKE_HELPER__;
      return { held: held[cmd] !== undefined, answered: answered[cmd] ?? 0 };
    }, cmd);
  await expect.poll(async () => (await helper()).held).toBe(true);
  const { answered } = await helper();
  await page.evaluate((cmd) => (window as unknown as HeldHelper).__FAKE_HELPER__.held[cmd](), cmd);
  await expect.poll(async () => (await helper()).answered).toBe(answered + 1);
  await page.evaluate(() => new Promise(requestAnimationFrame));
}

function invokeLog(page: Page): Promise<{ cmd: string; args?: unknown }[]> {
  return page.evaluate(
    () =>
      (window as unknown as { __TAURI_INVOKE_LOG__?: { cmd: string; args?: unknown }[] })
        .__TAURI_INVOKE_LOG__ ?? []
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

async function openApp(page: Page, files: HelperFile[], unplaced: string[] = []) {
  // One script, so the fake helper always wraps the mocks: Playwright does not order two init scripts.
  await page.addInitScript(getTauriMockScript() + getFakeHelperScript(files, unplaced));
  await page.goto('/');
}

const waiting = (name: string, node = 7, size = 200): HelperFile => ({
  node,
  name,
  size,
  state: 'waiting',
});

test.describe('Unsent files', () => {
  test('with no saved account, the window sends no mount command', async ({ page }) => {
    await page.addInitScript(getTauriMockScript());
    await page.goto('/');
    await expect
      .poll(async () => {
        const cmds = new Set((await invokeLog(page)).map((entry) => entry.cmd));
        return ['get_google_accounts', 'get_smb_servers', 'get_sftp_servers'].every((cmd) =>
          cmds.has(cmd)
        );
      })
      .toBe(true);
    await page.waitForTimeout(500);
    const sent = (await invokeLog(page)).filter((entry) => MOUNT_COMMANDS.includes(entry.cmd));
    expect(sent).toEqual([]);
  });

  test('a saved file shows as waiting, then uploading, then leaves the list', async ({ page }) => {
    await openApp(page, [waiting('report.txt')]);
    const mark = page.getByTestId('unsent-mark');
    await expect(mark).toContainText('1');
    await mark.click();
    const popover = page.getByTestId('unsent-files');
    await expect(popover).toContainText('report.txt');
    await expect(popover).toContainText('Waiting to upload');
    await page.keyboard.press('Escape');
    await expect(popover).toHaveCount(0);
    await mark.click();

    await emit(
      page,
      'unsent-changed',
      unsentList('mount-1', [{ ...waiting('report.txt'), state: 'uploading', sent: 100 }])
    );
    await expect(popover).toContainText('Uploading, 50%');

    await emit(page, 'unsent-changed', unsentList('mount-1', []));
    await expect(popover.getByTestId('unsent-file')).toHaveCount(0);
    await expect(popover).toContainText('No files are waiting to upload.');
    await expect(mark).toHaveCount(0);
  });

  test('a refused upload shows its error and folder, Retry sends it again, and Discard asks first', async ({
    page,
  }) => {
    const unplaced = [`/Users/testuser/.local/share/marlin/spool/gdrive/${ACCOUNT}/17`];
    await openApp(
      page,
      [
        {
          node: 7,
          name: 'report.txt',
          size: 200,
          path: 'My Drive/Projects/report.txt',
          state: 'refused',
          error: 'the account may not change this file',
        },
      ],
      unplaced
    );
    const mark = page.getByTestId('unsent-mark');
    await expect(mark).toHaveAttribute('title', 'Some files could not upload');
    await mark.click();
    const popover = page.getByTestId('unsent-files');
    await expect(popover).toContainText('the account may not change this file');
    await expect(popover).toContainText(unplaced[0]);
    // The folder opens in the window.
    await popover.getByRole('button', { name: 'My Drive/Projects' }).click();
    await expect(popover).toHaveCount(0);
    await expect
      .poll(async () =>
        (await invokeLog(page)).some(
          ({ args }) => isRecord(args) && args.path === `${MOUNT_POINT}/My Drive/Projects`
        )
      )
      .toBe(true);
    await mark.click();

    await popover.getByRole('button', { name: 'Retry' }).click();
    await expect
      .poll(async () => (await helperCalls(page, 'retry_unsent_file')).map((call) => call.args))
      .toEqual([{ provider: 'gdrive', account: ACCOUNT, mountId: 'mount-1', node: 7 }]);

    await setHelper(page, { discardAnswer: 'Cancel' });
    await popover.getByRole('button', { name: 'Discard' }).click();
    await expect
      .poll(async () => (await helperCalls(page, 'plugin:dialog|message')).length)
      .toBe(1);
    const question = (await helperCalls(page, 'plugin:dialog|message'))[0];
    expect(isRecord(question.args) && question.args.title).toEqual(
      expect.stringContaining('report.txt')
    );
    expect(isRecord(question.args) && question.args.message).toEqual(
      expect.stringContaining('My Drive/Projects')
    );
    expect(await helperCalls(page, 'discard_unsent_file')).toEqual([]);

    await setHelper(page, {
      discardAnswer: 'Discard',
      unsent: unsentList('mount-1', [], unplaced),
    });
    await popover.getByRole('button', { name: 'Discard' }).click();
    await expect
      .poll(async () => (await helperCalls(page, 'discard_unsent_file')).map((call) => call.args))
      .toEqual([{ provider: 'gdrive', account: ACCOUNT, mountId: 'mount-1', node: 7 }]);
    await expect(popover.getByTestId('unsent-file')).toHaveCount(0);
  });

  test('a discard the helper no longer matches (-1015) shows the helper’s list', async ({
    page,
  }) => {
    await openApp(page, [
      {
        node: 7,
        name: 'report.txt',
        size: 200,
        state: 'refused',
        error: 'the account may not change this file',
      },
    ]);
    const mark = page.getByTestId('unsent-mark');
    await expect(mark).toBeVisible();
    await setHelper(page, {
      errors: { discard_unsent_file: { code: -1015, message: 'no unsent bytes' } },
      unsent: unsentList('mount-2', [
        {
          node: 9,
          name: 'report.txt',
          size: 300,
          state: 'refused',
          error: 'the account may not change this file',
        },
      ]),
    });
    await mark.click();
    const popover = page.getByTestId('unsent-files');
    await popover.getByRole('button', { name: 'Discard' }).click();
    await expect(page.getByText('The list changed before Marlin could discard')).toBeVisible();
    await expect(popover.getByTestId('unsent-file')).toContainText(formatBytes(300));
    await setHelper(page, {
      errors: { discard_unsent_file: { code: -1016, message: 'the discard waited too long' } },
    });
    await popover.getByRole('button', { name: 'Discard' }).click();
    await expect
      .poll(async () => (await helperCalls(page, 'discard_unsent_file')).at(-1)?.args)
      .toEqual({
        provider: 'gdrive',
        account: ACCOUNT,
        mountId: 'mount-2',
        node: 9,
      });
    await expect(page.getByText('could not discard “report.txt” in time')).toBeVisible();
  });

  test('after the helper closes, no old list stays, and the window lists again', async ({
    page,
  }) => {
    await openApp(page, [waiting('report.txt')]);
    const mark = page.getByTestId('unsent-mark');
    await expect(mark).toBeVisible();
    await mark.click();
    const popover = page.getByTestId('unsent-files');
    await expect(popover).toContainText('report.txt');
    await setHelper(page, {
      unsent: unsentList('mount-2', [waiting('after.txt')]),
      hold: { list_mounts: true },
    });
    await emit(page, 'mount-helper-closed', null);
    await expect(mark).toHaveCount(0);
    await expect(popover).toContainText(NOT_IN_VIEW);
    await expect(popover.getByTestId('unsent-file')).toHaveCount(0);
    await release(page, 'list_mounts');
    await expect(popover).toContainText('after.txt');
    await expect(popover).not.toContainText('report.txt');
    await expect(mark).toBeVisible();
    expect(await helperCalls(page, 'list_mounts')).toHaveLength(2);
  });

  test('a list that fails after the helper closes is tried again', async ({ page }) => {
    await openApp(page, [waiting('report.txt')]);
    const mark = page.getByTestId('unsent-mark');
    await expect(mark).toBeVisible();
    await setHelper(page, {
      errors: { list_mounts: { message: 'Could not connect to the mount helper' } },
    });
    await emit(page, 'mount-helper-closed', null);
    // The mark goes as the helper closes, then comes back once a later list works.
    await expect(mark).toHaveCount(0);
    await expect(mark).toBeVisible({ timeout: 8000 });
    expect(await helperCalls(page, 'list_mounts')).toHaveLength(3);
  });

  test('an answer that lands after a newer notice does not bring back the old list', async ({
    page,
  }) => {
    await openApp(page, [waiting('old.txt')]);
    const mark = page.getByTestId('unsent-mark');
    await expect(mark).toBeVisible();
    // The store asks for the list again; that answer still holds old.txt, and lands after a newer notice.
    await setHelper(page, { hold: { list_unsent_files: true } });
    await emit(page, 'mount-changed', mountStatus('degraded'));
    await expect.poll(async () => (await helperCalls(page, 'list_unsent_files')).length).toBe(2);
    await emit(page, 'unsent-changed', unsentList('mount-1', []));
    await expect(mark).toHaveCount(0);
    await release(page, 'list_unsent_files');
    await expect(mark).toHaveCount(0);
  });

  test('a mount that stops loses its list at once, and one that comes up shows its own', async ({
    page,
  }) => {
    await openApp(page, [waiting('report.txt')]);
    const mark = page.getByTestId('unsent-mark');
    await expect(mark).toBeVisible();
    await emit(page, 'mount-changed', mountStatus('unmounted'));
    await expect(mark).toHaveCount(0);
    await setHelper(page, { unsent: unsentList('mount-2', [waiting('after.txt')]) });
    await emit(page, 'mount-changed', mountStatus('mounted'));
    await mark.click();
    await expect(page.getByTestId('unsent-files')).toContainText('after.txt');
  });

  test('two failed files of one name with no folder cannot be discarded', async ({ page }) => {
    const refused = (node: number): HelperFile => ({
      node,
      name: 'report.txt',
      size: 200,
      state: 'refused',
      error: 'the account may not change this file',
    });
    await openApp(page, [refused(7), refused(8)]);
    await page.getByTestId('unsent-mark').click();
    const popover = page.getByTestId('unsent-files');
    await expect(popover).toContainText('Another file here has this name');
    for (const button of await popover.getByRole('button', { name: 'Discard' }).all()) {
      await expect(button).toBeDisabled();
    }
    await expect(popover.getByRole('button', { name: 'Retry' }).first()).toBeEnabled();
  });

  test('the list stays inside a small window and scrolls', async ({ page }) => {
    await page.setViewportSize({ width: 560, height: 360 });
    const unplaced = Array.from({ length: 25 }, (_, index) => `/spool/gdrive/${ACCOUNT}/${index}`);
    await openApp(page, [waiting('report.txt')], unplaced);
    await page.getByTestId('unsent-mark').click();
    const box = await page.getByTestId('unsent-files').boundingBox();
    expect(box).not.toBeNull();
    expect(box!.x).toBeGreaterThanOrEqual(0);
    expect(box!.x + box!.width).toBeLessThanOrEqual(560);
    expect(box!.y + box!.height).toBeLessThanOrEqual(360);
    const last = page.getByRole('button', { name: 'Show' }).last();
    await last.scrollIntoViewIfNeeded();
    await expect(last).toBeInViewport();
  });

  test('a helper that closes before its list answers is listed again at a longer wait', async ({
    page,
  }) => {
    await page.clock.install();
    await openApp(page, [waiting('report.txt')]);
    const lists = async () => (await helperCalls(page, 'list_mounts')).length;
    await expect(page.getByTestId('unsent-mark')).toBeVisible();
    await page.clock.pauseAt(await page.evaluate(() => Date.now() + 100));
    // This helper stays up over a minute, so its close is listed again after 1 second.
    await page.clock.runFor(61_000);
    await setHelper(page, { hold: { list_mounts: true } });
    await emit(page, 'mount-helper-closed', null);
    await page.clock.runFor(999);
    expect(await lists()).toBe(1);
    await page.clock.runFor(2);
    await expect.poll(lists).toBe(2);
    // The next helper closes before its list answers, so the wait doubles.
    await emit(page, 'mount-helper-closed', null);
    await page.clock.runFor(1999);
    expect(await lists()).toBe(2);
    await page.clock.runFor(2);
    await expect.poll(lists).toBe(3);
  });
});
