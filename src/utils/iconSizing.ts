import { cloneElement, isValidElement } from 'react';
import type { ReactElement, ReactNode } from 'react';
import { FileTypeIcon } from '@/components/FileTypeIcon';
import FileExtensionBadge from '@/components/FileExtensionBadge';

interface NormalizeOptions {
  className?: string;
}

/**
 * Clone icons so they fill preview tiles consistently regardless of source component.
 */
export function normalizePreviewIcon(
  icon: ReactNode,
  targetPx: number,
  options: NormalizeOptions = {}
) {
  if (!isValidElement(icon)) return icon;

  type IconProps = {
    className?: unknown;
    size?: unknown;
    width?: unknown;
    height?: unknown;
  };

  const element = icon as ReactElement<IconProps>;
  const elementProps = element.props ?? {};
  const elementClass =
    typeof elementProps.className === 'string' ? (elementProps.className as string) : undefined;

  const mergedClassName = [elementClass, options.className, 'w-full', 'h-full']
    .filter(Boolean)
    .join(' ')
    .trim();

  const nextProps: Record<string, unknown> = { className: mergedClassName };

  if (element.type === FileTypeIcon) {
    nextProps.pixelSize = targetPx;
  } else if (element.type === FileExtensionBadge) {
    nextProps.pixelSize = targetPx;
  } else {
    if (typeof elementProps.size === 'undefined' || typeof elementProps.size === 'number') {
      nextProps.size = targetPx;
    }
    if (typeof elementProps.width === 'undefined' || typeof elementProps.width === 'number') {
      nextProps.width = targetPx;
    }
    if (typeof elementProps.height === 'undefined' || typeof elementProps.height === 'number') {
      nextProps.height = targetPx;
    }
  }

  return cloneElement(element, nextProps);
}
