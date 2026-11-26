import fs from 'node:fs/promises';
import path from 'node:path';
import { afterAll, beforeAll, expect, test } from 'vitest';
import { preferencesPath, runCommand, startApp } from './app';

let app: Awaited<ReturnType<typeof startApp>>;

beforeAll(async () => {
  app = await startApp();
});

afterAll(async () => {
  await app?.stop();
});

test('pins saved with a password or port 22 load without them, and the file is saved so', async () => {
  const { browser, home } = app;
  const file = path.join(path.dirname(preferencesPath(home)), 'pinned_directories.json');
  const pinnedAt = '2026-01-02T03:04:05Z';
  await fs.writeFile(
    file,
    JSON.stringify([
      { name: 'team', path: 'sftp://alice:secret@files.example:22/team', pinned_at: pinnedAt },
      {
        name: 'alice:secret@files.example:22',
        path: 'sftp://alice:secret@files.example:22',
        pinned_at: pinnedAt,
      },
      { name: 'docs', path: 'sftp://bob@files.example:22/docs', pinned_at: pinnedAt },
      { name: 'docs', path: 'sftp://bob@files.example/docs', pinned_at: pinnedAt },
    ])
  );

  const pins = (await runCommand(browser, 'get_pinned_directories')) as {
    name: string;
    path: string;
  }[];
  const want = [
    { name: 'team', path: 'sftp://alice@files.example/team' },
    { name: 'alice@files.example', path: 'sftp://alice@files.example' },
    { name: 'docs', path: 'sftp://bob@files.example/docs' },
  ];
  expect(pins.map((pin) => ({ name: pin.name, path: pin.path }))).toEqual(want);

  const text = await fs.readFile(file, 'utf8');
  expect(text).not.toContain('secret');
  expect(text).not.toContain(':22');
  const saved = JSON.parse(text) as { name: string; path: string }[];
  expect(saved.map((pin) => ({ name: pin.name, path: pin.path }))).toEqual(want);
});
