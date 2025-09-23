import { spawn } from 'node:child_process';
import os from 'node:os';
import path from 'node:path';
import fs from 'node:fs/promises';
import { createHash } from 'node:crypto';

const args = process.argv.slice(2);
const env = { ...process.env };

const LINUXDEPLOY_OVERRIDES = {
  x64: {
    filename: 'linuxdeploy-x86_64.AppImage',
    url: 'https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20240109-1/linuxdeploy-x86_64.AppImage',
    sha256: 'c86d6540f1df31061f02f539a2d3445f8d7f85cc3994eee1e74cd1ac97b76df0',
    versionLabel: '1-alpha-20240109-1',
  },
  arm64: {
    filename: 'linuxdeploy-aarch64.AppImage',
    url: 'https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20240109-1/linuxdeploy-aarch64.AppImage',
    sha256: '77d4d5918b5c9c7620dd74465c717ea59e8655eb83410cd86ebd24cec38c4679',
    versionLabel: '1-alpha-20240109-1',
  },
};

const LINUXDEPLOY_PLUGIN_OVERRIDES = {
  x64: {
    filename: 'linuxdeploy-plugin-appimage.AppImage',
    url: 'https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/1-alpha-20230713-1/linuxdeploy-plugin-appimage-x86_64.AppImage',
    sha256: '1c77541ad7903bc3ea3f235d4c8197557a0559f0fff6e6f768c3fd6918e9b4e3',
    versionLabel: '1-alpha-20230713-1',
  },
};

async function ensureLinuxdeployBinary() {
  const override = LINUXDEPLOY_OVERRIDES[process.arch];
  if (!override) return;

  const cacheDir = path.join(os.homedir(), '.cache', 'tauri');
  const targetPath = path.join(cacheDir, override.filename);

  await fs.mkdir(cacheDir, { recursive: true });

  let needsDownload = false;
  try {
    const existing = await fs.readFile(targetPath);
    const digest = createHash('sha256').update(existing).digest('hex');
    if (digest !== override.sha256) {
      needsDownload = true;
      console.info('[marlin] Replacing cached linuxdeploy binary due to checksum mismatch.');
    }
  } catch (error) {
    if (error.code === 'ENOENT') {
      needsDownload = true;
    } else {
      throw error;
    }
  }

  if (needsDownload) {
    console.info(`[marlin] Fetching linuxdeploy ${override.versionLabel} for ${process.arch}.`);
    const response = await fetch(override.url);
    if (!response.ok) {
      throw new Error(
        `Failed to download linuxdeploy from ${override.url}: ${response.status} ${response.statusText}`
      );
    }
    const buffer = Buffer.from(await response.arrayBuffer());
    const digest = createHash('sha256').update(buffer).digest('hex');
    if (digest !== override.sha256) {
      throw new Error(
        `linuxdeploy checksum mismatch (expected ${override.sha256}, received ${digest}).`
      );
    }
    await fs.writeFile(targetPath, buffer, { mode: 0o770 });
  }

  const zsyncPath = `${targetPath}.zsync`;
  try {
    await fs.unlink(zsyncPath);
  } catch (error) {
    if (error.code !== 'ENOENT') {
      throw error;
    }
  }
}

async function ensureLinuxdeployPlugin() {
  const override = LINUXDEPLOY_PLUGIN_OVERRIDES[process.arch];
  if (!override) return;

  const cacheDir = path.join(os.homedir(), '.cache', 'tauri');
  const targetPath = path.join(cacheDir, override.filename);

  await fs.mkdir(cacheDir, { recursive: true });

  let needsDownload = false;
  try {
    const existing = await fs.readFile(targetPath);
    const digest = createHash('sha256').update(existing).digest('hex');
    if (digest !== override.sha256) {
      needsDownload = true;
      console.info(
        '[marlin] Replacing cached linuxdeploy appimage plugin due to checksum mismatch.'
      );
    }
  } catch (error) {
    if (error.code === 'ENOENT') {
      needsDownload = true;
    } else {
      throw error;
    }
  }

  if (needsDownload) {
    console.info(
      `[marlin] Fetching linuxdeploy AppImage plugin ${override.versionLabel} for ${process.arch}.`
    );
    const response = await fetch(override.url);
    if (!response.ok) {
      throw new Error(
        `Failed to download linuxdeploy AppImage plugin from ${override.url}: ${response.status} ${response.statusText}`
      );
    }
    const buffer = Buffer.from(await response.arrayBuffer());
    const digest = createHash('sha256').update(buffer).digest('hex');
    if (digest !== override.sha256) {
      throw new Error(
        `linuxdeploy AppImage plugin checksum mismatch (expected ${override.sha256}, received ${digest}).`
      );
    }
    await fs.writeFile(targetPath, buffer, { mode: 0o770 });
  }

  try {
    await fs.unlink(`${targetPath}.zsync`);
  } catch (error) {
    if (error.code !== 'ENOENT') {
      throw error;
    }
  }
}

if (process.platform === 'linux') {
  await ensureLinuxdeployBinary();
  await ensureLinuxdeployPlugin();

  if (!env.APPIMAGE_EXTRACT_AND_RUN) {
    env.APPIMAGE_EXTRACT_AND_RUN = '1';
    console.info('[marlin] Enabled AppImage extract-and-run mode to avoid FUSE requirements.');
  }

  if (env.LD_LIBRARY_PATH) {
    const filtered = env.LD_LIBRARY_PATH.split(':').filter(
      (part) => part && !part.includes('/snap/')
    );
    if (filtered.length === 0) {
      delete env.LD_LIBRARY_PATH;
    } else if (filtered.length !== env.LD_LIBRARY_PATH.split(':').length) {
      env.LD_LIBRARY_PATH = filtered.join(':');
      console.info('[marlin] Sanitized LD_LIBRARY_PATH to avoid Snap glibc conflicts.');
    }
  }

  const SNAP_SUFFIX = '_VSCODE_SNAP_ORIG';
  const restoredKeys = [];
  for (const [key, value] of Object.entries(env)) {
    if (!key.endsWith(SNAP_SUFFIX)) continue;
    const baseKey = key.slice(0, -SNAP_SUFFIX.length);
    const current = env[baseKey];
    if (!current || !current.includes('/snap/')) continue;
    if (value) env[baseKey] = value;
    else delete env[baseKey];
    restoredKeys.push(baseKey);
  }
  if (restoredKeys.length) {
    console.info(`[marlin] Restored VS Code snap overrides for: ${restoredKeys.join(', ')}.`);
  }

  const removedKeys = [];
  for (const key of [
    'GTK_PATH',
    'GIO_MODULE_DIR',
    'GTK_IM_MODULE_FILE',
    'GTK_EXE_PREFIX',
    'GSETTINGS_SCHEMA_DIR',
    'LOCPATH',
  ]) {
    if (env[key] && env[key].includes('/snap/')) {
      delete env[key];
      removedKeys.push(key);
    }
  }
  if (removedKeys.length) {
    console.info(`[marlin] Removed snap-related environment keys: ${removedKeys.join(', ')}.`);
  }
}

const command = process.platform === 'win32' ? 'tauri.cmd' : 'tauri';
const child = spawn(command, args, { env, stdio: 'inherit' });

child.on('exit', (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
  } else {
    process.exit(code ?? 0);
  }
});

child.on('error', (error) => {
  console.error('[marlin] Failed to launch the Tauri CLI:', error);
  process.exit(1);
});
