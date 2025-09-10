import { useEffect, useRef, useState } from 'react'

export type VisibilityStage = 'far' | 'near' | 'visible'

export function useVisibility(options?: { nearMargin?: string; visibleMargin?: string }) {
  const elementRef = useRef<HTMLElement | null>(null)
  const [visible, setVisible] = useState(false)
  const [near, setNear] = useState(false)

  useEffect(() => {
    const el = elementRef.current
    if (!el) return

    const vis = new IntersectionObserver(
      (entries) => {
        const e = entries[0]
        setVisible(!!e?.isIntersecting)
      },
      { root: null, rootMargin: options?.visibleMargin ?? '0px', threshold: 0.01 }
    )

    const pre = new IntersectionObserver(
      (entries) => {
        const e = entries[0]
        setNear(!!e?.isIntersecting)
      },
      { root: null, rootMargin: options?.nearMargin ?? '800px', threshold: 0.01 }
    )

    vis.observe(el)
    pre.observe(el)

    return () => {
      vis.disconnect()
      pre.disconnect()
    }
  }, [options?.nearMargin, options?.visibleMargin])

  const stage: VisibilityStage = visible ? 'visible' : near ? 'near' : 'far'

  return { ref: elementRef, stage, visible, near }
}

