import { getPlatform } from '@/hooks/usePlatform';
import { buildArchiveUri, isArchiveUri, parseArchiveUri } from '@/utils/archiveUri';

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
 * The folder that holds `path`, or undefined at a root: `/`, a Windows drive,
 * or the root of a remote such as `smb://server`.
 */
export function parentPath(path: string): string | undefined {
  if (isArchiveUri(path)) {
    const archive = parseArchiveUri(path);
    if (!archive) return undefined;
    // The root of an archive stands for the archive file, so the folder above it holds that file.
    if (archive.path === '/') return parentPath(archive.src);
    return buildArchiveUri(archive.src, archive.path.slice(0, archive.path.lastIndexOf('/')));
  }
  if (!path || path === '/') return undefined;
  const normalized = path.replace(/\\/g, '/').replace(/\/+$/g, '') || '/';
  if (normalized === '/') return undefined;
  if (/^[A-Za-z]:$/.test(normalized)) return undefined;
  if (/^[a-zA-Z][a-zA-Z0-9+.-]*:\/\/[^/]*$/.test(normalized)) return undefined;
  const lastSlash = normalized.lastIndexOf('/');
  return lastSlash <= 0 ? '/' : normalized.slice(0, lastSlash);
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
