import { memo } from 'react'
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

  const formatFileSize = (bytes: number): string => {
    if (bytes === 0) return '0 B'
    
    const k = 1024
    const sizes = ['B', 'KB', 'MB', 'GB', 'TB']
    const i = Math.floor(Math.log(bytes) / Math.log(k))
    
    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i]
  }

  const content = (
    <div style={style}>
      {variant === 'grid' ? (
        <div className="flex flex-col items-center">
          <div 
            className={`text-sm font-medium text-center ${isSelected ? 'text-white' : ''} ${className}`}
            style={{
              minHeight: '2.5rem',
              lineHeight: '1.25rem',
              wordBreak: 'break-word',
              display: '-webkit-box',
              WebkitLineClamp: 2,
              WebkitBoxOrient: 'vertical',
              overflow: 'hidden',
              width: '100%'
            }}
          >
            {displayName}
          </div>
          {!file.is_directory && showSize && (
            <div className={`text-xs mt-1 ${isSelected ? 'text-white/80' : 'text-app-muted'}`}>
              {formatFileSize(file.size)}
            </div>
          )}
        </div>
      ) : (
        <span 
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
    </div>
  )

  return content
}

export const FileNameDisplay = memo(FileNameDisplayInner)
export default FileNameDisplay