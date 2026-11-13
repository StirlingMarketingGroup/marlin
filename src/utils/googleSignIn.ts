import { ask } from '@tauri-apps/plugin-dialog';
import type { GoogleAccountInfo } from '@/types';
import { useToastStore } from '@/store/useToastStore';

export const GOOGLE_SIGN_IN_CANCELLED = 'Google sign-in was cancelled.';

const SIGN_IN_NEEDED = '[GDRIVE_SIGN_IN_NEEDED] ';

// Marlin reads a remote folder from two places at once (issue #499), so two calls can fail for
// the same account. They share one prompt and one sign-in.
const signIns = new Map<string, Promise<void>>();

/**
 * Runs a Google Drive call. When Google no longer accepts the account's saved sign-in, asks the
 * user once, signs in again, and runs the call again.
 */
export async function withGoogleSignInCheck<T>(
  email: string,
  signIn: (email: string) => Promise<GoogleAccountInfo>,
  attempt: () => Promise<T>
): Promise<T> {
  try {
    return await attempt();
  } catch (err) {
    const text = err instanceof Error ? err.message : String(err);
    if (!text.includes(SIGN_IN_NEEDED)) {
      throw err;
    }

    let pending = signIns.get(email);
    if (!pending) {
      pending = signInAgain(email, signIn).finally(() => signIns.delete(email));
      signIns.set(email, pending);
    }
    await pending;
    return attempt();
  }
}

/** Asks the user, signs in to `email`, and shows that it worked. */
async function signInAgain(
  email: string,
  signIn: (email: string) => Promise<GoogleAccountInfo>
): Promise<void> {
  const accepted = await ask(
    `Google no longer accepts the saved sign-in for ${email}.\n\nSign in again to keep using this account in Marlin. Your browser will open.`,
    {
      title: 'Sign In to Google Again',
      kind: 'warning',
      okLabel: 'Sign In',
      cancelLabel: 'Cancel',
    }
  );
  if (!accepted) {
    throw new Error(GOOGLE_SIGN_IN_CANCELLED);
  }

  const account = await signIn(email);
  if (account.email !== email) {
    throw new Error(`You signed in as ${account.email}. Sign in as ${email} to open this folder.`);
  }

  useToastStore.getState().addToast({
    type: 'success',
    message: `Signed in to ${email}`,
    duration: 3000,
  });
}
