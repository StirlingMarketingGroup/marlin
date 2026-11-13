import { test, expect, type Page } from '@playwright/test';
import { getTauriMockScript, MOCK_LONG_NAME, MOCK_LONG_NAME_DIR } from './tauri-mocks';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(getTauriMockScript());
});

const longTile = (page: Page) =>
  page.locator(`[data-testid="file-item"][data-name="${MOCK_LONG_NAME}"]`);

async function nameOfTileBelowLongTile(page: Page): Promise<string> {
  const name = await longTile(page).evaluate((tile) => {
    const tileRect = tile.getBoundingClientRect();
    const below = Array.from(
      document.querySelectorAll<HTMLElement>('[data-file-item="true"]')
    ).find((el) => {
      const rect = el.getBoundingClientRect();
      return rect.top > tileRect.bottom && Math.abs(rect.left - tileRect.left) < 1;
    });
    return below?.dataset.name;
  });
  if (!name) throw new Error('no tile below the long-name tile');
  return name;
}

// The name is click-through, so a plain hit test skips it. This makes it a hit
// target for one call and reports what is painted on top at a point inside the
// tile one row down.
async function longNameOverTile(page: Page, belowName: string) {
  return page.evaluate(
    ({ longName, belowName }) => {
      const tileNamed = (name: string) =>
        Array.from(document.querySelectorAll<HTMLElement>('[data-file-item="true"]')).find(
          (el) => el.dataset.name === name
        );
      const name = tileNamed(longName)?.querySelector<HTMLElement>('.text-sm');
      const below = tileNamed(belowName);
      if (!name || !below) return { reachesTileBelow: false, paintedOnTop: false };

      const nameRect = name.getBoundingClientRect();
      const x = nameRect.left + nameRect.width / 2;
      const y = below.getBoundingClientRect().top + 12;

      name.style.pointerEvents = 'auto';
      const top = document.elementFromPoint(x, y);
      name.style.pointerEvents = 'none';
      return {
        reachesTileBelow: nameRect.top + name.scrollHeight > y,
        paintedOnTop: top !== null && name.contains(top),
      };
    },
    { longName: MOCK_LONG_NAME, belowName }
  );
}

test.describe('Selected file name in the grid', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');

    const pathInput = page.locator('[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 15000 });
    await pathInput.click();
    await pathInput.fill(MOCK_LONG_NAME_DIR);
    await pathInput.press('Enter');

    await page.keyboard.press('ControlOrMeta+1');
    await expect(page.locator('[data-testid="file-grid"]')).toBeVisible({ timeout: 5000 });
    await expect(longTile(page)).toBeVisible({ timeout: 5000 });
  });

  test('a selected long name shows over the tile below it', async ({ page }) => {
    const belowName = await nameOfTileBelowLongTile(page);

    await longTile(page).click();

    await expect
      .poll(() => longNameOverTile(page, belowName))
      .toEqual({ reachesTileBelow: true, paintedOnTop: true });
  });

  test('a selected long name shows over a selected tile below it', async ({ page }) => {
    const belowName = await nameOfTileBelowLongTile(page);

    await longTile(page).click();
    await page
      .locator(`[data-testid="file-item"][data-name="${belowName}"]`)
      .click({ modifiers: ['ControlOrMeta'] });
    await expect(page.locator('[data-testid="file-item"].bg-accent-selected')).toHaveCount(2);

    await expect
      .poll(() => longNameOverTile(page, belowName))
      .toEqual({ reachesTileBelow: true, paintedOnTop: true });
  });
});
