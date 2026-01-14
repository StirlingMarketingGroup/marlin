// src/utils/googleDriveUrl.ts

/**
 * Check if a path is a Google Drive internal path (gdrive://...)
 */
export function isGoogleDrivePath(path: string): boolean {
  return path.startsWith('gdrive://');
}

/**
 * Parse a Google Drive internal path and extract the email (authority).
 * Format: gdrive://email@example.com/path/to/file
 *
 * @returns The email address or null if not a valid Google Drive path
 */
export function parseGoogleDrivePathEmail(path: string): string | null {
  if (!isGoogleDrivePath(path)) {
    return null;
  }

  // Remove the gdrive:// prefix and extract everything before the first /
  const withoutScheme = path.slice('gdrive://'.length);
  const slashIndex = withoutScheme.indexOf('/');

  if (slashIndex === -1) {
    // No path component, the whole thing is the email
    return withoutScheme || null;
  }

  const email = withoutScheme.slice(0, slashIndex);
  return email || null;
}

/**
 * Parse a Google Drive URL and extract the file/folder ID.
 * Supports various URL formats:
 * - https://drive.google.com/drive/folders/ID
 * - https://drive.google.com/drive/u/0/folders/ID
 * - https://drive.google.com/open?id=ID
 * - https://drive.google.com/file/d/ID/view
 * - https://docs.google.com/document/d/ID/edit
 *
 * @returns The extracted ID or null if not a valid Google Drive URL
 */
export function parseGoogleDriveUrl(url: string): string | null {
  // Must be a Google URL (drive.google.com or docs.google.com)
  if (!url.includes('drive.google.com') && !url.includes('docs.google.com')) {
    return null;
  }

  // Try /drive/folders/ID or /drive/u/N/folders/ID
  const foldersMatch = url.match(/\/folders\/([a-zA-Z0-9_-]+)/);
  if (foldersMatch?.[1]) {
    return foldersMatch[1];
  }

  // Try /file/d/ID or /document/d/ID (for docs.google.com)
  const fileMatch = url.match(/\/(?:file|document|spreadsheets|presentation)\/d\/([a-zA-Z0-9_-]+)/);
  if (fileMatch?.[1]) {
    return fileMatch[1];
  }

  // Try ?id=ID
  const idMatch = url.match(/[?&]id=([a-zA-Z0-9_-]+)/);
  if (idMatch?.[1]) {
    return idMatch[1];
  }

  return null;
}
