let measureEl: HTMLDivElement | null = null;

type FontProps = {
  fontSize: number;
  fontFamily: string;
  fontWeight: string;
  fontStyle?: string;
  lineHeightPx: number;
  textAlign?: string;
};

type Result = { text: string; isTruncated: boolean };

const cache = new Map<string, Result>();

function getMeasureEl(): HTMLDivElement {
  if (!measureEl) {
    measureEl = document.createElement('div');
    measureEl.style.position = 'absolute';
    measureEl.style.visibility = 'hidden';
    measureEl.style.zIndex = '-1000';
    measureEl.style.top = '-99999px';
    measureEl.style.left = '0';
    measureEl.style.whiteSpace = 'normal';
    measureEl.style.wordBreak = 'break-word';
    measureEl.style.overflow = 'visible';
    document.body.appendChild(measureEl);
  }
  return measureEl;
}

function setMeasureStyles(width: number, font: FontProps) {
  const el = getMeasureEl();
  el.style.width = `${Math.max(0, Math.floor(width))}px`;
  el.style.fontFamily = font.fontFamily;
  el.style.fontSize = `${font.fontSize}px`;
  el.style.fontWeight = font.fontWeight;
  el.style.fontStyle = font.fontStyle || 'normal';
  el.style.lineHeight = `${font.lineHeightPx}px`;
  el.style.letterSpacing = 'normal';
  el.style.textAlign = font.textAlign || 'center';
}

function lineCountOf(text: string, width: number, font: FontProps): number {
  const el = getMeasureEl();
  setMeasureStyles(width, font);
  el.textContent = text;
  const h = el.scrollHeight;
  const lines = Math.max(1, Math.round(h / Math.max(1, font.lineHeightPx)));
  return lines;
}

function cacheKey(
  text: string,
  width: number,
  font: FontProps,
  preserveExtension: boolean
): string {
  return [
    text,
    width,
    font.fontSize,
    font.fontFamily,
    font.fontWeight,
    font.fontStyle || 'normal',
    font.lineHeightPx,
    preserveExtension ? '1' : '0',
  ].join('|');
}

export function truncateToTwoLines(
  text: string,
  width: number,
  font: FontProps,
  preserveExtension: boolean = true,
  maxLines: number = 2
): Result {
  if (width <= 1 || !text) return { text, isTruncated: false };
  const key = cacheKey(text, Math.floor(width), font, preserveExtension);
  const hit = cache.get(key);
  if (hit) return hit;

  // Early accept if it already fits
  if (lineCountOf(text, width, font) <= maxLines) {
    const res = { text, isTruncated: false };
    cache.set(key, res);
    return res;
  }

  const ellipsis = '...';
  const lastDotIndex = text.lastIndexOf('.');
  const hasExtension = preserveExtension && lastDotIndex > 0 && lastDotIndex < text.length - 1;
  const extension = hasExtension ? text.substring(lastDotIndex) : '';
  const baseName = hasExtension ? text.substring(0, lastDotIndex) : text;
  const baseSegs = segmentGraphemes(baseName);

  // Binary search by total kept basename characters
  let low = 0;
  let high = baseSegs.length;
  let best: string = ellipsis + extension; // fallback

  const buildCandidate = (keep: number): string => {
    const startLen = Math.ceil(keep / 2);
    const endLen = Math.floor(keep / 2);
    const start = joinSlice(baseSegs, 0, Math.max(0, startLen));
    const end = joinSlice(baseSegs, Math.max(0, baseSegs.length - endLen));
    return hasExtension ? `${start}${ellipsis}${end}${extension}` : `${start}${ellipsis}${end}`;
  };

  while (low <= high) {
    const mid = Math.floor((low + high) / 2);
    const candidate = buildCandidate(mid);
    const lines = lineCountOf(candidate, width, font);
    if (lines <= maxLines) {
      best = candidate;
      low = mid + 1;
    } else {
      high = mid - 1;
    }
  }

  // Ultra-narrow case: try to preserve at least extension tail if nothing fits
  if (best === ellipsis + extension && hasExtension) {
    // Incrementally try to add extension suffix characters from the end
    for (let i = extension.length; i >= 1; i--) {
      const candidate = ellipsis + extension.slice(-i);
      if (lineCountOf(candidate, width, font) <= maxLines) {
        best = candidate;
        break;
      }
    }
  }

  const res = { text: best, isTruncated: best !== text };
  cache.set(key, res);
  if (cache.size > 1000) {
    const first = cache.keys().next().value;
    if (first) cache.delete(first);
  }
  return res;
}

import { segmentGraphemes, joinSlice } from '@/utils/graphemes';
export function clearTwoLineCache() {
  cache.clear();
}
