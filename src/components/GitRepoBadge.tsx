import { GitBranch } from 'phosphor-react';
import type { CSSProperties } from 'react';

interface GitRepoBadgeProps {
  className?: string;
  size?: 'sm' | 'md' | 'lg';
  style?: CSSProperties;
}

export default function GitRepoBadge({ className = '', size = 'md', style }: GitRepoBadgeProps) {
  const iconClass = size === 'lg' ? 'w-5 h-5' : size === 'md' ? 'w-4 h-4' : 'w-3 h-3';
  const paddingClass = size === 'lg' ? 'p-[3px]' : size === 'md' ? 'p-[2px]' : 'p-[1px]';

  return (
    <span
      className={`pointer-events-none absolute rounded-full border border-app-border bg-app-dark/95 ${paddingClass} ${className}`}
      style={style}
    >
      <GitBranch className={`${iconClass} text-orange-400`} weight="fill" />
    </span>
  );
}
