import { defineConfig, devices } from '@playwright/test';

// CI sets E2E_PORT to a free port: runners on one host share its ports.
const port = Number(process.env.E2E_PORT ?? 1420);

export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: 'html',
  use: {
    baseURL: `http://localhost:${port}`,
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
    command: `npm run dev -- --port ${port}`,
    port,
    reuseExistingServer: !process.env.CI,
  },
});
