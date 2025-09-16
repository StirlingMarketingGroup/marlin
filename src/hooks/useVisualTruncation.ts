import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { truncateMiddle } from '../utils/truncate';

interface TruncationResult {
  text: string; // The truncated (or full) text
  isTruncated: boolean; // Whether truncation was applied
  fullText: string; // Original full text
}

// Cache for truncation results
const truncationCache = new Map<string, TruncationResult>();

export function useVisualTruncation(
  text: string,
  containerRef: React.RefObject<HTMLElement>,
  maxWidth?: number,
  enabled: boolean = true
): TruncationResult {
  const [result, setResult] = useState<TruncationResult>({
    text,
    isTruncated: false,
    fullText: text,
  });

  const measureRef = useRef<HTMLDivElement | null>(null);
  const debounceRef = useRef<number>();

  // Create cache key
  const cacheKey = useMemo(() => {
    const width = maxWidth ?? containerRef.current?.offsetWidth ?? 0;
    return `${text}-${width}`;
  }, [text, maxWidth, containerRef]);

  const measureTruncation = useCallback(() => {
    if (!enabled || !text) {
      setResult({ text, isTruncated: false, fullText: text });
      return;
    }

    // Check cache first
    const cached = truncationCache.get(cacheKey);
    if (cached) {
      setResult(cached);
      return;
    }

    const container = containerRef.current;
    if (!container) {
      setResult({ text, isTruncated: false, fullText: text });
      return;
    }

    // Step 1: Create measuring element if needed
    if (!measureRef.current) {
      measureRef.current = document.createElement('div');
      measureRef.current.style.position = 'absolute';
      measureRef.current.style.visibility = 'hidden';
      measureRef.current.style.zIndex = '-1000';
      measureRef.current.style.wordBreak = 'break-word';
      measureRef.current.style.textAlign = 'center';
      document.body.appendChild(measureRef.current);
    }

    const measureEl = measureRef.current;

    // Copy styles from the actual container
    const containerStyles = window.getComputedStyle(container);
    measureEl.style.fontFamily = containerStyles.fontFamily;
    measureEl.style.fontSize = containerStyles.fontSize;
    measureEl.style.fontWeight = containerStyles.fontWeight;
    measureEl.style.fontStyle = containerStyles.fontStyle;
    measureEl.style.lineHeight = containerStyles.lineHeight;
    measureEl.style.letterSpacing = containerStyles.letterSpacing;
    measureEl.style.width = `${container.offsetWidth}px`;
    measureEl.style.height = `${container.offsetHeight}px`;

    // Step 2: Test if full text fits within 2 lines
    measureEl.textContent = text;
    const doesOverflow = measureEl.scrollHeight > measureEl.clientHeight;

    if (!doesOverflow) {
      // Text fits naturally, no truncation needed
      const result: TruncationResult = { text, isTruncated: false, fullText: text };
      truncationCache.set(cacheKey, result);
      setResult(result);
      return;
    }

    // Step 3: Text overflows, use binary search to find optimal middle truncation
    let low = 10; // minimum meaningful length
    let high = text.length - 5; // leave room for ellipsis
    let bestFit = text;

    while (low <= high) {
      const mid = Math.floor((low + high) / 2);
      const truncated = truncateMiddle(text, mid);

      measureEl.textContent = truncated;
      const measuredOverflow = measureEl.scrollHeight > measureEl.clientHeight;

      if (!measuredOverflow) {
        // This fits within 2 lines, try longer
        bestFit = truncated;
        low = mid + 1;
      } else {
        // Still overflows, try shorter
        high = mid - 1;
      }
    }

    // Final result
    const finalResult: TruncationResult = {
      text: bestFit,
      isTruncated: bestFit !== text,
      fullText: text,
    };

    // Cache the result
    truncationCache.set(cacheKey, finalResult);
    setResult(finalResult);
  }, [text, cacheKey, enabled, containerRef]);

  // Debounced measure function
  const debouncedMeasure = useCallback(() => {
    if (debounceRef.current) {
      clearTimeout(debounceRef.current);
    }
    debounceRef.current = window.setTimeout(measureTruncation, 16); // ~1 frame delay
  }, [measureTruncation]);

  // Measure on mount and when dependencies change
  useEffect(() => {
    if (!enabled) return;

    // Use requestAnimationFrame for better performance
    const rafId = requestAnimationFrame(debouncedMeasure);
    return () => cancelAnimationFrame(rafId);
  }, [debouncedMeasure, enabled]);

  // Cleanup measure element on unmount
  useEffect(() => {
    return () => {
      if (measureRef.current) {
        document.body.removeChild(measureRef.current);
        measureRef.current = null;
      }
      if (debounceRef.current) {
        clearTimeout(debounceRef.current);
      }
    };
  }, []);

  return result;
}

// Clear cache function (useful for memory management)
export function clearTruncationCache() {
  truncationCache.clear();
}
