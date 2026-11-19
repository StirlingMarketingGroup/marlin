// The saved servers, the keychain, the host key check, and the connect live in `marlin-sftp`.
// Only the running app shows the commands, that crate, the in-memory keychain, and the temporary home working together.
import { execFile, spawn, type ChildProcess } from 'node:child_process';
import fs from 'node:fs/promises';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { afterAll, beforeAll, expect, test } from 'vitest';
import { freePort, runCommand, startApp } from './app';

const execFileAsync = promisify(execFile);
const sshdPath = '/usr/sbin/sshd';

let dir: string | undefined;
let sshd: ChildProcess | undefined;
// Settles once sshd has ended, however it ended: a failed spawn emits `close` but no `exit`.
let sshdClosed: Promise<void> | undefined;
let sshdLog: fs.FileHandle | undefined;
let app: Awaited<ReturnType<typeof startApp>> | undefined;
let port: number;
let fingerprint: string;
let served: string;

const username = os.userInfo().username;

async function sshdLogText(): Promise<string> {
  if (dir === undefined) return '';
  return fs.readFile(path.join(dir, 'sshd.log'), 'utf8').catch(() => '');
}

beforeAll(async () => {
  dir = await fs.realpath(await fs.mkdtemp('/tmp/marlin-sshd-'));
  try {
    await fs.access(sshdPath);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') {
      throw new Error(`Install OpenSSH's server. ${sshdPath} does not exist.`);
    }
    throw error;
  }

  await execFileAsync('ssh-keygen', [
    '-q',
    '-t',
    'ed25519',
    '-N',
    '',
    '-f',
    path.join(dir, 'host_key'),
  ]);
  // The app keeps this passphrase in the keychain and reads it back to sign in, so a broken
  // keychain read fails the connection.
  await execFileAsync('ssh-keygen', [
    '-q',
    '-t',
    'ed25519',
    '-N',
    'marlin test passphrase',
    '-f',
    path.join(dir, 'client_key'),
  ]);
  await fs.copyFile(path.join(dir, 'client_key.pub'), path.join(dir, 'authorized_keys'));

  port = await freePort();
  const config = path.join(dir, 'sshd_config');
  await fs.writeFile(
    config,
    [
      `Port ${port}`,
      'ListenAddress 127.0.0.1',
      `HostKey ${dir}/host_key`,
      `PidFile ${dir}/sshd.pid`,
      `AuthorizedKeysFile ${dir}/authorized_keys`,
      'PasswordAuthentication no',
      'KbdInteractiveAuthentication no',
      'PubkeyAuthentication yes',
      'UsePAM no',
      'StrictModes no',
      'Subsystem sftp internal-sftp',
      '',
    ].join('\n')
  );

  served = path.join(dir, 'served');
  await fs.mkdir(served);
  await fs.writeFile(path.join(served, 'hello.txt'), 'hello over sftp\n');

  const logPath = path.join(dir, 'sshd.log');
  sshdLog = await fs.open(logPath, 'a');
  let ended: string | undefined;
  sshd = spawn(sshdPath, ['-D', '-e', '-f', config], {
    stdio: ['ignore', sshdLog.fd, sshdLog.fd],
  });
  sshdClosed = new Promise((resolve) => sshd?.once('close', () => resolve()));
  sshd.once('error', (error) => {
    ended = error.message;
  });
  sshd.once('exit', (code, signal) => {
    ended = `code ${code}, signal ${signal}`;
  });

  const deadline = Date.now() + 10_000;
  for (;;) {
    if (ended !== undefined) {
      throw new Error(
        `sshd ended before it accepted a connection: ${ended}\n${await sshdLogText()}`
      );
    }
    const open = await new Promise<boolean>((resolve) => {
      const socket = net.connect(port, '127.0.0.1');
      socket.once('connect', () => {
        socket.end();
        resolve(true);
      });
      socket.once('error', () => {
        socket.destroy();
        resolve(false);
      });
    });
    if (open) break;
    if (Date.now() > deadline) {
      throw new Error(
        `sshd did not accept a connection on port ${port} within 10 seconds.\n${await sshdLogText()}`
      );
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }

  const { stdout } = await execFileAsync('ssh-keygen', [
    '-l',
    '-E',
    'sha256',
    '-f',
    path.join(dir, 'host_key.pub'),
  ]);
  const field = stdout.trim().split(/\s+/)[1];
  if (field === undefined || !field.startsWith('SHA256:')) {
    throw new Error(`ssh-keygen did not print a SHA256 fingerprint: ${stdout}`);
  }
  fingerprint = field;

  app = await startApp();
});

afterAll(async () => {
  try {
    await app?.stop();
  } finally {
    sshd?.kill();
    const forceKill = setTimeout(() => sshd?.kill('SIGKILL'), 5_000);
    await sshdClosed;
    clearTimeout(forceKill);
    await sshdLog?.close();
    if (dir !== undefined) {
      await fs.rm(dir, { recursive: true, force: true });
    }
  }
});

test(
  'an SFTP server is added, trusted, listed, read, and removed through the app',
  { timeout: 60_000 },
  async () => {
    const { browser, home } = app!;
    const server = {
      hostname: '127.0.0.1',
      port,
      username,
      authMethod: 'key',
    };
    const remote = `sftp://${username}@127.0.0.1:${port}${served}`;

    const added = await runCommand(browser, 'add_sftp_server', {
      ...server,
      password: 'marlin test passphrase',
      keyPath: path.join(dir!, 'client_key'),
    });
    expect(added).toMatchObject(server);

    const listed = await runCommand(browser, 'get_sftp_servers');
    expect(listed).toEqual([added]);

    const refused = await runCommand(browser, 'read_directory', { path: remote });
    expect(refused).toContain(
      `[SFTP_HOST_KEY_UNKNOWN] 127.0.0.1:${port} is not in ${home}/.ssh/known_hosts.`
    );
    expect(refused).toContain(fingerprint);

    const unknown = await runCommand(browser, 'get_sftp_unknown_host_key', {
      hostname: '127.0.0.1',
      port,
    });
    expect(unknown).toEqual({
      keyType: 'ssh-ed25519',
      fingerprint,
      knownHostsPath: `${home}/.ssh/known_hosts`,
    });

    const trusted = await runCommand(browser, 'trust_sftp_host_key', {
      hostname: '127.0.0.1',
      port,
      fingerprint,
    });
    expect(trusted).toBeNull();
    // russh writes a line feed before the first key of a new file.
    const knownHosts = (await fs.readFile(path.join(home, '.ssh', 'known_hosts'), 'utf8'))
      .split('\n')
      .filter((line) => line.trim() !== '');
    expect(knownHosts).toHaveLength(1);
    expect(knownHosts[0].startsWith(`[127.0.0.1]:${port} ssh-ed25519 `)).toBe(true);

    const listing = await runCommand(browser, 'read_directory', { path: remote });
    expect(listing).toMatchObject({
      entries: [expect.objectContaining({ name: 'hello.txt', is_directory: false, size: 16 })],
    });

    const downloaded = await runCommand(browser, 'download_sftp_file', {
      path: `${remote}/hello.txt`,
    });
    expect(await fs.readFile(String(downloaded), 'utf8')).toBe('hello over sftp\n');

    const removed = await runCommand(browser, 'remove_sftp_server', {
      hostname: '127.0.0.1',
      port,
    });
    expect(removed).toBeNull();
    expect(await runCommand(browser, 'get_sftp_servers')).toEqual([]);
  }
);
