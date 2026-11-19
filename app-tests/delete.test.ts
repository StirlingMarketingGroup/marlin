import fs from 'node:fs/promises';
import path from 'node:path';
import { afterAll, beforeAll, expect, test } from 'vitest';
import { runCommand, startApp } from './app';

let app: Awaited<ReturnType<typeof startApp>>;

beforeAll(async () => {
  app = await startApp();
});

afterAll(async () => {
  await app?.stop();
});

test('a permanent delete removes a folder and a broken link', async () => {
  const { browser, files } = app;
  const tree = path.join(files, 'tree');
  const outside = path.join(files, 'outside');
  await fs.mkdir(outside);
  await fs.writeFile(path.join(outside, 'keep.txt'), 'keep\n');
  await Promise.all(
    Array.from({ length: 20 }, async (_, folder) => {
      const dir = path.join(tree, `folder-${folder}`);
      await fs.mkdir(dir, { recursive: true });
      await Promise.all(
        Array.from({ length: 50 }, (_, file) =>
          fs.writeFile(path.join(dir, `file-${file}.txt`), 'x')
        )
      );
    })
  );
  // The link inside the tree must not be followed.
  await fs.symlink(path.join('..', 'outside'), path.join(tree, 'link-to-outside'));
  const broken = path.join(files, 'broken');
  // A broken link must be deletable.
  await fs.symlink('gone', broken);

  const deleted = await runCommand(browser, 'delete_paths_permanently', {
    paths: [tree, broken],
    requestId: 'delete-test',
  });

  expect(deleted).toEqual({ deleted: [tree, broken] });
  expect(await fs.readdir(files)).toEqual(['outside']);
  expect(await fs.readFile(path.join(outside, 'keep.txt'), 'utf8')).toBe('keep\n');
});
