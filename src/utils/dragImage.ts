import type { FileItem } from '@/types'

type DragVisual = {
  file: FileItem
  el?: HTMLElement | null
}

// Simplified, muted colors for native feel
const EXT_COLORS: Record<string, { bg: string; fg: string }> = {
  // Images - soft green
  jpg: { bg: '#23a55a20', fg: '#23a55a' }, 
  jpeg: { bg: '#23a55a20', fg: '#23a55a' }, 
  png: { bg: '#23a55a20', fg: '#23a55a' },
  gif: { bg: '#23a55a20', fg: '#23a55a' }, 
  webp: { bg: '#23a55a20', fg: '#23a55a' }, 
  svg: { bg: '#23a55a20', fg: '#23a55a' }, 
  bmp: { bg: '#23a55a20', fg: '#23a55a' },
  
  // Videos - soft blue
  mp4: { bg: '#3584e420', fg: '#3584e4' }, 
  mov: { bg: '#3584e420', fg: '#3584e4' }, 
  mkv: { bg: '#3584e420', fg: '#3584e4' }, 
  avi: { bg: '#3584e420', fg: '#3584e4' },
  
  // Documents - soft red
  pdf: { bg: '#e01b2420', fg: '#e01b24' },
  
  // 3D - soft teal
  stl: { bg: '#14b8a620', fg: '#14b8a6' },
}

function extColors(ext?: string): { bg: string; fg: string } {
  if (!ext) return { bg: '#3a3a3a', fg: '#a1a1aa' }
  const c = EXT_COLORS[ext.toLowerCase()]
  return c || { bg: '#3a3a3a', fg: '#a1a1aa' }
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

function drawFolderIcon(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number) {
  // Draw a simple folder icon
  const folderHeight = h * 0.7
  const tabWidth = w * 0.3
  const tabHeight = h * 0.15
  const cornerRadius = Math.min(w, h) * 0.06
  
  ctx.save()
  
  // Get the accent color from CSS variable (falls back to default if not found)
  const computedStyle = getComputedStyle(document.documentElement)
  const accentColor = computedStyle.getPropertyValue('--accent').trim() || '#3584e4'
  
  // Folder color (using the app's accent color)
  ctx.fillStyle = accentColor
  
  // Draw folder tab
  ctx.beginPath()
  ctx.moveTo(x + cornerRadius, y + h - folderHeight)
  ctx.lineTo(x + tabWidth, y + h - folderHeight)
  ctx.lineTo(x + tabWidth + tabHeight, y + h - folderHeight + tabHeight)
  ctx.lineTo(x + w - cornerRadius, y + h - folderHeight + tabHeight)
  ctx.arcTo(x + w, y + h - folderHeight + tabHeight, x + w, y + h - folderHeight + tabHeight + cornerRadius, cornerRadius)
  ctx.lineTo(x + w, y + h - cornerRadius)
  ctx.arcTo(x + w, y + h, x + w - cornerRadius, y + h, cornerRadius)
  ctx.lineTo(x + cornerRadius, y + h)
  ctx.arcTo(x, y + h, x, y + h - cornerRadius, cornerRadius)
  ctx.lineTo(x, y + h - folderHeight + cornerRadius)
  ctx.arcTo(x, y + h - folderHeight, x + cornerRadius, y + h - folderHeight, cornerRadius)
  ctx.closePath()
  ctx.fill()
  
  // Add subtle gradient for depth using the accent color
  const gradient = ctx.createLinearGradient(x, y + h - folderHeight, x, y + h)
  gradient.addColorStop(0, accentColor)
  // Darken the accent color slightly for the gradient
  gradient.addColorStop(1, accentColor + 'dd') // Adding transparency for subtle darkening
  ctx.fillStyle = gradient
  ctx.fill()
  
  ctx.restore()
}

function drawThumbOrBadgeWithIcon(ctx: CanvasRenderingContext2D, vis: DragVisual, x: number, y: number, size: number, prerenderedIcon?: HTMLImageElement | null) {
  const pad = Math.max(4, Math.floor(size * 0.06))
  const radius = Math.floor(size * 0.12)
  
  // Card background - subtle, translucent
  ctx.fillStyle = 'rgba(38, 38, 38, 0.9)' // app-gray with opacity
  ctx.strokeStyle = 'rgba(58, 58, 58, 0.8)' // app-border with opacity
  ctx.lineWidth = 1
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
    roundRect(ctx, innerX, innerY, innerW, innerH, Math.floor(radius * 0.8))
    ctx.clip()
    ctx.drawImage(imgEl, dx, dy, dw, dh)
    ctx.restore()
    return
  }

  // Check if it's a directory
  if (file.is_directory) {
    // Draw folder icon for directories
    const iconSize = Math.min(innerW, innerH) * 0.6
    const iconX = innerX + (innerW - iconSize) / 2
    const iconY = innerY + (innerH - iconSize) / 2
    drawFolderIcon(ctx, iconX, iconY, iconSize, iconSize)
    return
  }

  // If we have a prerendered icon, use it
  if (prerenderedIcon) {
    const iconSize = Math.min(innerW, innerH) * 0.65
    const iconX = innerX + (innerW - iconSize) / 2
    const iconY = innerY + (innerH - iconSize) / 2
    ctx.drawImage(prerenderedIcon, iconX, iconY, iconSize, iconSize)
    return
  }

  // Fallback: draw extension badge with muted colors for files
  const { bg, fg } = extColors(file.extension)
  ctx.fillStyle = bg
  roundRect(ctx, innerX, innerY, innerW, innerH, Math.floor(radius * 0.8))
  ctx.fill()
  
  // Subtle border for the extension area
  ctx.strokeStyle = fg + '30' // 30% opacity
  ctx.lineWidth = 1
  roundRect(ctx, innerX, innerY, innerW, innerH, Math.floor(radius * 0.8))
  ctx.stroke()
  
  const label = (file.extension || 'FILE').toUpperCase().slice(0, 6)
  ctx.fillStyle = fg
  ctx.font = `600 ${Math.floor(size * 0.18)}px -apple-system, ui-sans-serif, system-ui, Segoe UI, Roboto`
  ctx.textAlign = 'center'
  ctx.textBaseline = 'middle'
  ctx.fillText(label, innerX + innerW / 2, innerY + innerH / 2)
}

