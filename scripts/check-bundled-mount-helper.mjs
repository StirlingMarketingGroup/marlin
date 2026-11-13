// CI runs this on a built bundle: `node scripts/check-bundled-mount-helper.mjs <bundle folder>`.
// It proves that the bundle holds a real `marlin-mount`, signed on macOS, and not the
// empty placeholder that `src-tauri/build.rs` makes when nothing built the helper.
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import readline from 'node:readline';
import { setTimeout as sleep } from 'node:timers/promises';

const bundle = process.argv[2];

// A short name on purpose: the path of a Unix socket may be about 100 bytes at most.
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'mh-'));
let child;
process.on('exit', () => {
  child?.kill('SIGKILL');
  fs.rmSync(tmp, { recursive: true, force: true });
});

let helper;
if (process.platform === 'darwin') {
  const app = path.join(bundle, 'macos', 'Marlin.app');
  helper = path.join(app, 'Contents', 'MacOS', 'marlin-mount');
  execFileSync('codesign', ['--verify', '--deep', '--strict', app], { stdio: 'inherit' });
  // Apple's notary service refuses a program that is signed without the hardened runtime.
  const signature = spawnSync('codesign', ['--display', '--verbose=2', helper], {
    encoding: 'utf8',
  }).stderr;
  if (!/flags=.*runtime/.test(signature)) {
    throw new Error(`${helper} is not signed with the hardened runtime:\n${signature}`);
  }
} else {
  // The package a user installs, not the folder Tauri built it from. A self-hosted
  // runner keeps the packages of older versions in the same folder.
  const { version } = JSON.parse(
    fs.readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8')
  );
  const debs = fs
    .readdirSync(path.join(bundle, 'deb'))
    .filter((name) => name.startsWith(`Marlin_${version}_`) && name.endsWith('.deb'));
  if (debs.length !== 1) {
    throw new Error(`Expected one Marlin_${version}_*.deb in ${bundle}/deb, found ${debs.length}.`);
  }
  execFileSync('dpkg-deb', ['-x', path.join(bundle, 'deb', debs[0]), path.join(tmp, 'deb')]);
  helper = path.join(tmp, 'deb', 'usr', 'bin', 'marlin-mount');
}

const socketPath = path.join(tmp, 's');
child = spawn(helper, ['--socket', socketPath, '--root', path.join(tmp, 'root')], {
  stdio: ['ignore', 'ignore', 'inherit'],
});
setTimeout(() => {
  console.error(`${helper} did not answer hello and shut down within 30 seconds.`);
  process.exit(1);
}, 30_000).unref();

// The socket appears a moment after the helper starts.
let socket;
for (;;) {
  socket = net.createConnection(socketPath);
  try {
    await once(socket, 'connect');
    break;
  } catch {
    await sleep(100);
  }
}
const lines = readline.createInterface({ input: socket });

// The helper answers `hello` and takes `shutdown` whatever protocol number the client sends.
socket.write('{"jsonrpc":"2.0","id":1,"method":"hello","params":{"protocol":1}}\n');
const [answer] = await once(lines, 'line');
const version = JSON.parse(answer).result?.version;
if (typeof version !== 'string') {
  throw new Error(`${helper} answered hello with: ${answer}`);
}

socket.write('{"jsonrpc":"2.0","id":2,"method":"shutdown"}\n');
const [code] = await once(child, 'exit');
if (code !== 0) {
  throw new Error(`${helper} exited with status ${code} after shutdown.`);
}
console.log(`marlin-mount ${version} from the bundle answered hello.`);
