import { getPlatform } from '@/hooks/usePlatform';

/**
 * Extract the filename/basename from a path.
 * Handles both forward slashes and backslashes (Windows).
 */
export function basename(path: string): string {
  if (!path) return '';
  const normalized = path.replace(/\\+/g, '/');
  const segments = normalized.split('/').filter(Boolean);
  return segments.length > 0 ? segments[segments.length - 1] : path;
}

/**
 * Extract the parent directory from a path.
 * Handles both forward slashes and backslashes (Windows).
 * Returns '/' for root-level files.
 */
export function dirname(path: string): string {
  if (!path) return '/';
  const normalized = path.replace(/\\+/g, '/');
  const lastSlash = normalized.lastIndexOf('/');
  if (lastSlash <= 0) return '/';
  return normalized.substring(0, lastSlash);
}

/**
 * The `server/share/folder` part of an SMB address, however it was written:
 * \\server\share\folder, smb://server/share/folder, or either with mixed slashes.
 * A user name and password before an @ are dropped, so they never reach a path or a saved server name.
 */
export function smbServerAndFolders(address: string): string {
  return address
    .replace(/^smb:/i, '')
    .replace(/\\/g, '/')
    .replace(/^\/+/, '')
    .replace(/^[^/]*@/, '');
}

/**
 * Turn text a person typed into a location Marlin can open.
 * `~` is the home folder, as in a shell.
 * Windows writes a share as \\server\share\folder. Marlin opens it as smb://server/share/folder.
 * An smb: address typed with backslashes means the same thing. On Windows a \\ path is native.
 */
export function resolveTypedPath(text: string, homeDir?: string): string {
  if (homeDir && /^~($|[\\/])/.test(text)) {
    return homeDir + text.slice(1);
  }
  if (/^smb:/i.test(text) || (!getPlatform().isWindows && text.startsWith('\\\\'))) {
    return `smb://${smbServerAndFolders(text)}`;
  }
  return text;
}