export async function createDragImageForSelectionAsync(
  files: FileItem[],
  container: HTMLElement,
  options?: { size?: number }
): Promise<{ element: HTMLCanvasElement; dataUrl: string }> {
  const count = Math.max(1, Math.min(files.length, 3))
  // Slightly smaller for more subtle appearance
  const base = Math.max(96, Math.min(144, options?.size ?? 120))
  const spread = Math.floor(base * 0.12) // Tighter stacking
  const textHeight = 24 // Reduced text area
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

  // Pre-render all SVG icons to images
  const prerenderedIcons: (HTMLImageElement | null)[] = await Promise.all(
    visuals.map(async (vis) => {
      const svgEl = vis.el?.querySelector('svg') as SVGElement | null
      if (!svgEl) return null
      
      try {
        const svgClone = svgEl.cloneNode(true) as SVGElement
        const viewBox = svgEl.getAttribute('viewBox') || '0 0 24 24'
        svgClone.setAttribute('viewBox', viewBox)
        svgClone.setAttribute('width', '100')
        svgClone.setAttribute('height', '100')
        
        // Force white/light color for all paths and elements in the SVG
        svgClone.setAttribute('fill', 'currentColor')
        svgClone.style.color = '#e6e6e7' // app-text color
        
        // Also update any child elements that might have explicit fill
        const elements = svgClone.querySelectorAll('*')
        elements.forEach(el => {
          if (el.hasAttribute('fill') && el.getAttribute('fill') !== 'none') {
            el.setAttribute('fill', '#e6e6e7')
          }
          if (el.hasAttribute('stroke') && el.getAttribute('stroke') !== 'none') {
            el.setAttribute('stroke', '#e6e6e7')
          }
        })
        
        const svgString = new XMLSerializer().serializeToString(svgClone)
        const blob = new Blob([svgString], { type: 'image/svg+xml;charset=utf-8' })
        const url = URL.createObjectURL(blob)
        
        return new Promise<HTMLImageElement | null>((resolve) => {
          const img = new Image()
          img.onload = () => {
            URL.revokeObjectURL(url)
            resolve(img)
          }
          img.onerror = () => {
            URL.revokeObjectURL(url)
            resolve(null)
          }
          img.src = url
        })
      } catch {
        return null
      }
    })
  )

  // Store prerendered icons on visuals
  visuals.forEach((vis, i) => {
    (vis as any).prerenderedIcon = prerenderedIcons[i]
  })

  // Softer, more subtle shadow
  ctx.save()
  ctx.shadowColor = 'rgba(0,0,0,0.25)'
  ctx.shadowBlur = Math.floor(base * 0.1)
  ctx.shadowOffsetX = 0
  ctx.shadowOffsetY = Math.floor(base * 0.03)

  // Back-to-front stack
  for (let i = count - 1; i >= 0; i--) {
    const dx = spread * i
    const dy = spread * i
    drawThumbOrBadgeWithIcon(ctx, visuals[i], dx, dy, base, (visuals[i] as any).prerenderedIcon)
  }
  ctx.restore()

  // Count badge - more subtle
  if (files.length > 1) {
    const badgeR = Math.max(11, Math.floor(base * 0.15))
    const cx = w - badgeR - 6
    const cy = badgeR + 6
    
    // Subtle background circle
    ctx.beginPath()
    ctx.fillStyle = 'rgba(53, 132, 228, 0.9)' // app-accent with transparency
    ctx.arc(cx, cy, badgeR, 0, Math.PI * 2)
    ctx.fill()
    
    // Thin border
    ctx.strokeStyle = 'rgba(255, 255, 255, 0.3)'
    ctx.lineWidth = 1
    ctx.beginPath()
    ctx.arc(cx, cy, badgeR, 0, Math.PI * 2)
    ctx.stroke()
    
    ctx.font = `600 ${Math.floor(badgeR * 1.0)}px -apple-system, ui-sans-serif, system-ui, Segoe UI, Roboto`
    ctx.fillStyle = '#ffffff'
    ctx.textAlign = 'center'
    ctx.textBaseline = 'middle'
    const text = String(files.length)
    ctx.fillText(text, cx, cy)
  }

  // File name text - cleaner, minimal
  const cardStackHeight = base + spread * (count - 1)
  const textY = cardStackHeight + 14 // Closer to cards
  
  // Determine what text to show
  let displayText: string
  if (files.length === 1) {
    // Single file: show the file name
    const fileName = files[0].name
    displayText = fileName.length > 30 ? fileName.substring(0, 27) + '...' : fileName
  } else {
    // Multiple files: show count summary
    displayText = `${files.length} items`
  }
  
  // Simple text without background - native feel
  ctx.save()
  const fontSize = 11
  ctx.font = `400 ${fontSize}px -apple-system, ui-sans-serif, system-ui, Segoe UI, Roboto`
  
  // Subtle text shadow for readability
  ctx.shadowColor = 'rgba(0, 0, 0, 0.5)'
  ctx.shadowBlur = 2
  ctx.shadowOffsetX = 0
  ctx.shadowOffsetY = 1
  
  // Text color matching app theme
  ctx.fillStyle = 'rgba(230, 230, 231, 0.9)' // app-text with slight transparency
  ctx.textAlign = 'center'
  ctx.textBaseline = 'middle'
  ctx.fillText(displayText, w / 2, textY)
  ctx.restore()

  const dataUrl = canvas.toDataURL('image/png')
  
  return { element: canvas, dataUrl }
}

