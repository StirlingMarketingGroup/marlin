import { ArrowSquareOut, X } from 'phosphor-react';
import { useState } from 'react';
import { open } from '@tauri-apps/plugin-shell';
import { useUpdateCheck } from '@/hooks/useUpdateCheck';

const DISMISS_KEY = 'marlin:update-dismissed-version';

async function openReleasePage(url: string) {
  try {
    await open(url);
  } catch (error) {
    try {
      window.open(url, '_blank', 'noopener,noreferrer');
    } catch (fallbackError) {
      console.warn('Unable to open release page:', fallbackError || error);
    }
  }
}

const readDismissedVersion = () => {
  if (typeof window === 'undefined') return null;
  try {
    return window.localStorage.getItem(DISMISS_KEY);
  } catch (error) {
    console.warn('Failed to read dismissed update version:', error);
    return null;
  }
};

const persistDismissedVersion = (version: string) => {
  if (typeof window === 'undefined') return;
  try {
    window.localStorage.setItem(DISMISS_KEY, version);
  } catch (error) {
    console.warn('Failed to persist dismissed update version:', error);
  }
};

export default function UpdateNotice() {
  const { updateAvailable, latestVersion, releaseUrl, checking, error } = useUpdateCheck();
  const [dismissedVersion, setDismissedVersion] = useState<string | null>(() =>
    readDismissedVersion()
  );

  const versionLabel = latestVersion?.trim() ?? '';
  const dismissed = Boolean(versionLabel) && dismissedVersion === versionLabel;

  if (checking || error || !updateAvailable || !versionLabel || dismissed) {
    return null;
  }

  const label = `Marlin ${versionLabel} is available`;

  const handleDismiss = () => {
    setDismissedVersion(versionLabel);
    persistDismissedVersion(versionLabel);
  };

  return (
    <div
      className="flex items-center gap-2 text-sm px-3 py-1 rounded-md bg-accent-soft text-accent border border-accent/40 hover:border-accent transition-colors"
      data-tauri-drag-region={false}
      role="alert"
      aria-live="polite"
    >
      <button
        type="button"
        className="flex items-center gap-2 px-2 py-0.5 rounded-md border border-transparent hover:border-accent/60 transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/60"
        onClick={() => void openReleasePage(releaseUrl)}
        title={label}
        aria-label={label}
      >
        <span className="font-medium">Update available</span>
        <span className="text-xs text-app-muted">{versionLabel}</span>
        <ArrowSquareOut className="w-4 h-4" />
      </button>
      <button
        type="button"
        onClick={handleDismiss}
        className="flex shrink-0 items-center justify-center rounded-md p-1 text-app-muted hover:text-app-foreground hover:bg-accent/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/60"
        aria-label="Dismiss update notification"
      >
        <X className="w-4 h-4" weight="bold" />
      </button>
    </div>
  );
}
