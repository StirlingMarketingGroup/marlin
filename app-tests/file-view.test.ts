import fs from 'node:fs/promises';
import path from 'node:path';
import { afterAll, beforeAll, expect, test } from 'vitest';
import type { Browser } from 'webdriverio';
import { commandOrControl, fileRow, openFolder, rowNames, sizeBytes, startApp } from './app';

declare global {
  interface Window {
    __MAX_SIDEWAYS_OVERFLOW__?: number;
    __TRACK_SIDEWAYS_OVERFLOW__?: boolean;
  }
}

let app: Awaited<ReturnType<typeof startApp>>;

beforeAll(async () => {
  app = await startApp();
});

afterAll(async () => {
  await app?.stop();
});

function sidewaysOverflow(browser: Browser): Promise<number> {
  return browser.execute(() => {
    const scroller = document.querySelector('[data-main-content]')!;
    return scroller.scrollWidth - scroller.clientWidth;
  });
}

function canScrollDown(browser: Browser): Promise<boolean> {
  return browser.execute(() => {
    const scroller = document.querySelector('[data-main-content]')!;
    return scroller.scrollHeight > scroller.clientHeight;
  });
}

function columnCount(browser: Browser): Promise<number> {
  return browser.execute(() => {
    const row = document.querySelector('.file-grid')!;
    return getComputedStyle(row).gridTemplateColumns.split(' ').length;
  });
}

// Grouping characters in the count depend on the locale.
async function fileCount(browser: Browser): Promise<number> {
  const text = await browser.$('//span[text()="Files"]/following-sibling::span').getText();
  return Number(text.replace(/\D/g, ''));
}

// The listing streams in batches of 100 from Rust, and both views are virtual, so a lost or
// doubled batch shows only here. Walking both views takes hundreds of WebDriver calls.
test(
  'a folder of 500 files shows every file once, in list view and in grid view',
  { timeout: 300_000 },
  async () => {
    const { browser, files } = app;
    await browser.setWindowSize(1200, 800);

    const dir = path.join(files, 'many');
    const expected = Array.from(
      { length: 500 },
      (_, index) => `file-${String(index).padStart(3, '0')}.txt`
    );
    await fs.mkdir(dir);
    await Promise.all(expected.map((name) => fs.writeFile(path.join(dir, name), 'x')));
    await openFolder(browser, dir);
    await browser.waitUntil(
      async () => (await fileCount(browser)) === 500 && (await sizeBytes(browser)) === 500,
      { timeout: 10_000, timeoutMsg: 'the status bar never showed 500 files of 500 bytes' }
    );

    for (const [shortcut, view] of [
      ['2', 'file-list'],
      ['1', 'file-grid'],
    ] as const) {
      await browser.keys([commandOrControl, shortcut]);
      await browser.$(`[data-testid="${view}"]`).waitForExist({
        timeout: 10_000,
        timeoutMsg: `${view} never appeared`,
      });

      // Half a screen per step, so each step overlaps the one before it.
      const step = await browser.execute(() => {
        const scroller = document.querySelector('[data-main-content]')!;
        scroller.scrollTop = 0;
        return scroller.clientHeight / 2;
      });
      // The view keeps the rows it showed until the scroll is drawn. Rows from the bottom of
      // the last view would end the walk before the top files were collected.
      await browser.waitUntil(async () => (await rowNames(browser)).includes('file-000.txt'), {
        timeout: 10_000,
        timeoutMsg: 'the view never scrolled back to the first file',
      });

      const seen = new Set<string>();
      let last = '';
      while (last !== 'file-499.txt') {
        let names: string[] = [];
        await browser.waitUntil(
          async () => {
            names = await rowNames(browser);
            return names.length > 0 && names[names.length - 1] > last;
          },
          { timeout: 10_000, timeoutMsg: `no file after ${last} appeared while scrolling` }
        );
        expect(new Set(names).size).toBe(names.length);
        for (const name of names) seen.add(name);
        last = names[names.length - 1];
        await browser.execute((by: number) => {
          document.querySelector('[data-main-content]')!.scrollTop += by;
        }, step);
      }
      expect([...seen].sort()).toEqual(expected);
    }
  }
);

