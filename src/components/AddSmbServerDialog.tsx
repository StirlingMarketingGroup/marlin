import { useState, useEffect, useRef, FormEvent } from 'react';
import { createPortal } from 'react-dom';
import { X, CircleNotch, ShareNetwork } from 'phosphor-react';
import { useAppStore } from '@/store/useAppStore';
import { useToastStore } from '@/store/useToastStore';

interface AddSmbServerDialogProps {
  isOpen: boolean;
  onClose: () => void;
}

export default function AddSmbServerDialog({ isOpen, onClose }: AddSmbServerDialogProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const firstInputRef = useRef<HTMLInputElement>(null);

  const [hostname, setHostname] = useState('');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [domain, setDomain] = useState('');
  const [isConnecting, setIsConnecting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const { addSmbServer, navigateTo } = useAppStore();

  // Focus first input when dialog opens
  useEffect(() => {
    if (isOpen) {
      // Small delay to ensure dialog is rendered
      setTimeout(() => firstInputRef.current?.focus(), 50);
    }
  }, [isOpen]);

  // Reset form when dialog closes
  useEffect(() => {
    if (!isOpen) {
      setHostname('');
      setUsername('');
      setPassword('');
      setDomain('');
      setError(null);
      setIsConnecting(false);
    }
  }, [isOpen]);

  // Handle escape key
  useEffect(() => {
    if (!isOpen) return;

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onClose();
      }
    };

    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [isOpen, onClose]);

  // Handle click outside
  useEffect(() => {
    if (!isOpen) return;

    const onClick = (e: MouseEvent) => {
      const target = e.target as Node | null;
      if (dialogRef.current && target && !dialogRef.current.contains(target)) {
        onClose();
      }
    };

    // Use setTimeout to avoid the click that opened the dialog
    const timeoutId = setTimeout(() => {
      document.addEventListener('mousedown', onClick);
    }, 100);

    return () => {
      clearTimeout(timeoutId);
      document.removeEventListener('mousedown', onClick);
    };
  }, [isOpen, onClose]);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);

    if (!hostname.trim()) {
      setError('Server hostname is required');
      return;
    }

    if (!username.trim()) {
      setError('Username is required');
      return;
    }

    if (!password) {
      setError('Password is required');
      return;
    }

    setIsConnecting(true);
    const { addToast } = useToastStore.getState();

    try {
      const server = await addSmbServer(
        hostname.trim(),
        username.trim(),
        password,
        domain.trim() || undefined
      );

      addToast({
        type: 'success',
        message: `Connected to ${server.hostname}`,
        duration: 3000,
      });

      onClose();

      // Navigate to the newly added server
      navigateTo(`smb://${server.hostname}/`);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      setIsConnecting(false);
    }
  };

  if (!isOpen) return null;

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm">
      <div
        ref={dialogRef}
        className="bg-app-dark border border-app-border rounded-lg shadow-xl w-full max-w-md mx-4"
      >
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-app-border">
          <div className="flex items-center gap-2">
            <ShareNetwork className="w-5 h-5 text-accent" weight="fill" />
            <h2 className="text-base font-medium">Add Network Share</h2>
          </div>
          <button
            onClick={onClose}
            className="p-1 rounded hover:bg-app-light/50 text-app-muted hover:text-app-text transition-colors"
          >
            <X className="w-4 h-4" weight="bold" />
          </button>
        </div>

        {/* Form */}
        <form onSubmit={handleSubmit} className="p-4 space-y-4">
          {/* Server hostname */}
          <div className="space-y-1">
            <label htmlFor="smb-hostname" className="block text-sm text-app-muted">
              Server
            </label>
            <input
              ref={firstInputRef}
              id="smb-hostname"
              type="text"
              value={hostname}
              onChange={(e) => setHostname(e.target.value)}
              placeholder="server.local or 192.168.1.100"
              className="w-full px-3 py-2 text-sm bg-app-gray border border-app-border rounded-md focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent"
              disabled={isConnecting}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
            />
          </div>

          {/* Username */}
          <div className="space-y-1">
            <label htmlFor="smb-username" className="block text-sm text-app-muted">
              Username
            </label>
            <input
              id="smb-username"
              type="text"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              placeholder="user"
              className="w-full px-3 py-2 text-sm bg-app-gray border border-app-border rounded-md focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent"
              disabled={isConnecting}
              autoComplete="username"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
            />
          </div>

          {/* Password */}
          <div className="space-y-1">
            <label htmlFor="smb-password" className="block text-sm text-app-muted">
              Password
            </label>
            <input
              id="smb-password"
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="••••••••"
              className="w-full px-3 py-2 text-sm bg-app-gray border border-app-border rounded-md focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent"
              disabled={isConnecting}
              autoComplete="current-password"
            />
          </div>

          {/* Domain (optional) */}
          <div className="space-y-1">
            <label htmlFor="smb-domain" className="block text-sm text-app-muted">
              Domain <span className="text-app-muted/60">(optional)</span>
            </label>
            <input
              id="smb-domain"
              type="text"
              value={domain}
              onChange={(e) => setDomain(e.target.value)}
              placeholder="WORKGROUP"
              className="w-full px-3 py-2 text-sm bg-app-gray border border-app-border rounded-md focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent"
              disabled={isConnecting}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
            />
          </div>

          {/* Error message */}
          {error && (
            <div className="px-3 py-2 text-sm text-red-400 bg-red-500/10 border border-red-500/20 rounded-md">
              {error}
            </div>
          )}

          {/* Actions */}
          <div className="flex items-center justify-end gap-2 pt-2">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-2 text-sm text-app-muted hover:text-app-text hover:bg-app-light/50 rounded-md transition-colors"
              disabled={isConnecting}
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={isConnecting}
              className="px-4 py-2 text-sm font-medium text-white bg-accent hover:bg-accent/90 rounded-md transition-colors disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-2"
            >
              {isConnecting ? (
                <>
                  <CircleNotch className="w-4 h-4 animate-spin" weight="bold" />
                  Connecting...
                </>
              ) : (
                'Connect'
              )}
            </button>
          </div>
        </form>
      </div>
    </div>,
    document.body
  );
}
