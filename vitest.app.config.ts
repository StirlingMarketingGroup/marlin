import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    include: ['app-tests/**/*.test.ts'],
    environment: 'node',
    globalSetup: ['./app-tests/build-app.ts'],
    fileParallelism: false,
    testTimeout: 60_000,
    // startApp() waits up to 30 seconds for the server and 30 more for the first folder.
    hookTimeout: 120_000,
  },
});
