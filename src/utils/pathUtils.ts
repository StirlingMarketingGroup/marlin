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
