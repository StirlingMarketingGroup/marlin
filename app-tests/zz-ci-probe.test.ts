// CI probe for #578, reverted before merge: a cancelled run and a failing test.
import { afterAll, beforeAll, expect, it } from 'vitest';
import { startApp } from './app';

let app: Awaited<ReturnType<typeof startApp>> | undefined;

beforeAll(async () => {
  app = await startApp();
});

afterAll(async () => {
  await app?.stop();
});

it('fails on purpose after a long wait', async () => {
  await new Promise((resolve) => setTimeout(resolve, 240_000));
  expect('probe').toBe('a failing test');
}, 300_000);
