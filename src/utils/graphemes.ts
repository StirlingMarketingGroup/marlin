/**
 * Utilities for grapheme cluster–safe string slicing.
 * Uses Intl.Segmenter when available, falls back to code-point iteration.
 */

let seg: Intl.Segmenter | null = null

export function segmentGraphemes(text: string): string[] {
  if (typeof (Intl as any)?.Segmenter === 'function') {
    seg = seg || new (Intl as any).Segmenter(undefined, { granularity: 'grapheme' })
    const it = (seg as any).segment(text)[Symbol.iterator]()
    const out: string[] = []
    for (let r = it.next(); !r.done; r = it.next()) {
      const s = r.value
      out.push(text.slice(s.index, s.index + s.segment.length))
    }
    return out
  }
  // Fallback: split by unicode code points (may split some complex emoji)
  return Array.from(text)
}

export function joinSlice(segments: string[], start: number, end?: number): string {
  const s = Math.max(0, Math.min(start, segments.length))
  const e = end == null ? segments.length : Math.max(s, Math.min(end, segments.length))
  if (s === 0 && e === segments.length) return segments.join('')
  return segments.slice(s, e).join('')
}

export function lengthG(segments: string[]): number {
  return segments.length
}