// Keep the synchronous version as a fallback
export function createDragImageForSelection(
  files: FileItem[],
  container: HTMLElement,
  options?: { size?: number }
): { element: HTMLCanvasElement; dataUrl: string } {
  const count = Math.max(1, Math.min(files.length, 3))
  const base = Math.max(96, Math.min(144, options?.size ?? 120))
  const spread = Math.floor(base * 0.12)
  const textHeight = 24
  const w = base + spread * (count - 1)
  const h = base + spread * (count - 1) + textHeight
  const canvas = document.createElement('canvas')
  canvas.width = w
  canvas.height = h
  const ctx = canvas.getContext('2d')!

  const visuals: DragVisual[] = files.slice(0, count).map((file) => {
    const el = container.querySelector(`[data-file-path="${CSS.escape(file.path)}"]`) as HTMLElement | null
    return { file, el }
  })

  ctx.save()
  ctx.shadowColor = 'rgba(0,0,0,0.25)'
  ctx.shadowBlur = Math.floor(base * 0.1)
  ctx.shadowOffsetX = 0
  ctx.shadowOffsetY = Math.floor(base * 0.03)

  for (let i = count - 1; i >= 0; i--) {
    const dx = spread * i
    const dy = spread * i
    drawThumbOrBadgeWithIcon(ctx, visuals[i], dx, dy, base, null)
  }
  ctx.restore()

  // Count badge and text (same as async version)
  if (files.length > 1) {
    const badgeR = Math.max(11, Math.floor(base * 0.15))
    const cx = w - badgeR - 6
    const cy = badgeR + 6
    
    ctx.beginPath()
    ctx.fillStyle = 'rgba(53, 132, 228, 0.9)'
    ctx.arc(cx, cy, badgeR, 0, Math.PI * 2)
    ctx.fill()
    
    ctx.strokeStyle = 'rgba(255, 255, 255, 0.3)'
    ctx.lineWidth = 1
    ctx.beginPath()
    ctx.arc(cx, cy, badgeR, 0, Math.PI * 2)
    ctx.stroke()
    
    ctx.font = `600 ${Math.floor(badgeR * 1.0)}px -apple-system, ui-sans-serif, system-ui, Segoe UI, Roboto`
    ctx.fillStyle = '#ffffff'
    ctx.textAlign = 'center'
    ctx.textBaseline = 'middle'
    const text = String(files.length)
    ctx.fillText(text, cx, cy)
  }

  const cardStackHeight = base + spread * (count - 1)
  const textY = cardStackHeight + 14
  
  let displayText: string
  if (files.length === 1) {
    const fileName = files[0].name
    displayText = fileName.length > 30 ? fileName.substring(0, 27) + '...' : fileName
  } else {
    displayText = `${files.length} items`
  }
  
  ctx.save()
  const fontSize = 11
  ctx.font = `400 ${fontSize}px -apple-system, ui-sans-serif, system-ui, Segoe UI, Roboto`
  ctx.shadowColor = 'rgba(0, 0, 0, 0.5)'
  ctx.shadowBlur = 2
  ctx.shadowOffsetX = 0
  ctx.shadowOffsetY = 1
  ctx.fillStyle = 'rgba(230, 230, 231, 0.9)'
  ctx.textAlign = 'center'
  ctx.textBaseline = 'middle'
  ctx.fillText(displayText, w / 2, textY)
  ctx.restore()

  const dataUrl = canvas.toDataURL('image/png')
  return { element: canvas, dataUrl }
}