// The first cause of #480. The grid's element leaves the page while the folder is empty, and
// the grid once measured its width only when that element first appeared.
test("a grid whose folder empties and fills again follows the window's width", async () => {
  const { browser, files } = app;
  await browser.setWindowSize(1280, 800);

  const dir = path.join(files, 'grows');
  await fs.mkdir(dir);
  await fs.writeFile(path.join(dir, 'seed.txt'), 'x');
  await openFolder(browser, dir);
  await browser.keys([commandOrControl, '1']);
  await browser.$('[data-testid="file-grid"]').waitForExist({
    timeout: 10_000,
    timeoutMsg: 'grid view never appeared',
  });

  await fs.rm(path.join(dir, 'seed.txt'));
  await browser.waitUntil(
    async () => {
      const empty = await browser.$('//p[text()="This folder is empty"]').isExisting();
      const grid = await browser.$('[data-testid="file-grid"]').isExisting();
      return empty && !grid;
    },
    {
      timeout: 10_000,
      timeoutMsg: 'the grid never left the page when the folder emptied',
    }
  );

  await Promise.all(
    Array.from({ length: 40 }, (_, index) =>
      fs.writeFile(path.join(dir, `file-${String(index).padStart(2, '0')}.txt`), 'x')
    )
  );
  await browser.waitUntil(
    async () => (await fileCount(browser)) === 40 && (await browser.$$('.file-grid').length) > 0,
    { timeout: 10_000, timeoutMsg: 'the grid never showed the 40 files' }
  );

  const columnsBefore = await columnCount(browser);
  await browser.setWindowSize(900, 800);
  await browser.waitUntil(async () => (await columnCount(browser)) < columnsBefore, {
    timeout: 10_000,
    timeoutMsg: 'the grid never dropped a column when the window narrowed',
  });

  const tilesPastTheEdge = await browser.execute(() => {
    const scroller = document.querySelector('[data-main-content]')!;
    const row = document.querySelector('.file-grid')!;
    const right = scroller.getBoundingClientRect().right;
    return Array.from(row.querySelectorAll('[data-file-item="true"]')).filter(
      (tile) => tile.getBoundingClientRect().right > right
    ).length;
  });
  expect(tilesPastTheEdge).toBe(0);
});

// The second cause of #480: list cells let their text hang past the row.
test('a narrow list has no sideways overflow', async () => {
  const { browser, files } = app;
  await browser.setWindowSize(1200, 800);

  const dir = path.join(files, 'narrow');
  const longName = `${'alpha'.repeat(10)}.txt`;
  await fs.mkdir(dir);
  await fs.writeFile(path.join(dir, longName), 'a');
  await fs.writeFile(path.join(dir, `${'bravo'.repeat(10)}.txt`), 'bb');
  await fs.writeFile(path.join(dir, `${'charlie'.repeat(8)}.txt`), 'ccc');
  await openFolder(browser, dir);
  await browser.keys([commandOrControl, '2']);
  await browser.$('[data-testid="file-list"]').waitForExist({
    timeout: 10_000,
    timeoutMsg: 'list view never appeared',
  });
  await fileRow(browser, longName).waitForExist({
    timeout: 10_000,
    timeoutMsg: 'the list never showed its files',
  });

  await browser.setWindowSize(700, 800);
  await browser.waitUntil(
    () =>
      browser.execute(() => {
        const rows = Array.from(document.querySelectorAll('[data-file-item="true"]'));
        return rows.length > 0 && rows.every((row) => row.scrollWidth <= row.clientWidth);
      }),
    { timeout: 10_000, timeoutMsg: 'a list row stayed wider than its box' }
  );

  // At 360 the gaps between the list's columns are wider than the list, so only the view's clip holds it.
  await browser.setWindowSize(360, 800);
  await browser.waitUntil(async () => (await sidewaysOverflow(browser)) === 0, {
    timeout: 10_000,
    timeoutMsg: 'the list still stuck out past the window',
  });
});

