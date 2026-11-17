import { test, expect, type Page } from '@playwright/test';
import {
  getTauriMockScript,
  getTestUtilsScript,
  MOCK_CHANGING_DIR,
  MOCK_DOWNLOADS_DIR,
} from './tauri-mocks';

declare global {
  interface Window {
    __MOCK_CHANGING_NAMES__?: string[];
    __MAX_SIDEWAYS_OVERFLOW__: number;
  }
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(getTestUtilsScript());
});

const sidewaysOverflow = (page: Page) =>
  page
    .locator('[data-main-content]')
    .evaluate((scroller) => scroller.scrollWidth - scroller.clientWidth);

const canScrollDown = (page: Page) =>
  page
    .locator('[data-main-content]')
    .evaluate((scroller) => scroller.scrollHeight > scroller.clientHeight);

const columnCount = (page: Page) =>
  page
    .locator('.file-grid')
    .first()
    .evaluate((row) => getComputedStyle(row).gridTemplateColumns.split(' ').length);

test('a grid that mounted with no files still follows its width', async ({ page }) => {
  const names = Array.from({ length: 40 }, (_, i) => `file-${String(i).padStart(2, '0')}.txt`);

  await page.setViewportSize({ width: 1280, height: 720 });
  await page.addInitScript(getTauriMockScript('grid'));
  await page.addInitScript((seeded) => {
    window.__MOCK_CHANGING_NAMES__ = seeded;
  }, names);
  await page.goto('/?path=' + encodeURIComponent(MOCK_CHANGING_DIR));

  await expect(page.locator('[data-testid="path-input"]')).toBeVisible({ timeout: 15000 });
  await expect(page.locator('[data-testid="file-item"]').first()).toBeVisible();

  const columnsBefore = await columnCount(page);
  await page.setViewportSize({ width: 900, height: 720 });

  await expect.poll(() => columnCount(page)).toBeLessThan(columnsBefore);
  const tilesPastTheEdge = await page.evaluate(() => {
    const right = document.querySelector('[data-main-content]')!.getBoundingClientRect().right;
    return Array.from(
      document.querySelector('.file-grid')!.querySelectorAll('[data-testid="file-item"]')
    ).filter((tile) => tile.getBoundingClientRect().right > right).length;
  });
  expect(tilesPastTheEdge).toBe(0);
});

test('a narrow list has no sideways overflow', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.addInitScript(getTauriMockScript());
  await page.goto('/');

  const pathInput = page.locator('[data-testid="path-input"]');
  await expect(pathInput).toBeVisible({ timeout: 15000 });
  await pathInput.click();
  await pathInput.fill(MOCK_DOWNLOADS_DIR);
  await pathInput.press('Enter');
  await expect(page.locator('[data-testid="file-list"]')).toBeVisible();
  await expect(page.locator('[data-testid="file-item"]').first()).toBeVisible();

  // At 700 the date is wider than its column, so the cell must cut it.
  await page.setViewportSize({ width: 700, height: 720 });
  await expect
    .poll(() =>
      page
        .locator('[data-testid="file-item"]')
        .evaluateAll((rows) => rows.filter((row) => row.scrollWidth > row.clientWidth).length)
    )
    .toBe(0);

  // At 360 the gaps between the columns are wider than the list, so only the clip holds the view.
  await page.setViewportSize({ width: 360, height: 720 });
  await expect.poll(() => sidewaysOverflow(page)).toBe(0);
});

for (const view of ['grid', 'list'] as const) {
  test(`${view}: files that come and go never make the view wider, and the view ends up matching the folder`, async ({
    page,
  }) => {
    let names = Array.from({ length: 4 }, (_, i) => `file-${i}.txt`);
    const setFiles = () =>
      page.evaluate(
        (next) =>
          (
            window as unknown as { __TEST_UTILS__: { setChangingFiles: (names: string[]) => void } }
          ).__TEST_UTILS__.setChangingFiles(next),
        names
      );

    await page.setViewportSize({ width: 1180, height: 720 });
    await page.addInitScript(getTauriMockScript(view));
    await page.addInitScript((seeded) => {
      window.__MOCK_CHANGING_NAMES__ = seeded;
    }, names);
    await page.goto('/?path=' + encodeURIComponent(MOCK_CHANGING_DIR));

    await expect(page.locator('[data-testid="path-input"]')).toBeVisible({ timeout: 15000 });
    await expect(page.locator('[data-testid="file-item"]')).toHaveCount(4);

    // Keep the widest overflow of any frame, so one that lasts a single frame still fails.
    await page.evaluate(() => {
      window.__MAX_SIDEWAYS_OVERFLOW__ = 0;
      const scroller = document.querySelector('[data-main-content]')!;
      const tick = () => {
        window.__MAX_SIDEWAYS_OVERFLOW__ = Math.max(
          window.__MAX_SIDEWAYS_OVERFLOW__,
          scroller.scrollWidth - scroller.clientWidth
        );
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });

    // The watcher reads the folder after 400 ms of quiet, so wait for the vertical scrollbar.
    names = [
      ...names,
      ...Array.from({ length: 60 }, (_, i) => `bulk-${String(i).padStart(2, '0')}.txt`),
    ];
    await setFiles();
    await expect.poll(() => canScrollDown(page)).toBe(true);

    // What a browser download does, with no wait: a temp file, a rename, and now and then a delete.
    for (let i = 0; i < 12; i++) {
      names = [...names, `download-${i}.tmp`];
      await setFiles();

      names = names.map((name) => (name === `download-${i}.tmp` ? `download-${i}.txt` : name));
      if (i % 2 === 1) {
        const oldest = names.find((name) => name.startsWith('download-'));
        names = names.filter((name) => name !== oldest);
      }
      await setFiles();
    }

    names = names.filter((name) => !name.startsWith('bulk-'));
    await setFiles();
    await expect.poll(() => canScrollDown(page)).toBe(false);

    const items = page.locator('[data-testid="file-item"]');
    await expect
      .poll(() => items.evaluateAll((els) => els.map((el) => el.getAttribute('data-name')).sort()))
      .toEqual([...names].sort());
    // The fade-in takes 200 ms.
    await expect
      .poll(() =>
        items.evaluateAll(
          (els) =>
            els.filter(
              (el) =>
                el.hasAttribute('data-entering') ||
                el.hasAttribute('data-exiting') ||
                getComputedStyle(el).opacity !== '1'
            ).length
        )
      )
      .toBe(0);

    expect(await page.evaluate(() => window.__MAX_SIDEWAYS_OVERFLOW__)).toBe(0);
  });
}
