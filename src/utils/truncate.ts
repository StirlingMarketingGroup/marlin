/**
 * Truncates a filename in the middle, preserving the start and end (including extension)
 * @param filename - The filename to truncate
 * @param maxLength - Maximum length of the result
 * @returns The truncated filename with ellipsis in the middle
 */
export function truncateMiddle(filename: string, maxLength: number): string {
  if (filename.length <= maxLength) {
    return filename;
  }

  // Find the last dot for extension
  const lastDotIndex = filename.lastIndexOf('.');
  const hasExtension = lastDotIndex > 0 && lastDotIndex < filename.length - 1;
  
  // If we have an extension, preserve it
  if (hasExtension) {
    const extension = filename.substring(lastDotIndex); // includes the dot
    const baseName = filename.substring(0, lastDotIndex);
    
    // Reserve space for extension and ellipsis
    const availableLength = maxLength - extension.length - 3; // 3 for "..."
    
    if (availableLength <= 0) {
      // If the extension alone is too long, just truncate normally
      return filename.substring(0, maxLength - 3) + '...';
    }
    
    // Split the available length between start and end of basename
    const startLength = Math.ceil(availableLength / 2);
    const endLength = Math.floor(availableLength / 2);
    
    if (baseName.length <= availableLength) {
      return filename; // No need to truncate
    }
    
    const start = baseName.substring(0, startLength);
    const end = baseName.substring(baseName.length - endLength);
    
    return `${start}...${end}${extension}`;
  } else {
    // No extension, just truncate in the middle
    const ellipsis = '...';
    const availableLength = maxLength - ellipsis.length;
    const startLength = Math.ceil(availableLength / 2);
    const endLength = Math.floor(availableLength / 2);
    
    const start = filename.substring(0, startLength);
    const end = filename.substring(filename.length - endLength);
    
    return `${start}${ellipsis}${end}`;
  }
}

/**
 * Smart truncate that adds ellipsis but prevents CSS from adding another one
 * by adding a zero-width space at the end
 */
export function truncateMiddleForCSS(filename: string, maxLength: number): string {
  const truncated = truncateMiddle(filename, maxLength);
  // Add zero-width space to prevent CSS from adding its own ellipsis
  return truncated + '\u200B';
}

/**
 * React hook for middle truncation with responsive resizing
 */
export function useMiddleTruncate(text: string, maxWidth: number, fontSize: number = 14): string {
  // Approximate character width (this is a rough estimate, could be refined)
  const charWidth = fontSize * 0.6;
  const maxChars = Math.floor(maxWidth / charWidth);
  
  return truncateMiddle(text, maxChars);
}