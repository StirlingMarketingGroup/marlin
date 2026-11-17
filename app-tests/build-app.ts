import { execFileSync } from 'node:child_process';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');

export default function setup(): void {
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
