import { ask } from '@tauri-apps/plugin-dialog';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { GOOGLE_SIGN_IN_CANCELLED, withGoogleSignInCheck } from './googleSignIn';

vi.mock('@tauri-apps/plugin-dialog');

const mockAsk = vi.mocked(ask);

const email = 'a@example.com';
const needed = new Error(
  '[GDRIVE_SIGN_IN_NEEDED] Google no longer accepts the saved sign-in for a@example.com.'
);

/** A Google Drive call that fails on the saved sign-in until the caller signs in. */
function attemptThatFailsUntilSignedIn(result: string) {
  let signedIn = false;
  return vi.fn(() => {
    if (!signedIn) {
      signedIn = true;
      return Promise.reject(needed);
    }
    return Promise.resolve(result);
  });
}

describe('withGoogleSignInCheck', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('passes through any other error', async () => {
    const failure = new Error('Token refresh failed (status 400 Bad Request).');
    const attempt = vi.fn(() => Promise.reject(failure));
    const signIn = vi.fn();

    await expect(withGoogleSignInCheck(email, signIn, attempt)).rejects.toBe(failure);

    expect(attempt).toHaveBeenCalledTimes(1);
    expect(mockAsk).not.toHaveBeenCalled();
    expect(signIn).not.toHaveBeenCalled();
  });

  it('signs in and runs the call again', async () => {
    const attempt = attemptThatFailsUntilSignedIn('listing');
    const signIn = vi.fn(() => Promise.resolve({ email }));
    mockAsk.mockResolvedValue(true);

    await expect(withGoogleSignInCheck(email, signIn, attempt)).resolves.toBe('listing');

    expect(mockAsk).toHaveBeenCalledTimes(1);
    expect(mockAsk).toHaveBeenCalledWith(
      `Google no longer accepts the saved sign-in for ${email}.\n\nSign in again to keep using this account in Marlin. Your browser will open.`,
      {
        title: 'Sign In to Google Again',
        kind: 'warning',
        okLabel: 'Sign In',
        cancelLabel: 'Cancel',
      }
    );
    expect(signIn).toHaveBeenCalledTimes(1);
    expect(signIn).toHaveBeenCalledWith(email);
    expect(attempt).toHaveBeenCalledTimes(2);
  });

  it('rejects with the cancelled message and does not sign in', async () => {
    const attempt = attemptThatFailsUntilSignedIn('listing');
    const signIn = vi.fn();
    mockAsk.mockResolvedValue(false);

    await expect(withGoogleSignInCheck(email, signIn, attempt)).rejects.toThrow(
      GOOGLE_SIGN_IN_CANCELLED
    );

    expect(signIn).not.toHaveBeenCalled();
    expect(attempt).toHaveBeenCalledTimes(1);
  });

  it('rejects when the user signs in as a different account', async () => {
    const attempt = attemptThatFailsUntilSignedIn('listing');
    const signIn = vi.fn(() => Promise.resolve({ email: 'other@example.com' }));
    mockAsk.mockResolvedValue(true);

    await expect(withGoogleSignInCheck(email, signIn, attempt)).rejects.toThrow(
      'You signed in as other@example.com. Sign in as a@example.com to open this folder.'
    );

    expect(attempt).toHaveBeenCalledTimes(1);
  });

  it('asks once when two calls fail for the same account', async () => {
    const first = attemptThatFailsUntilSignedIn('one');
    const second = attemptThatFailsUntilSignedIn('two');
    const signIn = vi.fn(() => Promise.resolve({ email }));
    mockAsk.mockResolvedValue(true);

    const results = await Promise.all([
      withGoogleSignInCheck(email, signIn, first),
      withGoogleSignInCheck(email, signIn, second),
    ]);

    expect(results).toEqual(['one', 'two']);
    expect(mockAsk).toHaveBeenCalledTimes(1);
    expect(signIn).toHaveBeenCalledTimes(1);
  });

  it('asks again after the first sign-in has settled', async () => {
    const signIn = vi.fn(() => Promise.resolve({ email }));
    mockAsk.mockResolvedValue(true);

    await withGoogleSignInCheck(email, signIn, attemptThatFailsUntilSignedIn('one'));
    await withGoogleSignInCheck(email, signIn, attemptThatFailsUntilSignedIn('two'));

    expect(mockAsk).toHaveBeenCalledTimes(2);
    expect(signIn).toHaveBeenCalledTimes(2);
  });
});
