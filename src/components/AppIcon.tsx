import { useEffect, useRef, useState } from 'react'
import { useAppStore } from '@/store/useAppStore'

interface AppIconProps {
  path: string
  size?: number
  className?: string
  rounded?: boolean
  fallback?: React.ReactNode
}

export default function AppIcon({ path, size = 96, className = '', rounded = true, fallback }: AppIconProps) {
  const { appIconCache, fetchAppIcon } = useAppStore()
  const [url, setUrl] = useState<string | undefined>(appIconCache[path])
  const [loaded, setLoaded] = useState(false)
  const ref = useRef<HTMLDivElement | null>(null)
  const fetchedRef = useRef(false)

  useEffect(() => {
    if (url) return
    const el = ref.current
    if (!el) return
    const isMac = typeof navigator !== 'undefined' && navigator.platform.toUpperCase().includes('MAC')
    if (!isMac) return // Only fetch special app icons on macOS

    const observer = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        if (entry.isIntersecting && !fetchedRef.current) {
          fetchedRef.current = true
          const schedule = (cb: () => void) => {
            const ric = (window as any).requestIdleCallback as
              | ((cb: (deadline: any) => void, opts?: { timeout?: number }) => number)
              | undefined
            if (ric) ric(() => cb(), { timeout: 200 })
            else setTimeout(cb, 0)
          }
          schedule(() => {
            ;(async () => {
              const u = await fetchAppIcon(path, size)
              if (u) setUrl(u)
            })()
          })
        }
      }
    }, { rootMargin: '600px' })
    observer.observe(el)

    return () => { observer.disconnect() }
  }, [path, size, url, fetchAppIcon])

  return (
    <div ref={ref} className={className}>
      {/* Skeleton / fallback while loading */}
      {(!url || !loaded) && (
        <div className={`${rounded ? 'rounded' : ''} w-full h-full bg-app-gray/50 animate-pulse flex items-center justify-center`}>
          {fallback || null}
        </div>
      )}
      {url ? (
        // eslint-disable-next-line @next/next/no-img-element
        <img
          src={url}
          alt="app"
          className={`${rounded ? 'rounded' : ''} w-full h-full transition-opacity duration-200 ${loaded ? 'opacity-100' : 'opacity-0'}`}
          draggable={false}
          onLoad={() => setLoaded(true)}
        />
      ) : null}
    </div>
  )
}
