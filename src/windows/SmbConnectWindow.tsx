import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { emit, listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { CircleNotch, ShareNetwork, CheckCircle, X } from 'phosphor-react';
import type { SmbConnectInitPayload, SmbConnectSuccessPayload, SmbServerInfo } from '@/types';

const CONTAINER_TOP_PAD = '3rem';
const SMB_CONNECT_INIT_EVENT = 'smb-connect:init';

export default function SmbConnectWindow() {
  const windowRef = getCurrentWindow();
  const hostnameInputRef = useRef<HTMLInputElement>(null);
  const usernameInputRef = useRef<HTMLInputElement>(null);

  const [initialHostname, setInitialHostname] = useState<string | undefined>();
  const [targetPath, setTargetPath] = useState<string | undefined>();

  const [hostname, setHostname] = useState('');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [domain, setDomain] = useState('');
  const [isConnecting, setIsConnecting] = useState(false);
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState<string | undefined>();

  const hostnameLocked = useMemo(() => Boolean(initialHostname), [initialHostname]);

  useEffect(() => {
    let unlistenInit: (() => void) | undefined;
    let readyNotified = false;

    (async () => {
      try {
        unlistenInit = await listen<SmbConnectInitPayload>(SMB_CONNECT_INIT_EVENT, (event) => {
          const nextHostname = event.payload?.initialHostname ?? undefined;
          const nextTargetPath = event.payload?.targetPath ?? undefined;

          setInitialHostname(nextHostname);
          setTargetPath(nextTargetPath);
          setHostname(nextHostname ?? '');
          setUsername('');
          setPassword('');
          setDomain('');
          setError(undefined);
          setIsConnecting(false);
          setConnected(false);
        });
      } catch (listenErr) {
        console.warn('Failed to listen for SMB connect init payload:', listenErr);
      }

      try {
        await invoke('smb_connect_window_ready');
        readyNotified = true;
      } catch (readyErr) {
        console.warn('Failed to notify SMB connect window readiness:', readyErr);
      }
    })();

    return () => {
      if (unlistenInit) {
        unlistenInit();
      }
      if (readyNotified) {
        void invoke('smb_connect_window_unready').catch((err) => {
          console.warn('Failed to reset SMB connect readiness:', err);
        });
      }
    };
  }, []);

  useEffect(() => {
    window.setTimeout(() => {
      if (hostnameLocked) {
        usernameInputRef.current?.focus();
      } else {
        hostnameInputRef.current?.focus();
      }
    }, 50);
  }, [hostnameLocked, initialHostname]);

  const closeWindow = useCallback(async () => {
    try {
      await windowRef.close();
    } catch (error) {
      console.warn('Failed to close SMB connect window:', error);
      void invoke('hide_smb_connect_window').catch((hideErr) => {
        console.warn('Failed to close SMB connect window (fallback):', hideErr);
      });
    }
  }, [windowRef]);

  const handleSubmit = useCallback(async () => {
    setError(undefined);

    if (!hostname.trim()) {
      setError('Server hostname is required.');
      return;
    }
    if (!username.trim()) {
      setError('Username is required.');
      return;
    }
    if (!password) {
      setError('Password is required.');
      return;
    }

    setIsConnecting(true);
    try {
      const server = await invoke<SmbServerInfo>('add_smb_server', {
        hostname: hostname.trim(),
        username: username.trim(),
        password,
        domain: domain.trim() || undefined,
      });

      const payload: SmbConnectSuccessPayload = {
        hostname: server.hostname,
        targetPath: targetPath || `smb://${server.hostname}/`,
      };

      setConnected(true);
      await emit('smb-connect:success', payload);

      window.setTimeout(() => {
        void closeWindow();
      }, 450);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      setIsConnecting(false);
    }
  }, [closeWindow, domain, hostname, password, targetPath, username]);

  const handleFormSubmit = useCallback(
    (e: FormEvent) => {
      e.preventDefault();
      void handleSubmit();
    },
    [handleSubmit]
  );

  const handleCancel = useCallback(() => {
    void closeWindow();
  }, [closeWindow]);

  return (
    <div className="min-h-screen bg-app-dark text-app-text">
      <div
        className="relative mx-auto flex h-full max-w-md flex-col gap-4 px-6 pb-8"
        style={{ paddingTop: CONTAINER_TOP_PAD }}
      >
        <div data-tauri-drag-region className="absolute inset-x-2 top-0 h-10 rounded-lg" />

        <header className="flex items-start gap-3">
          <div className="mt-0.5 flex h-10 w-10 items-center justify-center rounded-full bg-app-light/30 border border-app-border">
            <ShareNetwork className="h-5 w-5 text-accent" weight="fill" />
          </div>
          <div className="flex-1 min-w-0 space-y-1">
            <div className="font-medium text-sm">Add Network Share</div>
            <div className="text-xs text-app-muted">
              Enter credentials for an SMB server (smb://).
            </div>
          </div>
          <button
            type="button"
            onClick={handleCancel}
            className="p-1.5 rounded hover:bg-app-light/50 text-app-muted hover:text-app-text transition-colors"
            aria-label="Close"
            data-tauri-drag-region={false}
          >
            <X className="h-4 w-4" weight="bold" />
          </button>
        </header>

        <form className="space-y-3" onSubmit={handleFormSubmit}>
          <div className="space-y-1">
            <label htmlFor="smb-hostname" className="block text-xs text-app-muted">
              Server
            </label>
            <input
              ref={hostnameInputRef}
              id="smb-hostname"
              type="text"
              value={hostname}
              onChange={(e) => setHostname(e.target.value)}
              placeholder="server.local or 192.168.1.100"
              className="w-full px-3 py-2 text-sm bg-app-gray border border-app-border rounded-md focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent disabled:opacity-70"
              disabled={isConnecting || hostnameLocked}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
              data-tauri-drag-region={false}
            />
          </div>

          <div className="space-y-1">
            <label htmlFor="smb-username" className="block text-xs text-app-muted">
              Username
            </label>
            <input
              ref={usernameInputRef}
              id="smb-username"
              type="text"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              placeholder="user"
              className="w-full px-3 py-2 text-sm bg-app-gray border border-app-border rounded-md focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent disabled:opacity-70"
              disabled={isConnecting}
              autoComplete="username"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
              data-tauri-drag-region={false}
            />
          </div>

          <div className="space-y-1">
            <label htmlFor="smb-password" className="block text-xs text-app-muted">
              Password
            </label>
            <input
              id="smb-password"
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="••••••••"
              className="w-full px-3 py-2 text-sm bg-app-gray border border-app-border rounded-md focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent disabled:opacity-70"
              disabled={isConnecting}
              autoComplete="current-password"
              data-tauri-drag-region={false}
            />
          </div>

          <div className="space-y-1">
            <label htmlFor="smb-domain" className="block text-xs text-app-muted">
              Domain <span className="text-app-muted/60">(optional)</span>
            </label>
            <input
              id="smb-domain"
              type="text"
              value={domain}
              onChange={(e) => setDomain(e.target.value)}
              placeholder="WORKGROUP"
              className="w-full px-3 py-2 text-sm bg-app-gray border border-app-border rounded-md focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent disabled:opacity-70"
              disabled={isConnecting}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
              data-tauri-drag-region={false}
            />
          </div>

          {error && (
            <div className="px-3 py-2 text-sm text-red-400 bg-red-500/10 border border-red-500/20 rounded-md">
              {error}
            </div>
          )}

          {connected && (
            <div className="px-3 py-2 text-sm text-emerald-400 bg-emerald-500/10 border border-emerald-500/20 rounded-md flex items-center gap-2">
              <CheckCircle className="h-4 w-4" weight="duotone" />
              Connected.
            </div>
          )}

          <div className="flex items-center justify-end gap-2 pt-2">
            <button
              type="button"
              onClick={handleCancel}
              className="px-4 py-2 text-sm text-app-muted hover:text-app-text hover:bg-app-light/50 rounded-md transition-colors disabled:opacity-60"
              disabled={isConnecting}
              data-tauri-drag-region={false}
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={isConnecting || connected}
              className="px-4 py-2 text-sm font-medium text-white bg-accent hover:bg-accent/90 rounded-md transition-colors disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-2"
              data-tauri-drag-region={false}
            >
              {isConnecting ? (
                <>
                  <CircleNotch className="h-4 w-4 animate-spin" weight="bold" />
                  Connecting...
                </>
              ) : (
                'Connect'
              )}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
