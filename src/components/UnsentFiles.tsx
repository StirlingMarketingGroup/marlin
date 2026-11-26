import { useEffect, useLayoutEffect, useRef, useState, type MouseEvent } from 'react';
import { createPortal } from 'react-dom';
import { ask } from '@tauri-apps/plugin-dialog';
import { CloudArrowUp, Warning } from 'phosphor-react';
import { useAppStore } from '@/store/useAppStore';
import { mountKey, retryOrDiscardUnsent, useMountStore } from '@/store/useMountStore';
import { useToastStore } from '@/store/useToastStore';
import type { MountCommandError, UnsentFile } from '@/types';
import { formatBytes } from '@/utils/formatBytes';
import { dirname } from '@/utils/pathUtils';

function folderOf(path: string): string {
  return path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '';
}

export default function UnsentFiles({
  provider,
  account,
  label,
}: {
  provider: string;
  account: string;
  label: string;
}) {
  const key = mountKey(provider, account);
  const files = useMountStore((state) => state.unsent[key]);
  const mountPoint = useMountStore((state) => state.mounts[key]?.mountPoint);
  const [place, setPlace] = useState<{ left: number; top: number } | null>(null);
  const [busy, setBusy] = useState<ReadonlySet<string>>(new Set());
  const markRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  // The popover's drawn size, so it can move to stay inside the window. Measured before paint whenever it opens or its list changes.
  const [size, setSize] = useState({ width: 0, height: 0 });
  useLayoutEffect(() => {
    setSize({
      width: popoverRef.current?.offsetWidth ?? 0,
      height: popoverRef.current?.offsetHeight ?? 0,
    });
  }, [place, files]);

  const count = files ? files.files.length + files.unplaced.length : 0;
  const needsUser = files
    ? files.files.some((file) => file.state === 'failed' || file.state === 'refused') ||
      files.unplaced.length > 0
    : false;

  useEffect(() => {
    if (place === null) return;
    const close = () => setPlace(null);
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') close();
    };
    const onMouseDown = (event: globalThis.MouseEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (popoverRef.current?.contains(target) || markRef.current?.contains(target)) return;
      close();
    };
    document.addEventListener('keydown', onKey);
    document.addEventListener('mousedown', onMouseDown);
    window.addEventListener('resize', close);
    return () => {
      document.removeEventListener('keydown', onKey);
      document.removeEventListener('mousedown', onMouseDown);
      window.removeEventListener('resize', close);
    };
  }, [place]);

  if (count === 0 && place === null) return null;

  const title = needsUser
    ? 'Some files could not upload'
    : `${count} ${count === 1 ? 'file' : 'files'} not uploaded yet`;

  function toggle(event: MouseEvent<HTMLButtonElement>) {
    event.stopPropagation();
    if (place) {
      setPlace(null);
      return;
    }
    const rect = event.currentTarget.getBoundingClientRect();
    const sidebar = event.currentTarget.closest('[data-sidebar]');
    const left = (sidebar?.getBoundingClientRect().right ?? rect.right) + 6;
    setPlace({ left, top: rect.top - 4 });
  }

  async function send(action: 'retry' | 'discard', file: UnsentFile) {
    if (!files) return;
    const id = `${files.mountId}:${file.node}`;
    setBusy((current) => new Set(current).add(id));
    try {
      await retryOrDiscardUnsent(action, files, file);
    } catch (error) {
      const { code, message } = error as MountCommandError;
      const { addToast } = useToastStore.getState();
      if (code === -1015) {
        addToast({
          type: 'info',
          message: `The list changed before Marlin could ${action} “${file.name}”. Check it again.`,
        });
      } else if (code === -1016) {
        addToast({
          type: 'error',
          message: `Marlin could not discard “${file.name}” in time. Try again.`,
        });
      } else {
        addToast({ type: 'error', message: `Could not ${action} “${file.name}”: ${message}` });
      }
    } finally {
      setBusy((current) => {
        const next = new Set(current);
        next.delete(id);
        return next;
      });
    }
  }

  async function onDiscard(file: UnsentFile) {
    const where = file.path === undefined ? '' : `It is in ${folderOf(file.path) || label}. `;
    let confirmed = false;
    try {
      confirmed = await ask(
        `${where}Marlin drops the changes that have not been uploaded. A new file goes away, and any other file goes back to the copy that was uploaded before. You can’t undo this action.`,
        {
          title: `Discard the changes to “${file.name}”?`,
          kind: 'warning',
          okLabel: 'Discard',
          cancelLabel: 'Cancel',
        }
      );
    } catch (dialogError) {
      console.warn('Failed to show the discard question:', dialogError);
    }
    if (confirmed) await send('discard', file);
  }

  // How many listed files have each name. Discard drops a change for good, so a file with no folder must not be mistaken for another of its name.
  const named = new Map<string, number>();
  for (const file of files?.files ?? []) named.set(file.name, (named.get(file.name) ?? 0) + 1);

  const left =
    place === null ? 0 : Math.max(12, Math.min(place.left, window.innerWidth - 12 - size.width));
  const top =
    place === null ? 0 : Math.max(12, Math.min(place.top, window.innerHeight - 12 - size.height));

  return (
    <>
      {count > 0 && (
        <button
          type="button"
          ref={markRef}
          onClick={toggle}
          className="flex flex-shrink-0 items-center gap-0.5 px-1 rounded text-[11px] leading-4 text-app-muted hover:bg-app-light/50 hover:text-app-text transition-colors"
          title={title}
          aria-label={title}
          data-tauri-drag-region={false}
          data-testid="unsent-mark"
        >
          {needsUser ? (
            <Warning className="w-3.5 h-3.5 text-app-yellow" weight="fill" />
          ) : (
            <CloudArrowUp className="w-3.5 h-3.5" weight="regular" />
          )}
          <span>{count}</span>
        </button>
      )}
      {place &&
        createPortal(
          <div
            ref={popoverRef}
            role="dialog"
            aria-label={`Files not uploaded to ${label}`}
            data-testid="unsent-files"
            className="fixed z-50 w-80 flex flex-col rounded-md border border-app-border bg-[rgb(var(--bg-rgb))] shadow-lg text-app-text"
            style={{
              left,
              top,
              maxWidth: window.innerWidth - 24,
              maxHeight: window.innerHeight - 24,
            }}
          >
            <div className="px-3 pt-2 pb-1">
              <div className="text-[13px] font-medium">Not uploaded yet</div>
              <div className="text-xs text-app-muted truncate">{label}</div>
            </div>
            <div className="min-h-0 overflow-y-auto">
              {!files && (
                <div className="px-3 py-2 text-xs text-app-muted">
                  Marlin cannot see this account’s unsent files right now.
                </div>
              )}
              {files && count === 0 && (
                <div className="px-3 py-2 text-xs text-app-muted">
                  No files are waiting to upload.
                </div>
              )}
              {files && files.files.length > 0 && (
                <ul className="py-1">
                  {files.files.map((file) => {
                    const folder = file.path === undefined ? undefined : folderOf(file.path);
                    const id = `${files.mountId}:${file.node}`;
                    const running = busy.has(id);
                    const ambiguous = file.path === undefined && (named.get(file.name) ?? 0) > 1;
                    const percent =
                      file.state === 'uploading'
                        ? file.size > 0
                          ? Math.min(100, Math.floor((file.sent * 100) / file.size))
                          : 100
                        : 0;
                    return (
                      <li key={id} className="px-3 py-1.5" data-testid="unsent-file">
                        <div className="flex items-baseline gap-2">
                          <span className="flex-1 min-w-0 truncate text-[13px]" title={file.name}>
                            {file.name}
                          </span>
                          <span className="flex-shrink-0 text-xs text-app-muted">
                            {formatBytes(file.size)}
                          </span>
                        </div>
                        {file.path !== undefined &&
                          (mountPoint ? (
                            <button
                              type="button"
                              className="block max-w-full truncate text-left text-xs text-app-muted hover:text-app-text hover:underline"
                              title="Show in Marlin"
                              onClick={() => {
                                void useAppStore
                                  .getState()
                                  .navigateTo(folder ? `${mountPoint}/${folder}` : mountPoint, {
                                    reveal: `${mountPoint}/${file.path}`,
                                  });
                                setPlace(null);
                              }}
                            >
                              {folder || label}
                            </button>
                          ) : (
                            <div className="truncate text-xs text-app-muted">{folder || label}</div>
                          ))}
                        <div className="text-xs text-app-muted">
                          {file.state === 'open' && 'Being written'}
                          {file.state === 'waiting' && 'Waiting to upload'}
                          {file.state === 'uploading' && `Uploading, ${percent}%`}
                          {file.state === 'failed' &&
                            'Upload failed. Marlin tries again by itself.'}
                          {file.state === 'refused' &&
                            'Upload refused. Retry it, or discard the changes.'}
                        </div>
                        {file.state === 'uploading' && (
                          <div className="mt-1 h-1 rounded bg-app-light">
                            <div
                              className="h-1 rounded bg-accent"
                              style={{ width: `${percent}%` }}
                            />
                          </div>
                        )}
                        {(file.state === 'failed' || file.state === 'refused') && (
                          <>
                            <div className="text-xs break-words">{file.error}</div>
                            {ambiguous && (
                              <div className="text-xs text-app-muted">
                                Another file here has this name. Marlin cannot show this one’s
                                folder, so Discard is off until a program opens that folder.
                              </div>
                            )}
                            <div className="mt-1 flex gap-2">
                              <button
                                type="button"
                                className="px-2 py-0.5 rounded border border-app-border text-xs hover:bg-app-light disabled:opacity-50"
                                disabled={running}
                                onClick={() => void send('retry', file)}
                              >
                                Retry
                              </button>
                              <button
                                type="button"
                                className="px-2 py-0.5 rounded border border-app-border text-xs hover:bg-app-light disabled:opacity-50 text-app-red"
                                disabled={running || ambiguous}
                                onClick={() => void onDiscard(file)}
                              >
                                Discard
                              </button>
                            </div>
                          </>
                        )}
                      </li>
                    );
                  })}
                </ul>
              )}
              {files && files.unplaced.length > 0 && (
                <div className="border-t border-app-border px-3 py-2">
                  <div className="text-xs text-app-muted">
                    Kept on this computer from an earlier run. Marlin does not know where they go,
                    so they never upload.
                  </div>
                  <ul>
                    {files.unplaced.map((path) => (
                      <li key={path} className="mt-1 flex items-center gap-2">
                        <span className="flex-1 min-w-0 truncate text-xs select-text" title={path}>
                          {path}
                        </span>
                        <button
                          type="button"
                          className="px-2 py-0.5 rounded border border-app-border text-xs hover:bg-app-light"
                          onClick={() => {
                            void useAppStore.getState().navigateTo(dirname(path), { reveal: path });
                            setPlace(null);
                          }}
                        >
                          Show
                        </button>
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </div>
          </div>,
          document.body
        )}
    </>
  );
}
