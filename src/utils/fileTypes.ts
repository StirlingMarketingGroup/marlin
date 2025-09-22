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
