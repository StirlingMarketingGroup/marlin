import type { FileItem } from '@/types'

type DragVisual = {
  file: FileItem
  el?: HTMLElement | null
}

const EXT_COLORS: Record<string, { bg: string; fg: string }> = {
  // Common media
  jpg: { bg: '#10b981', fg: '#ffffff' }, jpeg: { bg: '#10b981', fg: '#ffffff' }, png: { bg: '#10b981', fg: '#ffffff' },
  gif: { bg: '#10b981', fg: '#ffffff' }, webp: { bg: '#10b981', fg: '#ffffff' }, svg: { bg: '#10b981', fg: '#111827' }, bmp: { bg: '#10b981', fg: '#ffffff' },
  mp4: { bg: '#ef4444', fg: '#ffffff' }, mov: { bg: '#ef4444', fg: '#ffffff' }, mkv: { bg: '#ef4444', fg: '#ffffff' }, avi: { bg: '#ef4444', fg: '#ffffff' },
  pdf: { bg: '#ef4444', fg: '#ffffff' },
  stl: { bg: '#14b8a6', fg: '#ffffff' },
}

function extColors(ext?: string): { bg: string; fg: string } {
  if (!ext) return { bg: '#6b7280', fg: '#ffffff' }
  const c = EXT_COLORS[ext.toLowerCase()]
  return c || { bg: '#6b7280', fg: '#ffffff' }
}

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  const rr = Math.min(r, w / 2, h / 2)
  ctx.beginPath()
  ctx.moveTo(x + rr, y)
  ctx.arcTo(x + w, y, x + w, y + h, rr)
  ctx.arcTo(x + w, y + h, x, y + h, rr)
  ctx.arcTo(x, y + h, x, y, rr)
  ctx.arcTo(x, y, x + w, y, rr)
  ctx.closePath()
}

function drawThumbOrBadge(ctx: CanvasRenderingContext2D, vis: DragVisual, x: number, y: number, size: number) {
  const pad = Math.max(6, Math.floor(size * 0.08))
  const radius = Math.floor(size * 0.08)
  // Card background - more opaque for visibility
  ctx.fillStyle = 'rgba(17, 24, 39, 0.95)'
  ctx.strokeStyle = 'rgba(255, 255, 255, 0.25)'
  ctx.lineWidth = 1.5
  roundRect(ctx, x, y, size, size, radius)
  ctx.fill()
  ctx.stroke()

  // Inner preview area
  const innerX = x + pad
  const innerY = y + pad
  const innerW = size - pad * 2
  const innerH = size - pad * 2
  const file = vis.file

  // Try to find an <img> inside the element (already loaded thumbnail)
  const imgEl = vis.el ? (vis.el.querySelector('img') as HTMLImageElement | null) : null
  if (imgEl && imgEl.naturalWidth && imgEl.naturalHeight) {
    // Draw image centered, contain
    const iw = imgEl.naturalWidth
    const ih = imgEl.naturalHeight
    const scale = Math.min(innerW / iw, innerH / ih)
    const dw = Math.max(1, Math.floor(iw * scale))
    const dh = Math.max(1, Math.floor(ih * scale))
    const dx = innerX + Math.floor((innerW - dw) / 2)
    const dy = innerY + Math.floor((innerH - dh) / 2)
    ctx.save()
    ctx.beginPath()
    roundRect(ctx, innerX, innerY, innerW, innerH, Math.floor(radius * 0.75))
    ctx.clip()
    ctx.drawImage(imgEl, dx, dy, dw, dh)
    ctx.restore()
    return
  }

  // Fallback: draw extension block
  const { bg, fg } = extColors(file.extension)
  ctx.fillStyle = bg
  roundRect(ctx, innerX, innerY, innerW, innerH, Math.floor(radius * 0.75))
  ctx.fill()
  const label = (file.extension || (file.is_directory ? 'DIR' : 'FILE')).toUpperCase().slice(0, 6)
  ctx.fillStyle = fg
  ctx.font = `bold ${Math.floor(size * 0.22)}px ui-sans-serif, system-ui, -apple-system, Segoe UI, Roboto`
  ctx.textAlign = 'center'
  ctx.textBaseline = 'middle'
  ctx.fillText(label, innerX + innerW / 2, innerY + innerH / 2)
}

