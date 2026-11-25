export const SIGN_IN_CANCELLED = 'Sign-in was cancelled.';

const NO_CREDENTIALS = /\[(SMB|SFTP)_NO_CREDENTIALS\]/;

// Marlin reads a remote folder from two places at once (issue #499), and the user can ask again
// while the window waits, so two reads can ask for the same server. They share one connect
// window and its answer.
const signIns = new Map<string, Promise<string | null>>();

/** The SMB or SFTP connect window, as a read that needs a sign-in uses it. */
export interface ConnectWindow {
  /** Opens the window with a new request. Answers the address the user connected to, or null. */
  open: () => Promise<string | null>;
  /** Brings the open window to the front. */
  show: () => Promise<unknown>;
}

/**
 * Runs a read of an SMB or SFTP folder. When no sign-in is saved for its server, opens the
 * connect window, waits for the user, and runs the read again once they connect. Every read
 * that waits, not only the one that opened the window, tells `waiting` when it starts and stops.
 */
export async function withServerSignInCheck<T>(
  server: string,
  connectWindow: ConnectWindow,
  attempt: () => Promise<T>,
  waiting: (isWaiting: boolean) => void
): Promise<T> {
  try {
    return await attempt();
  } catch (err) {
    const text = err instanceof Error ? err.message : String(err);
    if (!NO_CREDENTIALS.test(text)) {
      throw err;
    }

    waiting(true);
    let answer = signIns.get(server);
    if (answer) {
      // The window is not modal, so it can be behind the main window when the user asks again.
      connectWindow.show().catch((showErr) => {
        console.warn('Failed to show the connect window:', showErr);
      });
    } else {
      answer = connectWindow.open().finally(() => signIns.delete(server));
      signIns.set(server, answer);
    }
    if ((await answer.finally(() => waiting(false))) === null) {
      throw new Error(SIGN_IN_CANCELLED);
    }
    return attempt();
  }
}
