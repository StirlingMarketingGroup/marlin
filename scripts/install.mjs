#!/usr/bin/env node
import os from 'os';
import fs from 'fs/promises';
import path from 'path';
import { spawn } from 'child_process';
import { pipeline } from 'stream/promises';

const REPO = 'StirlingMarketingGroup/marlin';
const API_URL = `https://api.github.com/repos/${REPO}/releases/latest`;

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

async function download(url, destination) {
  const response = await fetch(url, {
    headers: {
      'User-Agent': 'marlin-installer',
    },
  });
  if (!response.ok || !response.body) {
    fatal(`Failed to download asset from ${url} (status ${response.status})`);
  }
  await fs.mkdir(path.dirname(destination), { recursive: true });
  const fileHandle = await fs.open(destination, 'w');
  try {
    await pipeline(response.body, fileHandle.createWriteStream());
  } finally {
    await fileHandle.close();
  }
}

async function getLatestRelease() {
  const response = await fetch(API_URL, {
    headers: {
      'User-Agent': 'marlin-installer',
    },
  });
  if (!response.ok) {
    fatal(`Failed to query GitHub API (status ${response.status})`);
  }
  return response.json();
}

async function installMac(assets, version) {
  const asset = assets.find(({ name }) => name.endsWith('_universal.dmg'));
  if (!asset) fatal('Unable to locate macOS DMG in release assets.');
  const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'marlin-dmg-'));
  const dmgPath = path.join(tmp, asset.name);
  console.log(`Downloading ${asset.name}...`);
  await download(asset.browser_download_url, dmgPath);
  const mountPoint = path.join(tmp, 'mount');
  await fs.mkdir(mountPoint);
  let mounted = false;
  try {
    console.log('Mounting DMG...');
    await run('hdiutil', ['attach', dmgPath, '-nobrowse', '-mountpoint', mountPoint]);
    mounted = true;
    const sourceApp = path.join(mountPoint, 'Marlin.app');
    const destApp = '/Applications/Marlin.app';
    await fs.rm(destApp, { recursive: true, force: true });
    await fs.cp(sourceApp, destApp, { recursive: true });
  } catch (error) {
    console.error(
      'Failed to install Marlin.app. Ensure you have write permissions (try running with sudo).'
    );
    throw error;
  } finally {
    if (mounted) {
      console.log('Unmounting DMG...');
      await run('hdiutil', ['detach', mountPoint]).catch((err) => {
        console.warn(`Failed to detach DMG: ${err.message}`);
      });
    }
    await fs.rm(tmp, { recursive: true, force: true }).catch(() => {});
  }
  console.log(`Marlin ${version} installed to /Applications/Marlin.app.`);
}

async function installWindows(assets, version) {
  const suffixes = (() => {
    switch (process.arch) {
      case 'arm64':
        return ['arm64', 'aarch64'];
      case 'x64':
        return ['x64'];
      default:
        return [];
    }
  })();
  if (!suffixes.length) fatal(`Unsupported Windows architecture ${process.arch}.`);
  const asset = assets.find(({ name }) =>
    suffixes.some((suffix) => name.endsWith(`${suffix}_en-US.msi`))
  );
  if (!asset) fatal(`Unable to locate Windows MSI (${suffixes.join('/')}) in release assets.`);
  const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'marlin-msi-'));
  const msiPath = path.join(tmp, asset.name);
  console.log(`Downloading ${asset.name}...`);
  await download(asset.browser_download_url, msiPath);
  console.log('Launching installer (msiexec)...');
  try {
    await run('msiexec', ['/i', msiPath]);
  } finally {
    await fs.rm(tmp, { recursive: true, force: true }).catch(() => {});
  }
  console.log(`Marlin ${version} installer launched.`);
}

async function installLinux(assets, version) {
  const arch = (() => {
    switch (process.arch) {
      case 'x64':
        return 'amd64';
      case 'arm64':
        return 'aarch64';
      default:
        return null;
    }
  })();
  if (!arch) fatal(`Unsupported Linux architecture ${process.arch}.`);
  const suffixes = arch === 'aarch64' ? ['aarch64', 'arm64'] : [arch];
  const asset = assets.find(({ name }) =>
    suffixes.some((suffix) => name.endsWith(`${suffix}.AppImage`))
  );
  if (!asset) fatal(`Unable to locate Linux AppImage (${arch}) in release assets.`);
  const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'marlin-appimage-'));
  const appImagePath = path.join(tmp, asset.name);
  console.log(`Downloading ${asset.name}...`);
  await download(asset.browser_download_url, appImagePath);
  await fs.chmod(appImagePath, 0o755);

  const preferred = '/usr/local/bin/marlin';
  let destination = preferred;
  try {
    await fs.access(path.dirname(preferred), fs.constants.W_OK);
  } catch {
    destination = path.join(os.homedir(), '.local/bin/marlin');
    await fs.mkdir(path.dirname(destination), { recursive: true });
  }

  await fs.rm(destination, { force: true }).catch(() => {});
  try {
    await fs.copyFile(appImagePath, destination);
    await fs.chmod(destination, 0o755);
  } finally {
    await fs.rm(tmp, { recursive: true, force: true }).catch(() => {});
  }

  console.log(`Marlin ${version} AppImage installed to ${destination}.`);
  if (destination.startsWith(path.join(os.homedir(), '.local'))) {
    console.log('Ensure ~/.local/bin is in your PATH to launch Marlin.');
  }
}

async function main() {
  try {
    const release = await getLatestRelease();
    const tag = release.tag_name;
    if (!tag) fatal('Latest release tag not found.');
    const version = tag.startsWith('v') ? tag.slice(1) : tag;
    const assets = release.assets || [];
    if (!assets.length) fatal('Latest release has no assets.');

    switch (process.platform) {
      case 'darwin':
        await installMac(assets, version);
        break;
      case 'win32':
        await installWindows(assets, version);
        break;
      case 'linux':
        await installLinux(assets, version);
        break;
      default:
        fatal(`Unsupported platform ${process.platform}`);
    }
  } catch (error) {
    fatal(error.message || String(error));
  }
}

main();
