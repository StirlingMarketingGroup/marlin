import { useRef, useState } from 'react'
import { useThumbnail } from '@/hooks/useThumbnail'

interface AppIconProps {
  path: string
  size?: number
  className?: string
  rounded?: boolean
  fallback?: React.ReactNode
  priority?: 'high' | 'medium' | 'low'
}

export default function AppIcon({ 
  path, 
  size = 96, 
  className = '', 
  rounded = true, 
  fallback,
  priority = 'medium'
}: AppIconProps) {
  const [loaded, setLoaded] = useState(false)
  const ref = useRef<HTMLDivElement | null>(null)
  
  // Use the new ultra-efficient thumbnail system
  const { dataUrl, loading, error, cached, generationTimeMs } = useThumbnail(path, {
    size,
    quality: 'medium',
    priority,
    format: 'png' // Use PNG for app icons to preserve transparency
  })

  const isMac = typeof navigator !== 'undefined' && navigator.platform.toUpperCase().includes('MAC')
  const shouldShowThumbnail = isMac && (
    path.toLowerCase().endsWith('.app') || 
    path.toLowerCase().endsWith('.dmg') || 
    path.toLowerCase().endsWith('.pkg')
  )

  return (
    <div ref={ref} className={className}>
      {/* Skeleton / fallback while loading */}
      {(!dataUrl || !loaded) && (
        <div className={`${rounded ? 'rounded' : ''} w-full h-full bg-app-gray/50 ${loading ? 'animate-pulse' : ''} flex items-center justify-center`}>
          {fallback || null}
        </div>
      )}
      
      {/* Show thumbnail if available */}
      {dataUrl && shouldShowThumbnail ? (
        <img
          src={dataUrl}
          alt="app"
          className={`${rounded ? 'rounded' : ''} w-full h-full transition-opacity duration-200 ${loaded ? 'opacity-100' : 'opacity-0'}`}
          draggable={false}
          onLoad={() => setLoaded(true)}
          title={error ? `Error: ${error}` : `${cached ? 'Cached' : 'Generated'} in ${generationTimeMs}ms`}
        />
      ) : null}
      
      {/* Debug info in development */}
      {process.env.NODE_ENV === 'development' && (
        <div className="absolute bottom-0 right-0 text-xs bg-black text-white px-1 rounded opacity-75">
          {cached ? 'C' : 'G'}{generationTimeMs}
        </div>
      )}
    </div>
  )
}
