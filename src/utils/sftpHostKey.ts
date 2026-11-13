import { invoke } from '@tauri-apps/api/core';
import { ask } from '@tauri-apps/plugin-dialog';
import type { SftpUnknownHostKey } from '@/types';

const UNKNOWN_HOST_KEY_ERROR = /\[SFTP_HOST_KEY_UNKNOWN\] /;

// Marlin reads a remote folder from two places at once (issue #499), so two calls can fail on
// the same server key. They share one answer.
const answers = new Map<string, Promise<boolean>>();

/**
 * Runs an SFTP call. When the server has no saved key, asks the user once, and runs the call
 * again if they trust the key.
 */
export async function withSftpHostKeyCheck<T>(
  hostname: string,
  port: number,
  attempt: () => Promise<T>
): Promise<T> {
  try {
    return await attempt();
  } catch (err) {
    const text = err instanceof Error ? err.message : String(err);
    if (!UNKNOWN_HOST_KEY_ERROR.test(text)) {
      throw err;
    }

    const server = `${hostname.toLowerCase()}:${port}`;
    let answer = answers.get(server);
    if (!answer) {
      answer = askToTrustUnknownHostKey(hostname, port).finally(() => answers.delete(server));
      answers.set(server, answer);
    }
    if (!(await answer)) {
      throw new Error(text.replace(UNKNOWN_HOST_KEY_ERROR, ''));
    }
    return attempt();
  }
}

/** True when the user trusted the key and Marlin saved it. */
async function askToTrustUnknownHostKey(hostname: string, port: number): Promise<boolean> {
  const unknown = await invoke<SftpUnknownHostKey | null>('get_sftp_unknown_host_key', {
    hostname,
    port,
  });
  if (!unknown) {
    return false;
  }

  const host = port === 22 ? hostname : `${hostname}:${port}`;
  const trusted = await ask(
    `${host} has no saved key in ${unknown.knownHostsPath}.

The server's ${unknown.keyType} key fingerprint is:
${unknown.fingerprint}

Trust this server only if the fingerprint matches the one its owner gave you. Marlin saves the key in that file and checks it on every connection.`,
    {
      title: 'Unknown SFTP Server',
      kind: 'warning',
      okLabel: 'Trust and Connect',
      cancelLabel: 'Cancel',
    }
  );
  if (!trusted) {
    return false;
  }
  await invoke('trust_sftp_host_key', { hostname, port, fingerprint: unknown.fingerprint });
  return true;
}