for (const view of ['grid', 'list'] as const) {
  test(`${view}: files that come and go never make the view wider, and the view ends up matching the folder`, async () => {
    const { browser, files } = app;
    await browser.setWindowSize(1180, 720);

    const dir = path.join(files, `churn-${view}`);
    const seeded = Array.from({ length: 4 }, (_, index) => `file-${index}.txt`);
    await fs.mkdir(dir);
    await Promise.all(seeded.map((name) => fs.writeFile(path.join(dir, name), 'x')));
    await openFolder(browser, dir);
    await browser.keys([commandOrControl, view === 'grid' ? '1' : '2']);
    const viewId = view === 'grid' ? 'file-grid' : 'file-list';
    await browser.$(`[data-testid="${viewId}"]`).waitForExist({
      timeout: 10_000,
      timeoutMsg: `${view} view never appeared`,
    });
    await browser.waitUntil(
      async () => (await rowNames(browser)).join('\n') === seeded.join('\n'),
      { timeout: 10_000, timeoutMsg: 'the view never showed its four files' }
    );

    // The loop runs only while this flag is set. Both tests share the page, so the first
    // test's loop must not run on into the second.
    await browser.execute(() => {
      window.__MAX_SIDEWAYS_OVERFLOW__ = 0;
      window.__TRACK_SIDEWAYS_OVERFLOW__ = true;
      const scroller = document.querySelector('[data-main-content]')!;
      const tick = () => {
        if (window.__TRACK_SIDEWAYS_OVERFLOW__ !== true) return;
        window.__MAX_SIDEWAYS_OVERFLOW__ = Math.max(
          window.__MAX_SIDEWAYS_OVERFLOW__ ?? 0,
          scroller.scrollWidth - scroller.clientWidth
        );
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
    try {
      const bulk = Array.from(
        { length: 60 },
        (_, index) => `bulk-${String(index).padStart(2, '0')}.txt`
      );
      await Promise.all(bulk.map((name) => fs.writeFile(path.join(dir, name), 'x')));
      await browser.waitUntil(async () => await canScrollDown(browser), {
        timeout: 10_000,
        timeoutMsg: 'the folder never grew a scrollbar',
      });

      // What a browser download does, with no wait: a temp file, a rename, and now and then a delete.
      const downloads: string[] = [];
      for (let i = 0; i < 12; i++) {
        await fs.writeFile(path.join(dir, `download-${i}.tmp`), 'x');
        await fs.rename(path.join(dir, `download-${i}.tmp`), path.join(dir, `download-${i}.txt`));
        downloads.push(`download-${i}.txt`);
        if (i % 2 === 1) await fs.rm(path.join(dir, downloads.shift()!));
        expect(await sidewaysOverflow(browser)).toBe(0);
      }

      await Promise.all(bulk.map((name) => fs.rm(path.join(dir, name))));
      await browser.waitUntil(async () => !(await canScrollDown(browser)), {
        timeout: 10_000,
        timeoutMsg: 'the scrollbar never went away',
      });
      const onDisk = (await fs.readdir(dir)).sort().join('\n');
      await browser.waitUntil(async () => (await rowNames(browser)).join('\n') === onDisk, {
        timeout: 10_000,
        timeoutMsg: 'the view never matched the folder',
      });
      expect(await browser.execute(() => window.__MAX_SIDEWAYS_OVERFLOW__ ?? 0)).toBe(0);
    } finally {
      await browser.execute(() => {
        window.__TRACK_SIDEWAYS_OVERFLOW__ = false;
      });
    }
  });
}
