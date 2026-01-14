// src/utils/googleDriveUrl.ts

/**
 * Parse a Google Drive URL and extract the file/folder ID.
 * Supports various URL formats:
 * - https://drive.google.com/drive/folders/ID
 * - https://drive.google.com/drive/u/0/folders/ID
 * - https://drive.google.com/open?id=ID
 * - https://drive.google.com/file/d/ID/view
 *
 * @returns The extracted ID or null if not a valid Google Drive URL
 */
export function parseGoogleDriveUrl(url: string): string | null {
  // Must be a drive.google.com URL
  if (!url.includes('drive.google.com')) {
    return null;
  }

  // Try /drive/folders/ID or /drive/u/N/folders/ID
  const foldersMatch = url.match(/\/folders\/([a-zA-Z0-9_-]+)/);
  if (foldersMatch?.[1]) {
    return foldersMatch[1];
  }

  // Try /file/d/ID
  const fileMatch = url.match(/\/file\/d\/([a-zA-Z0-9_-]+)/);
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
