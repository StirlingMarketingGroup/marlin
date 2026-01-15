import { useCallback } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { ShieldCheck, FolderSimple, X } from 'phosphor-react';
import { FULL_DISK_ACCESS_DISMISSED_KEY } from '@/utils/fullDiskAccessPrompt';
import { WINDOW_CONTENT_TOP_PADDING } from '@/windows/windowLayout';

export default function PermissionsWindow() {
  const windowRef = getCurrentWindow();

  const closeWindow = useCallback(async () => {
    try {
      await windowRef.close();
    } catch (error) {
      console.warn('Failed to close permissions window:', error);
    }
  }, [windowRef]);

  const handleDismiss = useCallback(() => {
    void closeWindow();
  }, [closeWindow]);

  const handleDontAskAgain = useCallback(() => {
    localStorage.setItem(FULL_DISK_ACCESS_DISMISSED_KEY, 'true');
    void closeWindow();
  }, [closeWindow]);

  const handleGrantAccess = useCallback(async () => {
    try {
      const mod = await import('tauri-plugin-macos-permissions-api');
      await mod.requestFullDiskAccessPermission();
      void closeWindow();
    } catch (error) {
      console.error('Failed to request Full Disk Access:', error);
    }
  }, [closeWindow]);

  return (
    <div className="min-h-screen bg-app-dark text-app-text">
      <div
        className="relative mx-auto flex h-full max-w-lg flex-col px-6 pb-8"
        style={{ paddingTop: WINDOW_CONTENT_TOP_PADDING }}
      >
        <div data-tauri-drag-region className="absolute inset-x-2 top-0 h-10 rounded-lg" />

        <button
          onClick={handleDismiss}
          className="absolute top-2 right-2 p-1.5 rounded-lg hover:bg-app-light/50 text-app-muted hover:text-app-text transition-colors"
          aria-label="Close"
          data-tauri-drag-region={false}
        >
          <X size={18} weight="bold" />
        </button>

        <div className="pt-2 text-center">
          <div className="mx-auto w-16 h-16 rounded-full bg-app-accent/10 flex items-center justify-center mb-4 border border-app-border">
            <ShieldCheck size={32} weight="duotone" className="text-app-accent" />
          </div>
          <h2 className="text-lg font-semibold text-app-text mb-2">Full Disk Access Recommended</h2>
          <p className="text-sm text-app-muted leading-relaxed">
            For the best experience, grant Marlin Full Disk Access. This eliminates permission
            prompts when browsing protected folders.
          </p>
        </div>

        <div className="pt-5">
          <div className="bg-app-dark/50 rounded-lg p-4 space-y-3 border border-app-border">
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

        <div className="pt-6 space-y-2">
          <button
            onClick={() => void handleGrantAccess()}
            className="w-full py-2.5 px-4 bg-app-accent hover:bg-app-accent/90 text-white rounded-lg font-medium transition-colors"
            data-tauri-drag-region={false}
          >
            Open System Settings
          </button>
          <div className="flex justify-center gap-4 pt-1">
            <button
              onClick={handleDismiss}
              className="text-sm text-app-muted hover:text-app-text transition-colors"
              data-tauri-drag-region={false}
            >
              Not now
            </button>
            <button
              onClick={handleDontAskAgain}
              className="text-sm text-app-muted hover:text-app-text transition-colors"
              data-tauri-drag-region={false}
            >
              Don&apos;t ask again
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
