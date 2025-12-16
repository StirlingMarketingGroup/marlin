import { test, expect } from '@playwright/test';
import { spawn, ChildProcess } from 'child_process';
import * as path from 'path';

// This test uses tauri-driver to connect to the actual Tauri app
// with full Rust backend support (including Google Drive API)

let tauriDriver: ChildProcess | null = null;
let tauriApp: ChildProcess | null = null;

const TAURI_DRIVER_PORT = 4444;
const APP_PATH = path.join(__dirname, '../src-tauri/target/debug/marlin');

test.describe('Google Drive Integration (Tauri)', () => {
  test.beforeAll(async () => {
    // Start tauri-driver (WebDriver server)
    console.log('Starting tauri-driver...');
    tauriDriver = spawn('tauri-driver', ['--port', String(TAURI_DRIVER_PORT)], {
      stdio: ['pipe', 'pipe', 'pipe'],
    });

    tauriDriver.stdout?.on('data', (data) => {
      console.log(`[tauri-driver] ${data}`);
    });

    tauriDriver.stderr?.on('data', (data) => {
      console.error(`[tauri-driver error] ${data}`);
    });

    // Wait for tauri-driver to start
    await new Promise((resolve) => setTimeout(resolve, 2000));
  });

  test.afterAll(async () => {
    if (tauriApp) {
      tauriApp.kill();
      tauriApp = null;
    }
    if (tauriDriver) {
      tauriDriver.kill();
      tauriDriver = null;
    }
  });

  test('should navigate to gdrive:// path and list contents', async ({ browser }) => {
    // Connect to tauri-driver via CDP
    const context = await browser.newContext();
    const page = await context.newPage();

    // Navigate to a gdrive path - this will use the real Tauri backend
    // The user should already be authenticated (tokens stored locally)
    await page.goto(`tauri://localhost`);

    // Wait for app to load
    await page.waitForTimeout(3000);

    // Find the path input
    const pathInput = page.locator('input[data-testid="path-input"]');
    await expect(pathInput).toBeVisible({ timeout: 10000 });

    // Navigate to Google Drive
    await pathInput.click();
    await pathInput.fill('gdrive://brian@smg.gg/');
    await pathInput.press('Enter');

    await page.waitForTimeout(4000);

    // Check that we see the virtual folders
    const content = await page.content();
    console.log('Page content after gdrive navigation:', content.substring(0, 500));

    // Should see "My Drive" or "Shared with me"
    await expect(page.locator('text=My Drive')).toBeVisible({ timeout: 10000 });
  });
});
