import type { FileItem } from '@/types';

export const ARCHIVE_EXTENSIONS = new Set([
  'zip',
  'rar',
  '7z',
  '7zip',
  'tar',
  'gz',
  'tgz',
  'bz2',
  'tbz2',
  'xz',
  'txz',
  'zst',
  'tzst',
  'lz',
  'lzma',
]);

export type ExtractableArchiveFormat = 'zip' | 'tar' | 'tar.gz' | 'tar.bz2' | 'tar.xz' | 'tar.zst';

const EXTRACTABLE_ARCHIVE_PATTERNS: Array<{
  format: ExtractableArchiveFormat;
  patterns: string[];
}> = [
  { format: 'tar.gz', patterns: ['.tar.gz', '.tgz'] },
  { format: 'tar.bz2', patterns: ['.tar.bz2', '.tbz2', '.tbz'] },
  { format: 'tar.xz', patterns: ['.tar.xz', '.txz'] },
  { format: 'tar.zst', patterns: ['.tar.zst', '.tzst'] },
  { format: 'tar', patterns: ['.tar'] },
  { format: 'zip', patterns: ['.zip'] },
];

export function isArchiveExtension(ext?: string | null): boolean {
  if (!ext) return false;
  return ARCHIVE_EXTENSIONS.has(ext.toLowerCase());
}

export function isArchiveFile(file: Pick<FileItem, 'name' | 'extension'>): boolean {
  const ext = file.extension?.toLowerCase();
  if (ext && ARCHIVE_EXTENSIONS.has(ext)) {
    return true;
  }
  const name = file.name.toLowerCase();
  return EXTRACTABLE_ARCHIVE_PATTERNS.some(({ patterns }) =>
    patterns.some((pattern) => name.endsWith(pattern))
  );
}

export function getExtractableArchiveFormat(
  file: Pick<FileItem, 'name' | 'extension'>
): ExtractableArchiveFormat | null {
  const name = file.name.toLowerCase();
  for (const candidate of EXTRACTABLE_ARCHIVE_PATTERNS) {
    if (candidate.patterns.some((pattern) => name.endsWith(pattern))) {
      return candidate.format;
    }
  }
  return null;
}

export const VIDEO_EXTENSIONS = new Set([
  'mp4',
  'm4v',
  'mov',
  'mkv',
  'webm',
  'avi',
  'flv',
  'wmv',
  'mpg',
  'mpeg',
  'm2ts',
  'mts',
  '3gp',
  'ogv',
]);

export function isVideoExtension(ext?: string | null): boolean {
  if (!ext) return false;
  return VIDEO_EXTENSIONS.has(ext.toLowerCase());
}

export function getEffectiveExtension(
  file: Pick<FileItem, 'name' | 'extension'>
): string | undefined {
  if (file.extension) return file.extension;

  const name = file.name;
  if (!name.includes('.')) return undefined;

  const segments = name.split('.');
  if (segments.length > 1) {
    const lastSegment = segments[segments.length - 1];
    if (lastSegment) {
      return lastSegment;
    }
  }

  return undefined;
}

/**
 * macOS bundle extensions - directories that should be opened with system apps,
 * not navigated into like regular folders. These are "packages" in macOS terminology.
 */
export const MACOS_BUNDLE_EXTENSIONS = new Set([
  'app', // Applications
  'photoslibrary', // Photos Library
  'musiclibrary', // Music Library
  'aplibrary', // Aperture Library
  'fcpbundle', // Final Cut Pro bundle
  'fcpproject', // Final Cut Pro project
  'band', // GarageBand project
  'scriv', // Scrivener project
  'rtfd', // Rich Text with attachments
  'playground', // Xcode playground
  'xcodeproj', // Xcode project
  'xcworkspace', // Xcode workspace
  'framework', // macOS framework
  'bundle', // Generic bundle
  'plugin', // Plugin bundle
  'kext', // Kernel extension
  'prefpane', // System Preferences pane
  'saver', // Screen saver
  'slidesaver', // Screen saver (slides)
  'qlgenerator', // QuickLook generator
  'mdimporter', // Spotlight importer
  'appex', // App extension
]);

/**
 * Check if a file is a macOS bundle (directory that acts like a file)
 */
export function isMacOSBundle(file: Pick<FileItem, 'name' | 'is_directory'>): boolean {
  if (!file.is_directory) return false;
  const name = file.name.toLowerCase();
  const dotIndex = name.lastIndexOf('.');
  if (dotIndex === -1) return false;
  const ext = name.slice(dotIndex + 1);
  return MACOS_BUNDLE_EXTENSIONS.has(ext);
}

/**
 * Check if a file is an app bundle specifically (for special icon handling)
 */
export function isAppBundle(file: Pick<FileItem, 'name' | 'is_directory'>): boolean {
  return file.is_directory && file.name.toLowerCase().endsWith('.app');
}
