import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: 'html',
  use: {
    baseURL: 'http://localhost:1420',
    trace: 'on-first-retry',
  },

  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],

  // Use Vite dev server only (not full Tauri) for browser-based e2e testing
  // The tests inject Tauri API mocks via addInitScript
  webServer: {
    command: 'npm run dev -- --port 1420',
    port: 1420,
    reuseExistingServer: !process.env.CI,
  },
});
