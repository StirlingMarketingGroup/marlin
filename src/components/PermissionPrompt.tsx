import { useEffect, useState, useRef, useCallback } from 'react';
import { platform } from '@tauri-apps/plugin-os';
import { ShieldCheck, FolderSimple, X } from 'phosphor-react';

// Storage key for dismissal state
const PERMISSION_DISMISSED_KEY = 'marlin_fda_prompt_dismissed';

// Type definitions for the macos-permissions plugin
interface MacOSPermissionsAPI {
  checkFullDiskAccessPermission: () => Promise<boolean>;
  requestFullDiskAccessPermission: () => Promise<void>;
}

interface PermissionPromptProps {
  onClose?: () => void;
}

export default function PermissionPrompt({ onClose }: PermissionPromptProps) {
  const [visible, setVisible] = useState(false);
  const [checking, setChecking] = useState(true);

  // Use ref to store the dynamically loaded API to avoid module-level mutable state
  const permissionsAPIRef = useRef<MacOSPermissionsAPI | null>(null);

  useEffect(() => {
    const checkPermission = async () => {
      // Only show on macOS
      const currentPlatform = platform();
      if (currentPlatform !== 'macos') {
        setChecking(false);
        return;
      }

      // Check if user previously dismissed
      const dismissed = localStorage.getItem(PERMISSION_DISMISSED_KEY);
      if (dismissed === 'true') {
        setChecking(false);
        return;
      }

      // Dynamically import the macos-permissions plugin (only available on macOS)
      try {
        const mod = await import('tauri-plugin-macos-permissions-api');
        permissionsAPIRef.current = {
          checkFullDiskAccessPermission:
            mod.checkFullDiskAccessPermission as () => Promise<boolean>,
          requestFullDiskAccessPermission:
            mod.requestFullDiskAccessPermission as () => Promise<void>,
        };
      } catch {
        // Plugin not available
        setChecking(false);
        return;
      }

      // Check if FDA is already granted
      try {
        const hasAccess = await permissionsAPIRef.current.checkFullDiskAccessPermission();
        if (!hasAccess) {
          setVisible(true);
        }
      } catch (error) {
        console.warn('Failed to check Full Disk Access permission:', error);
      }

      setChecking(false);
    };

    checkPermission();
  }, []);

  const handleGrantAccess = useCallback(async () => {
    if (!permissionsAPIRef.current) return;

    try {
      await permissionsAPIRef.current.requestFullDiskAccessPermission();
      // The system preferences will open - close the prompt
      setVisible(false);
      onClose?.();
    } catch (error) {
      console.error('Failed to request Full Disk Access:', error);
    }
  }, [onClose]);

  const handleDismiss = useCallback(() => {
    setVisible(false);
    onClose?.();
  }, [onClose]);

  const handleDontAskAgain = useCallback(() => {
    localStorage.setItem(PERMISSION_DISMISSED_KEY, 'true');
    setVisible(false);
    onClose?.();
  }, [onClose]);

  if (checking || !visible) {
    return null;
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm">
      <div className="relative bg-app-surface rounded-xl shadow-2xl max-w-md w-full mx-4 overflow-hidden border border-app-border">
        {/* Close button */}
        <button
          onClick={handleDismiss}
          className="absolute top-3 right-3 p-1.5 rounded-lg hover:bg-app-hover text-app-muted hover:text-app-text transition-colors"
          aria-label="Close"
        >
          <X size={18} weight="bold" />
        </button>

        {/* Header */}
        <div className="px-6 pt-6 pb-4 text-center">
          <div className="mx-auto w-16 h-16 rounded-full bg-app-accent/10 flex items-center justify-center mb-4">
            <ShieldCheck size={32} weight="duotone" className="text-app-accent" />
          </div>
          <h2 className="text-lg font-semibold text-app-text mb-2">Full Disk Access Recommended</h2>
          <p className="text-sm text-app-muted leading-relaxed">
            For the best experience, grant Marlin Full Disk Access. This eliminates permission
            prompts when browsing protected folders.
          </p>
        </div>

        {/* Benefits */}
        <div className="px-6 pb-4">
          <div className="bg-app-dark/50 rounded-lg p-4 space-y-3">
            <div className="flex items-start gap-3">
              <FolderSimple size={20} className="text-app-accent mt-0.5 shrink-0" />
              <div className="text-sm">
                <span className="text-app-text font-medium">
                  Access Downloads, Documents & more
                </span>
                <p className="text-app-muted text-xs mt-0.5">
                  Browse all your folders without repeated prompts
                </p>
              </div>
            </div>
            <div className="flex items-start gap-3">
              <ShieldCheck size={20} className="text-app-accent mt-0.5 shrink-0" />
              <div className="text-sm">
                <span className="text-app-text font-medium">Your choice, your control</span>
                <p className="text-app-muted text-xs mt-0.5">
                  Revoke access anytime in System Settings
                </p>
              </div>
            </div>
          </div>
        </div>

        {/* Actions */}
        <div className="px-6 pb-6 space-y-2">
          <button
            onClick={handleGrantAccess}
            className="w-full py-2.5 px-4 bg-app-accent hover:bg-app-accent/90 text-white rounded-lg font-medium transition-colors"
          >
            Open System Settings
          </button>
          <div className="flex justify-center gap-4 pt-1">
            <button
              onClick={handleDismiss}
              className="text-sm text-app-muted hover:text-app-text transition-colors"
            >
              Not now
            </button>
            <button
              onClick={handleDontAskAgain}
              className="text-sm text-app-muted hover:text-app-text transition-colors"
            >
              Don&apos;t ask again
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
