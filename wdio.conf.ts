import type { Options } from '@wdio/types';
import { spawn, ChildProcess } from 'child_process';
import * as path from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

let tauriDriver: ChildProcess | null = null;

const APP_PATH = path.join(__dirname, 'src-tauri/target/debug/marlin');

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

  logLevel: 'info',
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

  // Start tauri-driver before tests
  onPrepare: async function () {
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
