import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { emit, listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { CircleNotch, Terminal, CheckCircle, X } from 'phosphor-react';
import type { SftpConnectInitPayload, SftpConnectSuccessPayload, SftpServerInfo } from '@/types';
import { SFTP_CONNECT_INIT_EVENT, SFTP_CONNECT_SUCCESS_EVENT } from '@/utils/events';
import { WINDOW_CONTENT_TOP_PADDING } from '@/windows/windowLayout';

export default function SftpConnectWindow() {
  const windowRef = getCurrentWindow();
  const hostnameInputRef = useRef<HTMLInputElement>(null);
  const usernameInputRef = useRef<HTMLInputElement>(null);

  const [initialHostname, setInitialHostname] = useState<string | undefined>();
  const [targetPath, setTargetPath] = useState<string | undefined>();

  const [hostname, setHostname] = useState('');
  const [port, setPort] = useState(22);
  const [username, setUsername] = useState('');
  const [authMethod, setAuthMethod] = useState('password');
  const [password, setPassword] = useState('');
  const [keyPath, setKeyPath] = useState('');
  const [isConnecting, setIsConnecting] = useState(false);
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState<string | undefined>();

  const hostnameLocked = useMemo(() => Boolean(initialHostname), [initialHostname]);

  useEffect(() => {
    let unlistenInit: (() => void) | undefined;
    let readyNotified = false;

    (async () => {
      try {
        unlistenInit = await listen<SftpConnectInitPayload>(SFTP_CONNECT_INIT_EVENT, (event) => {
          const p = event.payload;
          setInitialHostname(p?.initialHostname ?? undefined);
          setTargetPath(p?.targetPath ?? undefined);
          setHostname(p?.initialHostname ?? '');
          setPort(p?.initialPort ?? 22);
          setUsername(p?.initialUsername ?? '');
          setAuthMethod('password');
          setPassword('');
          setKeyPath('');
          setError(undefined);
          setIsConnecting(false);
          setConnected(false);
        });
      } catch (listenErr) {
        console.warn('Failed to listen for SFTP connect init payload:', listenErr);
      }

      try {
        await invoke('sftp_connect_window_ready');
        readyNotified = true;
      } catch (readyErr) {
        console.warn('Failed to notify SFTP connect window readiness:', readyErr);
      }
    })();

    return () => {
      if (unlistenInit) {
        unlistenInit();
      }
      if (readyNotified) {
        void invoke('sftp_connect_window_unready').catch((err) => {
          console.warn('Failed to reset SFTP connect readiness:', err);
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
    } catch {
      void invoke('hide_sftp_connect_window').catch((hideErr) => {
        console.warn('Failed to close SFTP connect window (fallback):', hideErr);
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
    if (authMethod === 'password' && !password) {
      setError('Password is required.');
      return;
    }
    if (authMethod === 'key' && !keyPath.trim()) {
      setError('SSH key path is required.');
      return;
    }

    setIsConnecting(true);
    try {
      const trimmedHostname = hostname.trim();
      const trimmedUsername = username.trim();
      const effectivePassword =
        authMethod === 'password' ? password : authMethod === 'key' ? password || null : null;
      const effectiveKeyPath = authMethod === 'key' ? keyPath.trim() : null;

      // Test the connection BEFORE saving credentials
      await invoke<boolean>('test_sftp_connection', {
        hostname: trimmedHostname,
        port,
        username: trimmedUsername,
        password: effectivePassword,
        authMethod,
        keyPath: effectiveKeyPath,
      });

      // Connection succeeded — now persist credentials
      const server = await invoke<SftpServerInfo>('add_sftp_server', {
        hostname: trimmedHostname,
        port,
        username: trimmedUsername,
        password: effectivePassword,
        authMethod,
        keyPath: effectiveKeyPath,
      });

      const portSuffix = server.port === 22 ? '' : `:${server.port}`;
      const payload: SftpConnectSuccessPayload = {
        hostname: server.hostname,
        port: server.port,
        username: server.username,
        targetPath: targetPath || `sftp://${server.username}@${server.hostname}${portSuffix}/`,
      };

      setConnected(true);
      await emit(SFTP_CONNECT_SUCCESS_EVENT, payload);

      window.setTimeout(() => {
        void closeWindow();
      }, 450);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      setIsConnecting(false);
    }
  }, [closeWindow, hostname, port, username, authMethod, password, keyPath, targetPath]);

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
        style={{ paddingTop: WINDOW_CONTENT_TOP_PADDING }}
      >
        <div data-tauri-drag-region className="absolute inset-x-2 top-0 h-10 rounded-lg" />

        <header className="flex items-start gap-3">
          <div className="mt-0.5 flex h-10 w-10 items-center justify-center rounded-full bg-app-light/30 border border-app-border">
            <Terminal className="h-5 w-5 text-accent" weight="fill" />
          </div>
          <div className="flex-1 min-w-0 space-y-1">
            <div className="font-medium text-sm">Add SFTP Server</div>
            <div className="text-xs text-app-muted">
              Enter credentials for an SFTP server (sftp://).
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
          <div className="flex gap-2">
            <div className="flex-1 space-y-1">
              <label htmlFor="sftp-hostname" className="block text-xs text-app-muted">
                Server
              </label>
              <input
                ref={hostnameInputRef}
                id="sftp-hostname"
                type="text"
                value={hostname}
                onChange={(e) => setHostname(e.target.value)}
                placeholder="host.example.com"
                className="w-full px-3 py-2 text-sm bg-app-gray border border-app-border rounded-md focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent disabled:opacity-70"
                disabled={isConnecting || hostnameLocked}
                autoComplete="off"
                autoCorrect="off"
                autoCapitalize="off"
                spellCheck={false}
                data-tauri-drag-region={false}
              />
            </div>
            <div className="w-24 space-y-1">
              <label htmlFor="sftp-port" className="block text-xs text-app-muted">
                Port
              </label>
              <input
                id="sftp-port"
                type="number"
                value={port}
                onChange={(e) => setPort(parseInt(e.target.value, 10) || 22)}
                className="w-full px-3 py-2 text-sm bg-app-gray border border-app-border rounded-md focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent disabled:opacity-70"
                disabled={isConnecting}
                min={1}
                max={65535}
                data-tauri-drag-region={false}
              />
            </div>
          </div>

          <div className="space-y-1">
            <label htmlFor="sftp-username" className="block text-xs text-app-muted">
              Username
            </label>
            <input
              ref={usernameInputRef}
              id="sftp-username"
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
            <label htmlFor="sftp-auth-method" className="block text-xs text-app-muted">
              Authentication
            </label>
            <select
              id="sftp-auth-method"
              value={authMethod}
              onChange={(e) => setAuthMethod(e.target.value)}
              className="w-full px-3 py-2 text-sm bg-app-gray border border-app-border rounded-md focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent disabled:opacity-70"
              disabled={isConnecting}
              data-tauri-drag-region={false}
            >
              <option value="password">Password</option>
              <option value="key">SSH Key</option>
              <option value="agent">SSH Agent</option>
            </select>
          </div>

          {authMethod === 'password' && (
            <div className="space-y-1">
              <label htmlFor="sftp-password" className="block text-xs text-app-muted">
                Password
              </label>
              <input
                id="sftp-password"
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
          )}

          {authMethod === 'key' && (
            <>
              <div className="space-y-1">
                <label htmlFor="sftp-key-path" className="block text-xs text-app-muted">
                  Private Key Path
                </label>
                <input
                  id="sftp-key-path"
                  type="text"
                  value={keyPath}
                  onChange={(e) => setKeyPath(e.target.value)}
                  placeholder="~/.ssh/id_ed25519"
                  className="w-full px-3 py-2 text-sm bg-app-gray border border-app-border rounded-md focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent disabled:opacity-70"
                  disabled={isConnecting}
                  autoComplete="off"
                  autoCorrect="off"
                  autoCapitalize="off"
                  spellCheck={false}
                  data-tauri-drag-region={false}
                />
              </div>
              <div className="space-y-1">
                <label htmlFor="sftp-key-passphrase" className="block text-xs text-app-muted">
                  Passphrase <span className="text-app-muted/60">(optional)</span>
                </label>
                <input
                  id="sftp-key-passphrase"
                  type="password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  placeholder="••••••••"
                  className="w-full px-3 py-2 text-sm bg-app-gray border border-app-border rounded-md focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent disabled:opacity-70"
                  disabled={isConnecting}
                  autoComplete="off"
                  data-tauri-drag-region={false}
                />
              </div>
            </>
          )}

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
