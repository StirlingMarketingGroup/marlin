import { segmentGraphemes, joinSlice, lengthG } from '@/utils/graphemes';

let measureCanvas: HTMLCanvasElement | null = null;
let measureContext: CanvasRenderingContext2D | null = null;

const measureCache = new Map<string, number>();

function getCacheKey(text: string, font: string): string {
  return `${text}|${font}`;
}

/**
 * Measure text width using a Canvas 2D context.
 * Accepts full CSS font shorthand components for accuracy.
 */
export function measureText(
  text: string,
  fontSize: number = 13,
  fontFamily: string = 'system-ui',
  fontWeight: string = 'normal',
  fontStyle: string = 'normal'
): number {
  const font = `${fontStyle} ${fontWeight} ${fontSize}px ${fontFamily}`;
  const cacheKey = getCacheKey(text, font);

  if (measureCache.has(cacheKey)) {
    return measureCache.get(cacheKey)!;
  }

  if (!measureCanvas) {
    measureCanvas = document.createElement('canvas');
    measureContext = measureCanvas.getContext('2d');
  }

  if (!measureContext) {
    console.warn('Failed to get 2D context for text measurement');
    return text.length * fontSize * 0.6;
  }

  measureContext.font = font;
  const metrics = measureContext.measureText(text);
  const width = metrics.width;

  measureCache.set(cacheKey, width);

  if (measureCache.size > 1000) {
    const firstKey = measureCache.keys().next().value;
    if (firstKey) measureCache.delete(firstKey);
  }

  return width;
}

export function truncateTextToWidth(
  text: string,
  maxWidth: number,
  fontSize: number = 13,
  fontFamily: string = 'system-ui',
  fontWeight: string = 'normal',
  fontStyle: string = 'normal',
  preserveExtension: boolean = true
): { text: string; isTruncated: boolean } {
  const fullWidth = measureText(text, fontSize, fontFamily, fontWeight, fontStyle);

  if (fullWidth <= maxWidth) {
    return { text, isTruncated: false };
  }

  const ellipsis = '...';
  const ellipsisWidth = measureText(ellipsis, fontSize, fontFamily, fontWeight, fontStyle);

  if (preserveExtension) {
    const lastDotIndex = text.lastIndexOf('.');
    const hasExtension = lastDotIndex > 0 && lastDotIndex < text.length - 1;

    if (hasExtension) {
      const extension = text.substring(lastDotIndex);
      const extensionWidth = measureText(extension, fontSize, fontFamily, fontWeight, fontStyle);
      const baseName = text.substring(0, lastDotIndex);
      const baseSegments = segmentGraphemes(baseName);

      const availableWidth = maxWidth - extensionWidth - ellipsisWidth;

      if (availableWidth <= 0) {
        // Column too narrow to show any basename. Try to preserve as much of the extension as possible.
        const allowForExt = Math.max(0, maxWidth - ellipsisWidth);
        if (allowForExt > 0) {
          const extSuffix = truncateFromEnd(
            extension,
            allowForExt,
            fontSize,
            fontFamily,
            fontWeight,
            fontStyle
          );
          return { text: ellipsis + extSuffix, isTruncated: true };
        }
        // Not even room for a single extension character; show just ellipsis
        return { text: ellipsis, isTruncated: true };
      }

      const halfWidth = availableWidth / 2;

      const startPart = truncateFromStart(
        baseName,
        halfWidth,
        fontSize,
        fontFamily,
        fontWeight,
        fontStyle,
        baseSegments
      );
      const startGCount = segmentGraphemes(startPart).length;
      const endPart = truncateFromEnd(
        baseName,
        halfWidth,
        fontSize,
        fontFamily,
        fontWeight,
        fontStyle,
        lengthG(baseSegments) - startGCount,
        baseSegments
      );

      return {
        text: `${startPart}${ellipsis}${endPart}${extension}`,
        isTruncated: true,
      };
    }
  }

  const availableWidth = maxWidth - ellipsisWidth;
  const halfWidth = availableWidth / 2;

  const textSegments = segmentGraphemes(text);
  const startPart = truncateFromStart(
    text,
    halfWidth,
    fontSize,
    fontFamily,
    fontWeight,
    fontStyle,
    textSegments
  );
  const startGCount = segmentGraphemes(startPart).length;
  const endPart = truncateFromEnd(
    text,
    halfWidth,
    fontSize,
    fontFamily,
    fontWeight,
    fontStyle,
    lengthG(textSegments) - startGCount,
    textSegments
  );

  return {
    text: `${startPart}${ellipsis}${endPart}`,
    isTruncated: true,
  };
}

function truncateFromStart(
  text: string,
  maxWidth: number,
  fontSize: number,
  fontFamily: string,
  fontWeight: string,
  fontStyle: string,
  segments?: string[]
): string {
  const segs = segments || segmentGraphemes(text);
  let low = 0;
  let high = segs.length;
  let result = '';

  while (low <= high) {
    const mid = Math.floor((low + high) / 2);
    const substr = joinSlice(segs, 0, mid);
    const width = measureText(substr, fontSize, fontFamily, fontWeight, fontStyle);

    if (width <= maxWidth) {
      result = substr;
      low = mid + 1;
    } else {
      high = mid - 1;
    }
  }

  return result;
}

function truncateFromEnd(
  text: string,
  maxWidth: number,
  fontSize: number,
  fontFamily: string,
  fontWeight: string,
  fontStyle: string,
  minGraphemes: number = 0,
  segments?: string[]
): string {
  const segs = segments || segmentGraphemes(text);
  let low = Math.max(0, segs.length - (minGraphemes || 0));
  let high = segs.length;
  let result = '';

  while (low <= high) {
    const mid = Math.floor((low + high) / 2);
    const substr = joinSlice(segs, mid);
    const width = measureText(substr, fontSize, fontFamily, fontWeight, fontStyle);

    if (width <= maxWidth) {
      result = substr;
      high = mid - 1;
    } else {
      low = mid + 1;
    }
  }

  return result;
}

export function clearMeasureCache(): void {
  measureCache.clear();
}
