import type { Options } from '@wdio/types';
import { spawn, ChildProcess } from 'child_process';
import * as path from 'path';
import * as fs from 'fs';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

let tauriDriver: ChildProcess | null = null;

// Check if tauri-driver is already running externally (e.g., in CI)
const TAURI_DRIVER_EXTERNAL = process.env.TAURI_DRIVER_EXTERNAL === 'true';

// Find the app binary - check multiple possible locations
function findAppBinary(): string {
  const possiblePaths = [
    // Local development (macOS)
    path.join(__dirname, 'src-tauri/target/debug/marlin'),
    // Local development (Linux)
    path.join(__dirname, 'src-tauri/target/debug/marlin'),
    // CI build output (Linux bundle)
    path.join(__dirname, 'src-tauri/target/debug/bundle/appimage/marlin.AppImage'),
    // CI build output (Linux deb extracted)
    path.join(__dirname, 'src-tauri/target/debug/marlin'),
    // Explicit override from environment
    process.env.TAURI_APP_PATH || '',
  ];

  for (const appPath of possiblePaths) {
    if (appPath && fs.existsSync(appPath)) {
      console.log(`Found app binary at: ${appPath}`);
      return appPath;
    }
  }

  // Default fallback
  const defaultPath = path.join(__dirname, 'src-tauri/target/debug/marlin');
  console.log(`Using default app path: ${defaultPath}`);
  return defaultPath;
}

const APP_PATH = findAppBinary();

export const config: Options.Testrunner = {
  runner: 'local',
  autoCompileOpts: {
    autoCompile: true,
    tsNodeOpts: {
      project: './tsconfig.node.json',
      transpileOnly: true,
    },
  },

  specs: ['./e2e-wdio/**/*.spec.ts'],
  exclude: [],

  maxInstances: 1,

  capabilities: [
    {
      // Use tauri-driver as WebDriver server
      browserName: 'wry',
      'tauri:options': {
        application: APP_PATH,
      },
    } as WebdriverIO.Capabilities,
  ],

  logLevel: 'trace',
  bail: 0,
  waitforTimeout: 30000,
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

  services: [],

  framework: 'mocha',
  reporters: ['spec'],

  mochaOpts: {
    ui: 'bdd',
    timeout: 120000,
  },

  // Start tauri-driver before tests (unless running externally in CI)
  onPrepare: async function () {
    if (TAURI_DRIVER_EXTERNAL) {
      console.log('Using external tauri-driver (TAURI_DRIVER_EXTERNAL=true)');
      return;
    }

    console.log('Starting tauri-driver...');
    tauriDriver = spawn('tauri-driver', [], {
      stdio: ['pipe', 'pipe', 'pipe'],
    });

    tauriDriver.stdout?.on('data', (data) => {
      console.log(`[tauri-driver] ${data.toString().trim()}`);
    });

    tauriDriver.stderr?.on('data', (data) => {
      console.error(`[tauri-driver] ${data.toString().trim()}`);
    });

    // Wait for tauri-driver to be ready
    await new Promise((resolve) => setTimeout(resolve, 2000));
    console.log('tauri-driver started');
  },

  // Stop tauri-driver after tests
  onComplete: async function () {
    if (TAURI_DRIVER_EXTERNAL) {
      console.log('External tauri-driver - not stopping');
      return;
    }

    console.log('Stopping tauri-driver...');
    if (tauriDriver) {
      tauriDriver.kill();
      tauriDriver = null;
    }
  },

  // Default port for tauri-driver
  port: 4444,
  path: '/',
};
