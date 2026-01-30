#!/usr/bin/env node
import os from 'os';
import fs from 'fs/promises';
import path from 'path';
import { spawn } from 'child_process';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(__dirname, '..');

function fatal(message) {
  console.error(`Error: ${message}`);
  process.exit(1);
}

async function run(cmd, args, opts = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(cmd, args, { stdio: 'inherit', ...opts });
    child.on('error', reject);
    child.on('exit', (code) => {
      if (code === 0) {
        resolve();
      } else {
        reject(new Error(`${cmd} exited with code ${code}`));
      }
    });
  });
}

async function exists(p) {
  try {
    await fs.access(p);
    return true;
  } catch {
    return false;
  }
}

async function installMac() {
  const sourceApp = path.join(projectRoot, 'src-tauri/target/release/bundle/macos/Marlin.app');
  const destApp = '/Applications/Marlin.app';

  if (!(await exists(sourceApp))) {
    fatal(
      `Built app not found at ${sourceApp}.\nRun 'npm run tauri build' first, or use 'npm run install:local:build' to build and install.`
    );
  }

  console.log('Installing Marlin.app to /Applications...');
  try {
    await fs.rm(destApp, { recursive: true, force: true });
    await fs.cp(sourceApp, destApp, { recursive: true });
  } catch (error) {
    console.error(
      'Failed to install Marlin.app. You may need to run with sudo for /Applications access.'
    );
    throw error;
  }

  console.log('Marlin installed to /Applications/Marlin.app');
}

async function installWindows() {
  const arch = process.arch === 'arm64' ? 'aarch64' : 'x64';
  const msiPattern = new RegExp(`Marlin_.*_${arch}_en-US\\.msi$`);
  const bundleDir = path.join(projectRoot, 'src-tauri/target/release/bundle/msi');

  let msiPath;
  try {
    const files = await fs.readdir(bundleDir);
    const msi = files.find((f) => msiPattern.test(f));
    if (msi) {
      msiPath = path.join(bundleDir, msi);
    }
  } catch {
    // bundleDir doesn't exist
  }

  if (!msiPath || !(await exists(msiPath))) {
    fatal(
      `Built MSI not found.\nRun 'npm run tauri build' first, or use 'npm run install:local:build' to build and install.`
    );
  }

  console.log('Launching MSI installer...');
  await run('msiexec', ['/i', msiPath]);
  console.log('Marlin installer launched.');
}

async function installLinux() {
  const arch = process.arch === 'arm64' ? 'aarch64' : 'amd64';
  const appImagePattern = new RegExp(`marlin_.*_${arch}\\.AppImage$`, 'i');
  const bundleDir = path.join(projectRoot, 'src-tauri/target/release/bundle/appimage');

  let appImagePath;
  try {
    const files = await fs.readdir(bundleDir);
    const appImage = files.find((f) => appImagePattern.test(f));
    if (appImage) {
      appImagePath = path.join(bundleDir, appImage);
    }
  } catch {
    // bundleDir doesn't exist
  }

  if (!appImagePath || !(await exists(appImagePath))) {
    fatal(
      `Built AppImage not found.\nRun 'npm run tauri build' first, or use 'npm run install:local:build' to build and install.`
    );
  }

  const preferred = '/usr/local/bin/marlin';
  let destination = preferred;
  try {
    await fs.access(path.dirname(preferred), fs.constants.W_OK);
  } catch {
    destination = path.join(os.homedir(), '.local/bin/marlin');
    await fs.mkdir(path.dirname(destination), { recursive: true });
  }

  await fs.rm(destination, { force: true }).catch(() => {});
  await fs.copyFile(appImagePath, destination);
  await fs.chmod(destination, 0o755);

  console.log(`Marlin AppImage installed to ${destination}`);
  if (destination.startsWith(path.join(os.homedir(), '.local'))) {
    console.log('Ensure ~/.local/bin is in your PATH to launch Marlin.');
  }
}

async function main() {
  const shouldBuild = process.argv.includes('--build');

  if (shouldBuild) {
    console.log('Building production app...');
    await run('npm', ['run', 'tauri', 'build'], { cwd: projectRoot, shell: true });
  }

  switch (process.platform) {
    case 'darwin':
      await installMac();
      break;
    case 'win32':
      await installWindows();
      break;
    case 'linux':
      await installLinux();
      break;
    default:
      fatal(`Unsupported platform ${process.platform}`);
  }
}

main().catch((error) => {
  fatal(error.message || String(error));
});
