import { memo, useEffect, useRef, useState } from 'react'
import { FileItem } from '../types'
import QuickTooltip from './QuickTooltip'

interface FileNameDisplayProps {
  file: FileItem
  maxWidth?: number
  isSelected?: boolean
  variant: 'grid' | 'list'
  showSize?: boolean
  className?: string
  style?: React.CSSProperties
}

function FileNameDisplayInner({ 
  file, 
  maxWidth, 
  isSelected = false, 
  variant, 
  showSize = false,
  className = '',
  style 
}: FileNameDisplayProps) {
  const isMac = typeof navigator !== 'undefined' && navigator.userAgent.toUpperCase().includes('MAC')
  
  const displayName = (isMac && file.is_directory && file.name.toLowerCase().endsWith('.app'))
    ? file.name.replace(/\.app$/i, '')
    : file.name

  // For grid view, we don't truncate - CSS handles wrapping
  // For list view, we use standard CSS ellipsis

  const textRef = useRef<HTMLDivElement | HTMLSpanElement | null>(null)
  const [needsTooltip, setNeedsTooltip] = useState(false)

  useEffect(() => {
    const el = textRef.current
    if (!el) return

    const measure = () => {
      if (!textRef.current) return
      if (variant === 'list') {
        const over = textRef.current.scrollWidth > textRef.current.clientWidth + 1
        setNeedsTooltip(over)
      } else {
        // grid: multiline clamp; detect vertical overflow
        const over = textRef.current.scrollHeight > textRef.current.clientHeight + 1
        setNeedsTooltip(over)
      }
    }

    // Initial measurement
    measure()

    // Observe size changes
    const ro = new ResizeObserver(() => measure())
    ro.observe(el)

    // Window resize fallback
    window.addEventListener('resize', measure)
    return () => {
      ro.disconnect()
      window.removeEventListener('resize', measure)
    }
  }, [displayName, variant])

  const formatFileSize = (bytes: number): string => {
    if (bytes === 0) return '0 B'
    
    const k = 1024
    const sizes = ['B', 'KB', 'MB', 'GB', 'TB']
    const i = Math.floor(Math.log(bytes) / Math.log(k))
    
    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i]
  }

  return (
    <div style={style}>
      {variant === 'grid' ? (
        <div className="flex flex-col items-center">
          {needsTooltip ? (
            <QuickTooltip text={displayName}>
              {(handlers) => (
                <div
                  ref={(el) => { textRef.current = el as any; handlers.ref(el) }}
                  onMouseEnter={handlers.onMouseEnter}
                  onMouseLeave={handlers.onMouseLeave}
                  onFocus={handlers.onFocus}
                  onBlur={handlers.onBlur}
                  className={`text-sm font-medium text-center ${isSelected ? 'text-white' : ''} ${className}`}
                  style={{
                    lineHeight: '1.25rem',
                    wordBreak: 'break-word',
                    display: '-webkit-box',
                    WebkitLineClamp: 2,
                    WebkitBoxOrient: 'vertical',
                    overflow: 'hidden',
                    width: '100%'
                  }}
                  title={displayName}
                >
                  {displayName}
                </div>
              )}
            </QuickTooltip>
          ) : (
            <div
              ref={(el) => { textRef.current = el as any }}
              className={`text-sm font-medium text-center ${isSelected ? 'text-white' : ''} ${className}`}
              style={{
                lineHeight: '1.25rem',
                wordBreak: 'break-word',
                display: '-webkit-box',
                WebkitLineClamp: 2,
                WebkitBoxOrient: 'vertical',
                overflow: 'hidden',
                width: '100%'
              }}
              title={displayName}
            >
              {displayName}
            </div>
          )}
          {!file.is_directory && showSize && (
            <div className={`text-xs mt-1 ${isSelected ? 'text-white/80' : 'text-app-muted'}`}>
              {formatFileSize(file.size)}
            </div>
          )}
        </div>
      ) : (
        needsTooltip ? (
          <QuickTooltip text={displayName}>
            {(handlers) => (
              <span
                ref={(el) => { textRef.current = el as any; handlers.ref(el) }}
                onMouseEnter={handlers.onMouseEnter}
                onMouseLeave={handlers.onMouseLeave}
                onFocus={handlers.onFocus}
                onBlur={handlers.onBlur}
                className={`text-sm font-medium ${isSelected ? 'text-white' : ''} ${className}`}
                style={{
                  display: 'block',
                  overflow: 'hidden',
                  whiteSpace: 'nowrap',
                  textOverflow: 'ellipsis'
                }}
                title={displayName}
              >
                {displayName}
              </span>
            )}
          </QuickTooltip>
        ) : (
          <span
            ref={(el) => { textRef.current = el as any }}
            className={`text-sm font-medium ${isSelected ? 'text-white' : ''} ${className}`}
            style={{
              display: 'block',
              overflow: 'hidden',
              whiteSpace: 'nowrap',
              textOverflow: 'ellipsis'
            }}
            title={displayName}
          >
            {displayName}
          </span>
        )
      )}
    </div>
  )
}

export const FileNameDisplay = memo(FileNameDisplayInner)
export default FileNameDisplay
