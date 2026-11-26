import { test, expect } from '@playwright/test';
import { getTauriMockScript } from './tauri-mocks';

test.describe('Pinned folders', () => {
  test.beforeEach(async ({ page }) => {
    await page.addInitScript(getTauriMockScript());
    // Added after the mock script, as in gdrive.spec.ts, so it wraps the mock's `invoke`.
    await page.addInitScript(() => {
      const internals = (
        window as unknown as {
          __TAURI_INTERNALS__: {
            invoke: (cmd: string, args?: { path?: string }) => Promise<unknown>;
          };
        }
      ).__TAURI_INTERNALS__;
      const original = internals.invoke.bind(internals);
      internals.invoke = async (cmd, args) => {
        if (cmd === 'get_pinned_directories') {
          const pinnedAt = '2026-01-02T03:04:05Z';
          return [
            {
              name: 'alice@files.example',
              path: 'sftp://alice@files.example',
              pinned_at: pinnedAt,
              is_git_repo: false,
              is_symlink: false,
            },
            {
              name: 'team',
              path: 'sftp://alice@files.example/team',
              pinned_at: pinnedAt,
              is_git_repo: false,
              is_symlink: false,
            },
          ];
        }
        const folder = args?.path ?? '';
        if (cmd === 'read_directory' && folder.startsWith('sftp://')) {
          const url = new URL(folder);
          // A non-special scheme's root has an empty pathname. The listing's address ends in `/`.
          const pathname = url.pathname || '/';
          const raw = `sftp://alice@files.example${pathname}`;
          return {
            entries: [],
            location: {
              scheme: 'sftp',
              authority: 'alice@files.example',
              path: pathname,
              displayPath: raw,
              raw,
            },
            capabilities: { canCreate: true, canDelete: true, canRename: true },
          };
        }
        return original(cmd, args);
      };
    });
    await page.goto('/');

    await expect(page.locator('[data-testid="path-input"]')).toBeVisible({ timeout: 15000 });
  });

  test('a pinned server root is marked as the open folder', async ({ page }) => {
    const pathInput = page.locator('[data-testid="path-input"]');
    const rootPin = page.getByRole('button', { name: 'alice@files.example', exact: true });
    const teamPin = page.getByRole('button', { name: 'team', exact: true });
    const rootName = rootPin.getByText('alice@files.example', { exact: true });
    const teamName = teamPin.getByText('team', { exact: true });

    await rootPin.click();
    await expect(pathInput).toHaveValue('sftp://alice@files.example/');
    await expect(rootName).toHaveClass(/text-accent/);
    await expect(teamName).not.toHaveClass(/text-accent/);

    await teamPin.click();
    await expect(pathInput).toHaveValue('sftp://alice@files.example/team');
    await expect(teamName).toHaveClass(/text-accent/);
    await expect(rootName).not.toHaveClass(/text-accent/);
  });
});
