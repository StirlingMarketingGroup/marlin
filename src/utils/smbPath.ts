/**
 * Check if a path is an SMB network path
 */
export function isSmbPath(path: string): boolean {
  return path.startsWith("smb://");
}

/**
 * Extract server hostname from SMB path
 */
export function parseSmbServer(path: string): string | null {
  const match = path.match(/^smb:\/\/([^/]+)/);
  return match ? match[1] : null;
}

/**
 * Extract share name from SMB path
 */
export function parseSmbShare(path: string): string | null {
  const match = path.match(/^smb:\/\/[^/]+\/([^/]+)/);
  return match ? match[1] : null;
}

/**
 * Build an SMB path from components
 */
export function buildSmbPath(
  server: string,
  share: string,
  path?: string
): string {
  const basePath = `smb://${server}/${share}`;
  if (!path || path === "/") {
    return basePath;
  }
  const cleanPath = path.startsWith("/") ? path.slice(1) : path;
  return `${basePath}/${cleanPath}`;
}
