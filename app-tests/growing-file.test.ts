import fs from 'node:fs/promises';
import path from 'node:path';
import { afterAll, beforeAll, describe, expect, test } from 'vitest';
import { commandOrControl, openFolder, sizeBytes, startApp } from './app';

// On macOS the file events for a file written through one open handle come only when it
// is created and closed, so the list does not change while it grows.
describe.skipIf(process.platform !== 'linux')('a file that is still being written', () => {
  let app: Awaited<ReturnType<typeof startApp>>;

  beforeAll(async () => {
    app = await startApp();
  });

  afterAll(async () => {
    await app?.stop();
  });

  // A write every 20 ms keeps the folder from going quiet: the app reports a change about
  // every 300 ms, and the list refreshes on 400 ms of quiet. So only the 2-second cap in
  // useDirectoryWatcher.ts refreshes the list before the file is closed. Writes 100 ms apart
  // can leave a 400 ms gap, which would let the test pass without the cap.
  test('the list follows a file while it grows and shows its final size when it is done', async () => {
    const { browser, files } = app;
    const dir = path.join(files, 'growing');
    const file = path.join(dir, 'download.bin');
    let written = 0;
    await fs.mkdir(dir);
    await openFolder(browser, dir);
    await browser.keys([commandOrControl, '2']);
    await browser.$('//p[text()="This folder is empty"]').waitForExist({
      timeout: 10_000,
      timeoutMsg: 'the empty list never appeared',
    });

    // The writer runs on its own, not in step with the reads below. A failed write still
    // clears the flag, so the read loop ends.
    let writing = true;
    const writer = (async () => {
      try {
        const handle = await fs.open(file, 'w');
        try {
          const start = Date.now();
          while (Date.now() - start < 8_000) {
            await handle.write(Buffer.alloc(16_384, 1));
            written += 16_384;
            await new Promise((resolve) => setTimeout(resolve, 20));
          }
        } finally {
          await handle.close();
        }
      } finally {
        writing = false;
      }
    })();

    // Every size the list showed while the file was still being written.
    const partialSizes = new Set<number>();
    try {
      while (writing) {
        const size = await sizeBytes(browser);
        if (writing && size > 0) partialSizes.add(size);
      }
    } finally {
      await writer;
    }
    await browser.waitUntil(async () => (await sizeBytes(browser)) === written, {
      timeout: 5_000,
      timeoutMsg: 'the list never showed the final size of the file',
    });
    // Eight seconds of writing gives the cap three or four refreshes. One alone could be a
    // stray quiet moment.
    expect(
      partialSizes.size,
      'the list refreshed fewer than twice while the file was being written'
    ).toBeGreaterThanOrEqual(2);
  });
});