export function createDragImageForSelection(
  files: FileItem[],
  container: HTMLElement,
  options?: { size?: number }
): { element: HTMLCanvasElement; dataUrl: string } {
  const count = Math.max(1, Math.min(files.length, 3))
  // Make it bigger for visibility
  const base = Math.max(128, Math.min(256, options?.size ?? 160))
  const spread = Math.floor(base * 0.18)
  const textHeight = 32 // Extra space for file name text
  const w = base + spread * (count - 1)
  const h = base + spread * (count - 1) + textHeight
  const canvas = document.createElement('canvas')
  canvas.width = w
  canvas.height = h
  const ctx = canvas.getContext('2d')!
  


  // Pick the top N visuals from current selection (first 3)
  const visuals: DragVisual[] = files.slice(0, count).map((file) => {
    const el = container.querySelector(`[data-file-path="${CSS.escape(file.path)}"]`) as HTMLElement | null
    return { file, el }
  })

  // Drop shadow
  ctx.save()
  ctx.shadowColor = 'rgba(0,0,0,0.35)'
  ctx.shadowBlur = Math.floor(base * 0.15)
  ctx.shadowOffsetX = Math.floor(base * 0.04)
  ctx.shadowOffsetY = Math.floor(base * 0.06)

  // Back-to-front stack
  for (let i = count - 1; i >= 0; i--) {
    const dx = spread * i
    const dy = spread * i
    drawThumbOrBadge(ctx, visuals[i], dx, dy, base)
  }
  ctx.restore()

  // Count badge
  if (files.length > 1) {
    const badgeR = Math.max(12, Math.floor(base * 0.18))
    const cx = w - badgeR - 4
    const cy = badgeR + 4
    ctx.beginPath()
    ctx.fillStyle = '#ef4444'
    ctx.arc(cx, cy, badgeR, 0, Math.PI * 2)
    ctx.fill()
    ctx.font = `bold ${Math.floor(badgeR * 1.1)}px -apple-system, system-ui, Segoe UI, Roboto`
    ctx.fillStyle = '#ffffff'
    ctx.textAlign = 'center'
    ctx.textBaseline = 'middle'
    const text = String(files.length)
    ctx.fillText(text, cx, cy)
  }

  // File name text
  const cardStackHeight = base + spread * (count - 1)
  const textY = cardStackHeight + 20 // 20px below the cards
  const maxTextWidth = w - 16 // 8px padding on each side
  
  // Determine what text to show
  let displayText: string
  if (files.length === 1) {
    // Single file: show the file name
    const fileName = files[0].name
    displayText = fileName.length > 25 ? fileName.substring(0, 22) + '...' : fileName
  } else {
    // Multiple files: show count summary
    displayText = `${files.length} files`
  }
  
  // Draw text background (rounded rectangle)
  ctx.save()
  const fontSize = 12
  ctx.font = `${fontSize}px -apple-system, system-ui, Segoe UI, Roboto`
  const textMetrics = ctx.measureText(displayText)
  const textWidth = textMetrics.width
  const bgWidth = Math.min(textWidth + 12, maxTextWidth) // 6px padding on each side
  const bgHeight = 18
  const bgX = (w - bgWidth) / 2
  const bgY = textY - bgHeight / 2
  
  // Semi-transparent background
  ctx.fillStyle = 'rgba(0, 0, 0, 0.75)'
  const cornerRadius = 4
  ctx.beginPath()
  ctx.roundRect(bgX, bgY, bgWidth, bgHeight, cornerRadius)
  ctx.fill()
  
  // Text
  ctx.fillStyle = '#ffffff'
  ctx.textAlign = 'center'
  ctx.textBaseline = 'middle'
  ctx.fillText(displayText, w / 2, textY)
  ctx.restore()

  const dataUrl = canvas.toDataURL('image/png')
  
  return { element: canvas, dataUrl }
}
