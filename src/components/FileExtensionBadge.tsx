import { memo, useMemo } from 'react';
import type { CSSProperties, HTMLAttributes } from 'react';
import { File } from 'phosphor-react';

import type { FileIconSize } from '@/components/FileTypeIcon';

type SpanProps = Omit<HTMLAttributes<HTMLSpanElement>, 'size'>;

interface FileExtensionBadgeProps extends SpanProps {
  extension?: string;
  size?: FileIconSize;
  showLabel?: boolean;
  pixelSize?: number;
  width?: number | string;
  height?: number | string;
}

function normalizeExtension(extension?: string) {
  if (!extension) return undefined;
  const trimmed = extension.toUpperCase().replace(/^\./, '');
  return trimmed || undefined;
}

function abbreviateExtension(normalized?: string) {
  if (!normalized) return 'FILE';
  if (normalized.length <= 4) return normalized;
  return `${normalized.slice(0, 3)}…`;
}

function buildAriaLabel(normalized?: string) {
  return normalized ? `${normalized} file` : 'Generic file';
}

function toCssSize(value?: number | string) {
  if (typeof value === 'number') return `${value}px`;
  if (typeof value === 'string' && value.trim().length > 0) return value;
  return undefined;
}

function FileExtensionBadgeComponent({
  extension,
  size: sizeProp = 'small',
  showLabel,
  className = '',
  pixelSize,
  width,
  height,
  style,
  ...rest
}: FileExtensionBadgeProps) {
  const normalized = useMemo(() => normalizeExtension(extension), [extension]);
  const display = useMemo(() => abbreviateExtension(normalized), [normalized]);
  const ariaLabel = useMemo(() => buildAriaLabel(normalized), [normalized]);

  const computedShowLabel = showLabel ?? sizeProp === 'large';

  const resolvedSize = toCssSize(pixelSize);
  const resolvedWidth = resolvedSize ?? toCssSize(width);
  const resolvedHeight = resolvedSize ?? toCssSize(height) ?? resolvedWidth;

  const mergedStyle: CSSProperties = { ...style };
  if (resolvedWidth) mergedStyle.width = resolvedWidth;
  if (resolvedHeight) mergedStyle.height = resolvedHeight;

  const baseClass = [
    'relative inline-flex select-none items-center justify-center text-app-text uppercase',
    computedShowLabel ? 'flex-col gap-[2px] py-[2px]' : '',
    className,
  ]
    .filter(Boolean)
    .join(' ')
    .trim();

  const iconWrapperClass = computedShowLabel
    ? 'flex-1 flex items-center justify-center w-full min-h-0'
    : 'flex items-center justify-center w-full h-full';

  const iconClass = computedShowLabel
    ? 'w-[78%] h-[78%] text-app-muted'
    : 'w-full h-full text-app-muted';

  const defaultPixelEstimate = sizeProp === 'large' ? 64 : 24;
  const basePixel = pixelSize ?? defaultPixelEstimate;
  const labelFontPx = computedShowLabel
    ? Math.round(Math.max(11, Math.min(18, basePixel * 0.18)))
    : undefined;
  const labelStyle: CSSProperties | undefined = labelFontPx
    ? { fontSize: `${labelFontPx}px`, lineHeight: 1 }
    : undefined;

  return (
    <span role="img" aria-label={ariaLabel} className={baseClass} style={mergedStyle} {...rest}>
      <span className={iconWrapperClass} aria-hidden="true">
        <File weight="fill" className={`max-h-full max-w-full ${iconClass}`} />
      </span>
      {computedShowLabel ? (
        <span className="font-semibold tracking-wide" style={labelStyle}>
          {display}
        </span>
      ) : null}
    </span>
  );
}

const FileExtensionBadge = memo(FileExtensionBadgeComponent);
FileExtensionBadge.displayName = 'FileExtensionBadge';

export default FileExtensionBadge;
