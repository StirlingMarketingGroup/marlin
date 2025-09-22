import type { FileItem } from '@/types';

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
