import { memo, useEffect, useRef, useState } from 'react';
import type { CSSProperties } from 'react';
import { FileItem } from '../types';
import QuickTooltip from './QuickTooltip';
import { truncateTextToWidth } from '@/utils/textMeasure';
import { truncateToTwoLines } from '@/utils/multiLineTruncate';

interface FileNameDisplayProps {
  file: FileItem;
  maxWidth?: number;
  isSelected?: boolean;
  variant: 'grid' | 'list';
  showSize?: boolean;
  className?: string;
  style?: React.CSSProperties;
}

function FileNameDisplayInner({
  file,
  maxWidth,
  isSelected = false,
  variant,
  showSize = false,
  className = '',
  style,
}: FileNameDisplayProps) {
  const isMac =
    typeof navigator !== 'undefined' && navigator.userAgent.toUpperCase().includes('MAC');
  const X_PAD = 4; // subtle horizontal padding for selected background

  const displayName =
    isMac && file.is_directory && file.name.toLowerCase().endsWith('.app')
      ? file.name.replace(/\.app$/i, '')
      : file.name;

  // Grid: 2 lines max with center-ellipsis via DOM multi-line measurement
  // List: 1 line with center-ellipsis via canvas measurement

  const textRef = useRef<HTMLElement | null>(null);
  const containerRef = useRef<HTMLElement | null>(null);
  const [needsTooltip, setNeedsTooltip] = useState(false);
  const [renderText, setRenderText] = useState<string>(displayName);
  // Track when the name spills beyond 2 lines in grid/selected state
  const [bgHeight, setBgHeight] = useState<number>(0);
  const [hadOverflow, setHadOverflow] = useState<boolean>(false);

  useEffect(() => {
    if (variant === 'grid') {
      const compute = () => {
        const el = textRef.current as HTMLDivElement | null;
        if (!el) return;
        const width = el.clientWidth;
        if (width <= 1) return;
        const cs = window.getComputedStyle(el);
        const fontSize = parseFloat(cs.fontSize) || 13;
        const fontFamily = cs.fontFamily || 'system-ui';
        const fontWeight = cs.fontWeight || 'normal';
        const fontStyle = cs.fontStyle || 'normal';
        const lineHeightPx = (() => {
          const lh = cs.lineHeight;
          const parsed = parseFloat(lh);
          if (Number.isFinite(parsed)) return parsed;
          return Math.round(fontSize * 1.2);
        })();
        if (isSelected) {
          // Show full text and allow overflow when selected
          setRenderText(displayName);
          setNeedsTooltip(false);
          // Measure full content height so we can paint
          // a background across all lines for readability.
          try {
            const actual = el.scrollHeight;
            setBgHeight(actual > 1 ? Math.ceil(actual) : 0);
          } catch {
            setBgHeight(0);
          }
          // Determine if it would have been truncated in 2 lines
          const { isTruncated } = truncateToTwoLines(
            displayName,
            width,
            { fontSize, fontFamily, fontWeight, fontStyle, lineHeightPx, textAlign: 'center' },
            true,
            2
          );
          setHadOverflow(isTruncated);
        } else {
          const { text, isTruncated } = truncateToTwoLines(
            displayName,
            width,
            { fontSize, fontFamily, fontWeight, fontStyle, lineHeightPx, textAlign: 'center' },
            true,
            2
          );
          setRenderText(text);
          setNeedsTooltip(isTruncated);
          setBgHeight(0);
          setHadOverflow(isTruncated);
        }
      };
      compute();
      const ro = new ResizeObserver(() => compute());
      if (textRef.current) ro.observe(textRef.current);
      window.addEventListener('resize', compute);
      return () => {
        ro.disconnect();
        window.removeEventListener('resize', compute);
      };
    } else {
      const compute = () => {
        const el = containerRef.current;
        if (!el) return;
        const measureTarget =
          (el.closest('[data-name-cell="true"]') as HTMLElement) || el.parentElement || el;
        const cs = window.getComputedStyle(el);
        const width = measureTarget.clientWidth;
        if (width <= 1) return;
        const fontSize = parseFloat(cs.fontSize) || 13;
        const fontFamily = cs.fontFamily || 'system-ui';
        const fontWeight = cs.fontWeight || 'normal';
        const fontStyle = cs.fontStyle || 'normal';
        const { text, isTruncated } = truncateTextToWidth(
          displayName,
          Math.max(0, width - 1),
          fontSize,
          fontFamily,
          fontWeight,
          fontStyle,
          true
        );
        setRenderText(text);
        setNeedsTooltip(isTruncated);
      };

      compute();
      const target =
        (containerRef.current?.closest('[data-name-cell="true"]') as HTMLElement) ||
        containerRef.current?.parentElement ||
        containerRef.current;
      const ro = new ResizeObserver(() => compute());
      if (target) ro.observe(target);
      window.addEventListener('resize', compute);
      return () => {
        ro.disconnect();
        window.removeEventListener('resize', compute);
      };
    }
  }, [displayName, variant, isSelected]);

  const formatFileSize = (bytes: number): string => {
    if (bytes === 0) return '0 B';

    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));

    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
  };

  const combinedStyle = maxWidth != null ? { ...style, maxWidth } : style;

  return (
    <div style={combinedStyle}>
      {variant === 'grid' ? (
        <div className="flex flex-col items-center">
          {needsTooltip ? (
            <QuickTooltip text={displayName}>
              {(handlers) => (
                <div
                  className="relative w-full"
                  ref={(el: HTMLDivElement | null) => {
                    handlers.ref(el);
                    containerRef.current = el;
                  }}
                  onMouseEnter={handlers.onMouseEnter}
                  onMouseLeave={handlers.onMouseLeave}
                  onFocus={handlers.onFocus}
                  onBlur={handlers.onBlur}
                  style={{ overflow: 'visible' }}
                >
                  {isSelected && hadOverflow && bgHeight > 0 && (
                    <div
                      className="absolute bg-app-dark/80 pointer-events-none rounded"
                      style={{
                        left: X_PAD,
                        right: X_PAD,
                        top: 0,
                        height: `${bgHeight}px`,
                        zIndex: 25,
                      }}
                    />
                  )}
                  <div
                    ref={(el: HTMLDivElement | null) => {
                      textRef.current = el;
                    }}
                    className={`text-sm font-medium text-center ${isSelected ? 'text-white' : ''} ${className}`}
                    style={{
                      lineHeight: '1.25rem',
                      wordBreak: 'break-word',
                      display: (isSelected ? 'block' : '-webkit-box') as CSSProperties['display'],
                      WebkitLineClamp: isSelected ? undefined : 2,
                      WebkitBoxOrient: isSelected ? undefined : 'vertical',
                      overflow: isSelected ? 'visible' : 'hidden',
                      maxHeight: isSelected ? '2.5rem' : undefined,
                      width: '100%',
                      position: isSelected ? 'relative' : undefined,
                      zIndex: isSelected ? 30 : undefined,
                      pointerEvents: isSelected ? 'none' : undefined,
                      paddingLeft: isSelected && hadOverflow ? X_PAD : undefined,
                      paddingRight: isSelected && hadOverflow ? X_PAD : undefined,
                    }}
                  >
                    {renderText}
                  </div>
                </div>
              )}
            </QuickTooltip>
          ) : (
            <div className="relative w-full" style={{ overflow: 'visible' }}>
              {isSelected && hadOverflow && bgHeight > 0 && (
                <div
                  className="absolute bg-app-dark/80 pointer-events-none rounded"
                  style={{
                    left: X_PAD,
                    right: X_PAD,
                    top: 0,
                    height: `${bgHeight}px`,
                    zIndex: 25,
                  }}
                />
              )}
              <div
                ref={(el: HTMLDivElement | null) => {
                  textRef.current = el;
                }}
                className={`text-sm font-medium text-center ${isSelected ? 'text-white' : ''} ${className}`}
                style={{
                  lineHeight: '1.25rem',
                  wordBreak: 'break-word',
                  display: (isSelected ? 'block' : '-webkit-box') as CSSProperties['display'],
                  WebkitLineClamp: isSelected ? undefined : 2,
                  WebkitBoxOrient: isSelected ? undefined : 'vertical',
                  overflow: isSelected ? 'visible' : 'hidden',
                  maxHeight: isSelected ? '2.5rem' : undefined,
                  width: '100%',
                  position: isSelected ? 'relative' : undefined,
                  zIndex: isSelected ? 30 : undefined,
                  pointerEvents: isSelected ? 'none' : undefined,
                  paddingLeft: isSelected && hadOverflow ? X_PAD : undefined,
                  paddingRight: isSelected && hadOverflow ? X_PAD : undefined,
                }}
              >
                {renderText}
              </div>
            </div>
          )}
          {!file.is_directory && showSize && (
            <div className={`text-xs mt-1 ${isSelected ? 'text-white/80' : 'text-app-muted'}`}>
              {formatFileSize(file.size)}
            </div>
          )}
        </div>
      ) : needsTooltip ? (
        <QuickTooltip text={displayName}>
          {(handlers) => (
            <span
              ref={(el: HTMLSpanElement | null) => {
                textRef.current = el;
                handlers.ref(el);
                containerRef.current = el;
              }}
              onMouseEnter={handlers.onMouseEnter}
              onMouseLeave={handlers.onMouseLeave}
              onFocus={handlers.onFocus}
              onBlur={handlers.onBlur}
              className={`text-sm font-medium ${isSelected ? 'text-white' : ''} ${className}`}
              style={{
                display: 'block',
                overflow: 'hidden',
                whiteSpace: 'nowrap',
              }}
            >
              {renderText}
            </span>
          )}
        </QuickTooltip>
      ) : (
        <span
          ref={(el: HTMLSpanElement | null) => {
            textRef.current = el;
            containerRef.current = el;
          }}
          className={`text-sm font-medium ${isSelected ? 'text-white' : ''} ${className}`}
          style={{
            display: 'block',
            overflow: 'hidden',
            whiteSpace: 'nowrap',
          }}
        >
          {renderText}
        </span>
      )}
    </div>
  );
}

export const FileNameDisplay = memo(FileNameDisplayInner);
export default FileNameDisplay;
