import { invoke } from '@tauri-apps/api/core';
import { ask } from '@tauri-apps/plugin-dialog';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { withSftpHostKeyCheck } from './sftpHostKey';

vi.mock('@tauri-apps/api/core');
vi.mock('@tauri-apps/plugin-dialog');

const mockInvoke = vi.mocked(invoke);
const mockAsk = vi.mocked(ask);

const unknownError = new Error(
  '[SFTP_HOST_KEY_UNKNOWN] example.com:22 is not in /home/me/.ssh/known_hosts. Its ssh-ed25519 key fingerprint is SHA256:abc.'
);
const unknownKey = {
  keyType: 'ssh-ed25519',
  fingerprint: 'SHA256:abc',
  knownHostsPath: '/home/me/.ssh/known_hosts',
};

/** An SFTP call that fails on the host key until the key is trusted. */
function callThatNeedsTheKeyTrusted() {
  let trusted = false;
  mockInvoke.mockImplementation((cmd) => {
    if (cmd === 'get_sftp_unknown_host_key') {
      return Promise.resolve(unknownKey);
    }
    if (cmd === 'trust_sftp_host_key') {
      trusted = true;
    }
    return Promise.resolve(undefined);
  });
  return vi.fn(() => (trusted ? Promise.resolve('listing') : Promise.reject(unknownError)));
}

describe('withSftpHostKeyCheck', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('passes through any other error, a changed key included', async () => {
    const failure = new Error('The key for example.com does not match the one saved in …');
    const attempt = vi.fn(() => Promise.reject(failure));

    await expect(withSftpHostKeyCheck('example.com', 22, attempt)).rejects.toBe(failure);

    expect(attempt).toHaveBeenCalledTimes(1);
    expect(mockInvoke).not.toHaveBeenCalled();
    expect(mockAsk).not.toHaveBeenCalled();
  });

  it('saves the key the user trusts and runs the call again', async () => {
    const attempt = callThatNeedsTheKeyTrusted();
    mockAsk.mockResolvedValue(true);

    await expect(withSftpHostKeyCheck('example.com', 22, attempt)).resolves.toBe('listing');

    expect(mockAsk.mock.calls[0][0]).toContain('example.com has no saved key in /home/me');
    expect(mockAsk.mock.calls[0][0]).toContain('SHA256:abc');
    expect(mockInvoke).toHaveBeenCalledWith('trust_sftp_host_key', {
      hostname: 'example.com',
      port: 22,
      fingerprint: 'SHA256:abc',
    });
    expect(attempt).toHaveBeenCalledTimes(2);
  });

  it('asks once when two calls fail on the same server key', async () => {
    const attempt = callThatNeedsTheKeyTrusted();
    mockAsk.mockResolvedValue(true);

    const results = await Promise.all([
      withSftpHostKeyCheck('example.com', 22, attempt),
      withSftpHostKeyCheck('example.com', 22, attempt),
    ]);

    expect(results).toEqual(['listing', 'listing']);
    expect(mockAsk).toHaveBeenCalledTimes(1);
    expect(mockInvoke.mock.calls.filter(([cmd]) => cmd === 'trust_sftp_host_key')).toHaveLength(1);
  });

  it('fails without the bracket tag and saves nothing when the user cancels', async () => {
    const attempt = callThatNeedsTheKeyTrusted();
    mockAsk.mockResolvedValue(false);

    await expect(withSftpHostKeyCheck('example.com', 22, attempt)).rejects.toThrow(
      /^example\.com:22 is not in /
    );

    expect(attempt).toHaveBeenCalledTimes(1);
    expect(mockInvoke).not.toHaveBeenCalledWith('trust_sftp_host_key', expect.anything());
  });
});
