import { ArrowSquareOut } from 'phosphor-react'
import type { CSSProperties } from 'react'

interface SymlinkBadgeProps {
  className?: string
  size?: 'sm' | 'md' | 'lg'
  style?: CSSProperties
}

export default function SymlinkBadge({ className = '', size = 'md', style }: SymlinkBadgeProps) {
  const iconClass = size === 'lg' ? 'w-5 h-5' : size === 'md' ? 'w-4 h-4' : 'w-3 h-3'
  const paddingClass = size === 'lg' ? 'p-[3px]' : size === 'md' ? 'p-[2px]' : 'p-[1px]'

  return (
    <span
      className={`pointer-events-none absolute rounded-full border border-app-border bg-app-dark/95 ${paddingClass} ${className}`}
      style={style}
    >
      <ArrowSquareOut className={`${iconClass} text-accent`} weight="fill" />
    </span>
  )
}
