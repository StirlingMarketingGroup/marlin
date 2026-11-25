import { beforeEach, describe, expect, it, vi } from 'vitest';
import { SIGN_IN_CANCELLED, withServerSignInCheck } from './serverSignIn';

describe('withServerSignInCheck', () => {
  const waiting = vi.fn<(isWaiting: boolean) => void>();
  const show = vi.fn(() => Promise.resolve());
  beforeEach(() => {
    waiting.mockClear();
    show.mockClear();
  });

  it('never opens the window when the read succeeds', async () => {
    const attempt = vi.fn(() => Promise.resolve('listing'));
    const openConnectWindow = vi.fn(() => Promise.resolve('sftp://u@h/'));

    await expect(
      withServerSignInCheck('sftp://h:22', { open: openConnectWindow, show }, attempt, waiting)
    ).resolves.toBe('listing');

    expect(attempt).toHaveBeenCalledTimes(1);
    expect(openConnectWindow).not.toHaveBeenCalled();
    expect(waiting).not.toHaveBeenCalled();
    expect(show).not.toHaveBeenCalled();
  });

  it('rethrows another error and opens no window', async () => {
    const failure = new Error('Could not connect');
    const attempt = vi.fn(() => Promise.reject(failure));
    const openConnectWindow = vi.fn(() => Promise.resolve('sftp://u@h/'));

    await expect(
      withServerSignInCheck('sftp://h:22', { open: openConnectWindow, show }, attempt, waiting)
    ).rejects.toBe(failure);

    expect(attempt).toHaveBeenCalledTimes(1);
    expect(openConnectWindow).not.toHaveBeenCalled();
  });

  it('opens the window for a missing sign-in and runs the read again', async () => {
    let reads = 0;
    const attempt = vi.fn(() => {
      reads += 1;
      if (reads === 1) {
        return Promise.reject(new Error('[SFTP_NO_CREDENTIALS] 10.5.0.247:22'));
      }
      return Promise.resolve('listing');
    });
    const openConnectWindow = vi.fn(() => Promise.resolve('sftp://admin@10.5.0.247/'));

    await expect(
      withServerSignInCheck('sftp://h:22', { open: openConnectWindow, show }, attempt, waiting)
    ).resolves.toBe('listing');

    expect(openConnectWindow).toHaveBeenCalledTimes(1);
    expect(attempt).toHaveBeenCalledTimes(2);
  });

  it('throws when the window closes and runs the read once only', async () => {
    const attempt = vi.fn(() => Promise.reject(new Error('[SMB_NO_CREDENTIALS] server')));
    const openConnectWindow = vi.fn(() => Promise.resolve(null));

    await expect(
      withServerSignInCheck('smb://server', { open: openConnectWindow, show }, attempt, waiting)
    ).rejects.toThrow(SIGN_IN_CANCELLED);

    expect(attempt).toHaveBeenCalledTimes(1);
    expect(openConnectWindow).toHaveBeenCalledTimes(1);
  });

  it('opens one window when two reads ask for one server at once, and both run again', async () => {
    let release: (address: string | null) => void = () => {};
    const openConnectWindow = vi.fn(
      () =>
        new Promise<string | null>((resolve) => {
          release = resolve;
        })
    );
    let reads = 0;
    const attempt = vi.fn(() => {
      reads += 1;
      if (reads <= 2) {
        return Promise.reject(new Error('[SFTP_NO_CREDENTIALS] host'));
      }
      return Promise.resolve('listing');
    });

    const pending = Promise.all([
      withServerSignInCheck('sftp://h:22', { open: openConnectWindow, show }, attempt, waiting),
      withServerSignInCheck('sftp://h:22', { open: openConnectWindow, show }, attempt, waiting),
    ]);
    await vi.waitFor(() => {
      expect(openConnectWindow).toHaveBeenCalledTimes(1);
      // The read that joined the open window waits too, and brings the window to the front.
      expect(waiting.mock.calls).toEqual([[true], [true]]);
      expect(show).toHaveBeenCalledTimes(1);
    });
    release('sftp://u@h/');

    await expect(pending).resolves.toEqual(['listing', 'listing']);
    expect(attempt).toHaveBeenCalledTimes(4);
    expect(waiting.mock.calls).toEqual([[true], [true], [false], [false]]);
  });

  it('opens a new window for a later read after the first settled', async () => {
    const openConnectWindow = vi
      .fn<() => Promise<string | null>>()
      .mockResolvedValueOnce('sftp://u@h/')
      .mockResolvedValueOnce('sftp://u@h/');
    let reads = 0;
    const attempt = vi.fn(() => {
      reads += 1;
      if (reads === 1 || reads === 3) {
        return Promise.reject(new Error('[SFTP_NO_CREDENTIALS] host'));
      }
      return Promise.resolve('listing');
    });

    await expect(
      withServerSignInCheck('sftp://h:22', { open: openConnectWindow, show }, attempt, waiting)
    ).resolves.toBe('listing');
    await expect(
      withServerSignInCheck('sftp://h:22', { open: openConnectWindow, show }, attempt, waiting)
    ).resolves.toBe('listing');

    expect(openConnectWindow).toHaveBeenCalledTimes(2);
  });

  it('returns the error when the window fails to open', async () => {
    const openError = new Error('Failed to create SFTP connect window: test');
    const attempt = vi.fn(() => Promise.reject(new Error('[SFTP_NO_CREDENTIALS] host')));
    const openConnectWindow = vi.fn(() => Promise.reject(openError));

    await expect(
      withServerSignInCheck('sftp://h:22', { open: openConnectWindow, show }, attempt, waiting)
    ).rejects.toBe(openError);

    expect(attempt).toHaveBeenCalledTimes(1);
  });
});
