import { ArrowSquareOut } from 'phosphor-react';
import { open } from '@tauri-apps/plugin-shell';
import { useUpdateCheck } from '@/hooks/useUpdateCheck';

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

export default function UpdateNotice() {
  const { updateAvailable, latestVersion, releaseUrl, checking, error } = useUpdateCheck();

  if (checking || error || !updateAvailable || !latestVersion) {
    return null;
  }

  const label = `Marlin ${latestVersion} is available`;

  return (
    <button
      type="button"
      className="flex items-center gap-2 text-sm px-3 py-1 rounded-md bg-accent-soft text-accent border border-accent/40 hover:border-accent transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/60"
      data-tauri-drag-region={false}
      onClick={() => void openReleasePage(releaseUrl)}
      title={label}
      aria-label={label}
    >
      <span className="font-medium">Update available</span>
      <span className="text-xs text-app-muted">{latestVersion}</span>
      <ArrowSquareOut className="w-4 h-4" />
    </button>
  );
}
