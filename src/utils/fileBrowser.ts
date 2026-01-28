import { platform } from '@tauri-apps/plugin-os';
import { invoke } from '@tauri-apps/api/core';
import { getPlatform } from '@/hooks/usePlatform';
import { useToastStore } from '@/store/useToastStore';

let cachedLabel: string | null = null;

export function getFileBrowserLabel(): string {
  if (cachedLabel) return cachedLabel;
  let detected: string | null = null;
  try {
    detected = platform();
  } catch {
    detected = null;
  }

  if (!detected) {
    const { isMac, isWindows, isLinux } = getPlatform();
    cachedLabel = isMac ? 'Finder' : isWindows ? 'Explorer' : isLinux ? 'Files' : 'Files';
    return cachedLabel;
  }

  switch (detected) {
    case 'macos':
      cachedLabel = 'Finder';
      break;
    case 'windows':
      cachedLabel = 'Explorer';
      break;
    case 'linux':
      cachedLabel = 'Files';
      break;
    default:
      cachedLabel = 'Files';
  }

  return cachedLabel;
}

export function getShowInLabel(): string {
  return `Show in ${getFileBrowserLabel()}`;
}

export function parseStructuredError(error: unknown): { code: string | null; message: string } {
  const raw = error instanceof Error ? error.message : String(error);
  const match = raw.match(/^\[([A-Z]+)\]\s*/);
  if (!match) return { code: null, message: raw };
  return { code: match[1] ?? null, message: raw.slice(match[0].length) };
}

export function getRevealErrorMessage(error: unknown): string {
  const { code, message } = parseStructuredError(error);
  switch (code) {
    case 'ENOENT':
      return 'Item not found.';
    case 'EPERM':
      return 'Permission denied.';
    case 'EOPEN':
      return message || 'File browser failed to open.';
    default:
      return message || 'File browser failed to open.';
  }
}

/**
 * Check if a path is a local filesystem path (not a virtual URI scheme).
 * Returns false for archive://, smb://, etc.
 */
export function isLocalPath(path: string): boolean {
  return !path.includes('://');
}

/**
 * Reveal a file in the native file browser (Finder/Explorer/Files).
 * Shows an error toast if the operation fails.
 */
export async function revealInFileBrowser(path: string): Promise<void> {
  try {
    await invoke('reveal_in_file_browser', { path });
  } catch (error) {
    const errorMessage = getRevealErrorMessage(error);
    useToastStore.getState().addToast({
      type: 'error',
      message: `Unable to show in ${getFileBrowserLabel()}: ${errorMessage}`,
      duration: 6000,
    });
  }
}
