import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');

export default function setup(): void {
  // A screenshot left by an earlier run would read as a failure of this one.
  fs.rmSync(path.join(root, 'test-results', 'app'), { recursive: true, force: true });
  execFileSync(
    'npm',
    ['run', 'tauri', '--', 'build', '--debug', '--no-bundle', '--features', 'app-test'],
    {
      cwd: root,
      env: {
        ...process.env,
        CARGO_TARGET_DIR: path.join(root, 'src-tauri/target/app-test'),
      },
      stdio: 'inherit',
    }
  );
}
