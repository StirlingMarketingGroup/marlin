import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');

export default function setup(): void {
  // A screenshot left by an earlier run would read as a failure of this one.
  fs.rmSync(path.join(root, 'test-results', 'app'), { recursive: true, force: true });
  const env = {
    ...process.env,
    CARGO_TARGET_DIR: path.join(root, 'src-tauri/target/app-test'),
  };
  execFileSync(
    'npm',
    ['run', 'tauri', '--', 'build', '--debug', '--no-bundle', '--features', 'app-test'],
    {
      cwd: root,
      env,
      stdio: 'inherit',
    }
  );
  // The app starts the mount helper beside its binary, and the app's build put the plain helper
  // there, the one in src-tauri/binaries/ that `npm run tauri dev` uses. This puts the test
  // build's helper in its place, and builds the fake Google that tests start.
  execFileSync(
    'cargo',
    [
      'build',
      '-p',
      'marlin-mount',
      '-p',
      'marlin-test-google',
      '--features',
      'marlin-mount/app-test',
    ],
    {
      cwd: path.join(root, 'src-tauri'),
      env,
      stdio: 'inherit',
    }
  );
}
