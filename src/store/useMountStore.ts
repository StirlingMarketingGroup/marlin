import { invoke } from '@tauri-apps/api/core';
import { listen, type Event } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { MountState, MountStatus, UnsentFile, UnsentFiles } from '../types';

export function mountKey(provider: string, account: string): string {
  return `${provider}:${account}`;
}

interface MountStore {
  /** The latest status of each mount the helper has, by `mountKey`. Empty until the window first lists, and after the helper closes. */
  mounts: Record<string, MountStatus>;
  /** Each mount's unsent files, by `mountKey`, while the helper's list is known. A key with no entry is not known: the window has not listed it, or the mount takes no writes. */
  unsent: Record<string, UnsentFiles>;
}

export const useMountStore = create<MountStore>(() => ({ mounts: {}, unsent: {} }));

// Listing starts a helper when none runs. A helper that will not start, or keeps closing, is listed again at waits that double up to a minute. One that stayed up longer than that is listed again after the first wait. On Windows every command fails at once and starts nothing (#375).
const FIRST_RETRY_MS = 1000;
const LAST_RETRY_MS = 60_000;

// A mount in these states has no unsent list: the helper answers null.
const NO_UNSENT: ReadonlySet<MountState> = new Set(['starting', 'failed', 'unmounted']);

// Whether the window may start a helper by listing: the last `listNow` given to `followMounts`.
let wanted = false;
// The three `listen` calls, made once, before any request.
let listening: Promise<unknown> | null = null;
// The connection's state: `listed` once a `list_mounts` answer came over it, `listing` while that request runs.
let listed = false;
let listing: Promise<void> | null = null;
// The timer that lists again after a failed list or a closed connection, its next wait, and when the last list answered.
let retry: ReturnType<typeof setTimeout> | null = null;
let retryDelay = FIRST_RETRY_MS;
let listedAt = 0;
// Every request and notice takes the next number. An answer to a request sent before the last `mount-helper-closed` came over the old connection. Per mount, `mountHeard` and `unsentHeard` hold the number of the newest word applied to its status and to its unsent list: notices arrive in order, but an answer can land after a newer notice or a newer answer.
let seq = 0;
let closedAt = 0;
const mountHeard = new Map<string, number>();
const unsentHeard = new Map<string, number>();

/**
 * Registers the helper's three events, then lists the mounts when `listNow` and nothing has listed them over the open connection. The Sidebar passes whether the user has a saved account. With `false` the window sends no command, so it starts no helper. An event comes only over an open connection, so the first one makes the window list then, which starts nothing.
 */
export async function followMounts(listNow: boolean): Promise<void> {
  wanted = listNow;
  listening ??= Promise.all([
    listen<MountStatus>('mount-changed', onMountChanged),
    listen<UnsentFiles>('unsent-changed', onUnsentChanged),
    listen('mount-helper-closed', onHelperClosed),
  ]);
  await listening;
  if (wanted) await listMounts();
}

/** Sends one unsent file again, or drops its changes, then lists the mount's files again. Errors reach the caller unchanged. */
export async function retryOrDiscardUnsent(
  action: 'retry' | 'discard',
  files: UnsentFiles,
  file: UnsentFile
): Promise<void> {
  try {
    await invoke(action === 'retry' ? 'retry_unsent_file' : 'discard_unsent_file', {
      provider: files.provider,
      account: files.account,
      mountId: files.mountId,
      node: file.node,
    });
  } finally {
    // The list updates at once. After a -1015 it shows the helper's current list.
    void listUnsent(files.provider, files.account);
  }
}

/** Lists the mounts, unless an answer already came over the open connection. Two triggers send one request. */
function listMounts(): Promise<void> {
  if (listed) return Promise.resolve();
  if (listing === null) {
    const run = requestMounts().finally(() => {
      if (listing === run) listing = null;
    });
    listing = run;
  }
  return listing;
}

function scheduleList(): void {
  if (!wanted) return;
  if (retry !== null) clearTimeout(retry);
  retry = setTimeout(() => {
    retry = null;
    if (wanted) void listMounts();
  }, retryDelay);
  retryDelay = Math.min(retryDelay * 2, LAST_RETRY_MS);
}

async function requestMounts(): Promise<void> {
  const asked = ++seq;
  let statuses: MountStatus[];
  try {
    statuses = await invoke<MountStatus[]>('list_mounts');
  } catch (error) {
    console.warn('Could not list the mounts:', error);
    if (asked > closedAt) scheduleList();
    return;
  }
  if (asked < closedAt) return;
  listed = true;
  listedAt = Date.now();
  const listedKeys = new Set(statuses.map((status) => mountKey(status.provider, status.account)));
  useMountStore.setState((state) => {
    const mounts = { ...state.mounts };
    const unsent = { ...state.unsent };
    // A mount the list leaves out is gone, with its unsent list, unless newer word about it came after the request.
    for (const key of Object.keys(mounts)) {
      if (!listedKeys.has(key) && asked > (mountHeard.get(key) ?? 0)) delete mounts[key];
    }
    for (const key of Object.keys(unsent)) {
      if (!listedKeys.has(key) && asked > (unsentHeard.get(key) ?? 0)) delete unsent[key];
    }
    for (const status of statuses) {
      const key = mountKey(status.provider, status.account);
      if (asked > (mountHeard.get(key) ?? 0)) mounts[key] = status;
    }
    return { mounts, unsent };
  });
  for (const status of Object.values(useMountStore.getState().mounts)) {
    if (!NO_UNSENT.has(status.state)) void listUnsent(status.provider, status.account);
  }
}

async function listUnsent(provider: string, account: string): Promise<void> {
  const key = mountKey(provider, account);
  const asked = ++seq;
  let files: UnsentFiles | null;
  try {
    files = await invoke<UnsentFiles | null>('list_unsent_files', { provider, account });
  } catch (error) {
    console.warn('Could not list the unsent files:', error);
    return;
  }
  if (asked < closedAt || asked < (unsentHeard.get(key) ?? 0)) return;
  unsentHeard.set(key, asked);
  useMountStore.setState((state) => {
    const unsent = { ...state.unsent };
    if (files) unsent[key] = files;
    else delete unsent[key];
    return { unsent };
  });
}

function onMountChanged({ payload: status }: Event<MountStatus>): void {
  const key = mountKey(status.provider, status.account);
  const heard = ++seq;
  mountHeard.set(key, heard);
  // The old list's mount id is gone. A mount in any other state is asked again, since one that came up has a new list.
  const gone = NO_UNSENT.has(status.state);
  if (gone) unsentHeard.set(key, heard);
  useMountStore.setState((state) => {
    const mounts = { ...state.mounts, [key]: status };
    if (!gone) return { mounts };
    const unsent = { ...state.unsent };
    delete unsent[key];
    return { mounts, unsent };
  });
  if (!gone && listed) void listUnsent(status.provider, status.account);
  void listMounts();
}

function onUnsentChanged({ payload: files }: Event<UnsentFiles>): void {
  const key = mountKey(files.provider, files.account);
  unsentHeard.set(key, ++seq);
  useMountStore.setState((state) => ({ unsent: { ...state.unsent, [key]: files } }));
  void listMounts();
}

function onHelperClosed(): void {
  // Only a connection whose list answered can have stayed up. One that closed before that is a helper that dies while it starts.
  if (listed && Date.now() - listedAt > LAST_RETRY_MS) retryDelay = FIRST_RETRY_MS;
  closedAt = ++seq;
  listed = false;
  listing = null;
  mountHeard.clear();
  unsentHeard.clear();
  useMountStore.setState({ mounts: {}, unsent: {} });
  scheduleList();
}
