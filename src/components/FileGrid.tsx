import { useState, useMemo, useEffect, useRef, type ReactNode } from 'react'
import { Folder, File, ImageSquare, MusicNote, VideoCamera, FileZip, FileText, AppWindow, Package, FilePdf, PaintBrush, Palette, Disc, Cube } from 'phosphor-react'
import { FileItem, ViewPreferences } from '../types'
import { useAppStore } from '../store/useAppStore'
import AppIcon from '@/components/AppIcon'
import { FileTypeIcon, resolveVSCodeIcon } from '@/components/FileTypeIcon'
import { open } from '@tauri-apps/plugin-shell'

import { invoke } from '@tauri-apps/api/core'
import { createDragImageForSelection, createDragImageForSelectionAsync } from '@/utils/dragImage'
// no direct invoke here; background opens the menu
import { useThumbnail } from '@/hooks/useThumbnail'
import { useVisibility } from '@/hooks/useVisibility'
import FileNameDisplay from './FileNameDisplay'

interface FileGridProps {
  files: FileItem[]
  preferences: ViewPreferences
}

// Stable, top-level preview component to avoid remount flicker
function GridFilePreview({ file, isMac, fallbackIcon, tile }: { file: FileItem; isMac: boolean; fallbackIcon: ReactNode; tile: number }) {
  const { ref, stage } = useVisibility({ nearMargin: '900px', visibleMargin: '0px' })
  const ext = file.extension?.toLowerCase()
  const isImage = !!ext && ['jpg', 'jpeg', 'png', 'gif', 'webp', 'bmp', 'tiff', 'tga', 'ico', 'svg'].includes(ext || '')
  const isPdf = ext === 'pdf'
  const isAi = ext === 'ai' || ext === 'eps'
  const isPsd = ext === 'psd' || ext === 'psb'

  const isStl = ext === 'stl'
  const isAppBundle = isMac && file.is_directory && file.name.toLowerCase().endsWith('.app')

  // (Rendering handled below with a fixed preview box for alignment)

  // Device pixel ratio quantized to 1 or 2 for cache reuse
  const dpr = typeof window !== 'undefined' ? (window.devicePixelRatio && window.devicePixelRatio > 1.5 ? 2 : 1) : 1
  // Padding is small and stable (more noticeable for breathing room)
  const pad = Math.max(3, Math.min(8, Math.round(tile * 0.03)))
  // Fixed preview box to keep rows aligned
  const box = Math.max(48, Math.min(tile - pad * 2, 320))

  // Bucket request sizes to improve cache hits across navigation and zoom
  const pickBucket = (target: number) => {
    const buckets = [64, 96, 128, 192, 256, 320, 384, 512]
    let best = buckets[0]
    let bestDiff = Math.abs(buckets[0] - target)
    for (let i = 1; i < buckets.length; i++) {
      const diff = Math.abs(buckets[i] - target)
      if (diff < bestDiff) { best = buckets[i]; bestDiff = diff }
    }
    return best
  }

  // Image-like previews (real thumbnails)
  if (isImage || isPdf || isAi || isPsd || isStl) {
    const requestSize = pickBucket(Math.round((box - pad * 2) * dpr))
    const shouldLoad = stage !== 'far'
    const priority = stage === 'visible' ? 'high' : 'medium'
    const { dataUrl, loading, hasTransparency } = useThumbnail(shouldLoad ? file.path : undefined, { size: requestSize, quality: 'medium', priority })
    if (dataUrl) {
      return (
        <div ref={ref as any} className={`rounded-md border border-app-border ${hasTransparency ? 'bg-checker' : ''} overflow-hidden`} style={{ width: box, height: box, padding: pad, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
          <img
            src={dataUrl}
            alt={file.name}
            className={`max-w-full max-h-full w-full h-full`}
            style={{ objectFit: 'contain', transform: 'none' }}
            draggable={false}
          />
        </div>
      )
    }
    if (loading) {
      return <div ref={ref as any} className="rounded-md border border-app-border animate-pulse" style={{ width: box, height: box, padding: pad }} />
    }
    return <div ref={ref as any} className="rounded-md border border-app-border" style={{ width: box, height: box, padding: pad }} />
  }

  // macOS .app Application icons (native icons)
  if (isAppBundle) {
    const requestSize = pickBucket(Math.round((box - pad * 2) * dpr))
    if (stage === 'far') {
      return <div ref={ref as any} className="overflow-hidden rounded-md border border-app-border bg-checker" style={{ width: box, height: box, padding: pad }} />
    }
    return (
      <div ref={ref as any} className="overflow-hidden" style={{ width: box, height: box, padding: pad }}>
        <AppIcon
          path={file.path}
          size={requestSize}
          className="w-full h-full"
          priority={stage === 'visible' ? 'high' : 'medium'}
          fallback={<AppWindow className="w-14 h-14 text-accent" />}
        />
      </div>
    )
  }

  // Non-image fallback icon (reuse the same padding rule)
  const thumb = Math.max(48, Math.min(tile - pad * 2, 320))
  // Base icons ~48px; target ~50% of thumb at default
  const target = Math.max(32, Math.min(thumb * 0.5, 140))
  const scale = Math.max(0.75, Math.min(2.0, target / 48))
  return (
    <div ref={ref as any} className="rounded-md" style={{ width: thumb, height: thumb, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
      <div style={{ transform: `scale(${scale})`, transformOrigin: 'center' }}>
        {fallbackIcon}
      </div>
    </div>
  )
}

export default function FileGrid({ files, preferences }: FileGridProps) {
  const { selectedFiles, setSelectedFiles, navigateTo } = useAppStore()
  const { renameTargetPath, setRenameTarget, renameFile } = useAppStore()
  const [renameText, setRenameText] = useState<string>('')
  const [draggedFile, setDraggedFile] = useState<string | null>(null)
  const [hoveredFile, setHoveredFile] = useState<string | null>(null)
  const renameInputRef = useRef<HTMLInputElement>(null)

  
  // Tile width from preferences (default 120)
  // Allow full range up to 320 to match ZoomSlider
  const tile = Math.max(80, Math.min(320, preferences.gridSize ?? 120))

  const isMac = typeof navigator !== 'undefined' && navigator.platform.toUpperCase().includes('MAC')

  const getFileIcon = (file: FileItem) => {
    // Special-case: macOS files with system icons
    if (isMac) {
      const fileName = file.name.toLowerCase()
      if (file.is_directory && fileName.endsWith('.app')) {
        return (
          <AppIcon
            path={file.path}
            size={64}
            className="w-16 h-16"
            priority="high"
            fallback={<AppWindow className="w-14 h-14 text-accent" />}
          />
        )
      }
      
      // PKG files use a package icon
      if (fileName.endsWith('.pkg')) {
        return <Package className="w-12 h-12 text-blue-500" weight="fill" />
      }
      
      // DMG files use a custom icon since they don't have embedded icons
      if (fileName.endsWith('.dmg')) {
        return <Disc className="w-12 h-12 text-app-muted" weight="fill" />
      }
    }
    if (file.is_directory) {
      return <Folder className="w-12 h-12 text-accent" weight="fill" />
    }

    const ext = file.extension?.toLowerCase()
    if (!ext) {
      const special = resolveVSCodeIcon(file.name)
      if (special) return <FileTypeIcon name={file.name} size="large" />
      return <File className="w-12 h-12 text-app-muted" />
    }

    // Image files
    if (['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg', 'bmp'].includes(ext)) {
      return <ImageSquare className="w-12 h-12 text-app-green" />
    }

    // PDF files
    if (ext === 'pdf') {
      return <FilePdf className="w-12 h-12 text-red-500" />
    }

    // Adobe Illustrator files
    if (ext === 'ai' || ext === 'eps') {
      return <PaintBrush className="w-12 h-12 text-orange-500" />
    }

    // Photoshop files
    if (ext === 'psd' || ext === 'psb') {
      return <Palette className="w-12 h-12 text-blue-500" />
    }

    // Audio files
    if (['mp3', 'wav', 'flac', 'aac', 'm4a', 'ogg'].includes(ext)) {
      return <MusicNote className="w-12 h-12 text-app-yellow" />
    }

    // Video files
    if (['mp4', 'mkv', 'avi', 'mov', 'webm', 'flv'].includes(ext)) {
      return <VideoCamera className="w-12 h-12 text-app-red" />
    }

    // Archive files
    if (['zip', 'rar', '7z', 'tar', 'gz', 'bz2'].includes(ext)) {
      return <FileZip className="w-12 h-12 text-app-muted" />
    }

    // 3D model: STL
    if (ext === 'stl') {
      return <Cube className="w-12 h-12 text-app-green" />
    }

    // VSCode-style file icons for code/config types
    if (resolveVSCodeIcon(file.name, ext)) {
      return <FileTypeIcon name={file.name} ext={ext} size="large" />
    }

    // Text files
    if (['txt', 'md', 'json', 'xml', 'yml', 'yaml', 'toml', 'ini'].includes(ext)) {
      return <FileText className="w-12 h-12 text-app-text" />
    }

    return <File className="w-12 h-12 text-app-muted" />
  }

  // (moved FilePreview to top-level GridFilePreview to avoid remounting)

  const handleFileClick = (file: FileItem, isCtrlClick = false) => {
    if (isCtrlClick) {
      const newSelection = selectedFiles.includes(file.path)
        ? selectedFiles.filter(path => path !== file.path)
        : [...selectedFiles, file.path]
      setSelectedFiles(newSelection)
    } else {
      // Single click just selects (no navigation)
      setSelectedFiles([file.path])
    }
  }

  const handleDoubleClick = async (file: FileItem) => {
    if (file.is_directory && !file.name.toLowerCase().endsWith('.app')) {
      navigateTo(file.path)
    } else {
      // Open file or app with system default
      try {
        await open(file.path)
      } catch (error) {
        // Fallback to backend command if plugin shell is unavailable/blocked
        try {
          await invoke('open_path', { path: file.path })
        } catch (err2) {
          console.error('Failed to open file:', error, err2)
        }
      }
    }
  }

  // Handle mouse down for drag initiation and right-click selection
  const handleMouseDownForFile = (e: React.MouseEvent, file: FileItem) => {
    // Right-click: Pre-select for context menu
    if (e.button === 2) {
      if (!selectedFiles.includes(file.path)) {
        setSelectedFiles([file.path])
      }
      return
    }
    
    // Left-click: Add drag threshold to prevent accidental drags
    if (e.button === 0) {
      const startX = e.clientX
      const startY = e.clientY
      const dragThreshold = 5 // pixels
      let dragStarted = false

      const startDrag = () => {
        if (dragStarted) return
        dragStarted = true
        
        // Clean up listeners
        document.removeEventListener('mousemove', onMouseMove)
        document.removeEventListener('mouseup', onMouseUp)
        
        // Update selection if dragging a non-selected file
        let actualSelectedFiles = selectedFiles
        if (!selectedFiles.includes(file.path)) {
          actualSelectedFiles = [file.path]
          setSelectedFiles([file.path])
        }
        
        // Determine which files to drag using the actual selection
        const selected = actualSelectedFiles.includes(file.path) && actualSelectedFiles.length > 0
          ? files.filter(f => actualSelectedFiles.includes(f.path))
          : [file]
        
        // Set dragging state for visual feedback
        setDraggedFile(file.path)

        // Perform native drag
        void (async () => {
          try {
            // Create drag preview image with nice icons
            let dragImageDataUrl: string | undefined
            try {
              // Try to use async version to render SVG icons properly
              const dragVisual = await createDragImageForSelectionAsync(selected, document.body)
              dragImageDataUrl = dragVisual.dataUrl
            } catch (e) {
              console.warn('Failed to create async drag image, falling back:', e)
              // Fallback to synchronous version
              try {
                const dragVisual = createDragImageForSelection(selected, document.body)
                dragImageDataUrl = dragVisual.dataUrl
              } catch (e2) {
                console.warn('Failed to create drag image:', e2)
              }
            }
            
            // Use new unified native drag API
            await invoke('start_native_drag', {
              paths: selected.map(f => f.path),
              previewImage: dragImageDataUrl,
              dragOffsetY: 0
            })
          } catch (error) {
            console.warn('Native drag failed:', error)
          } finally {
            // Clear dragging state and hover state
            setDraggedFile(null)
            setHoveredFile(null)
          }
        })()
      }

      const onMouseMove = (moveEvent: MouseEvent) => {
        const deltaX = moveEvent.clientX - startX
        const deltaY = moveEvent.clientY - startY
        const distance = Math.sqrt(deltaX * deltaX + deltaY * deltaY)
        
        if (distance >= dragThreshold) {
          startDrag()
        }
      }

      const onMouseUp = () => {
        document.removeEventListener('mousemove', onMouseMove)
        document.removeEventListener('mouseup', onMouseUp)
      }

      // Add temporary listeners to detect movement
      document.addEventListener('mousemove', onMouseMove)
      document.addEventListener('mouseup', onMouseUp)
    }
  }

  // Begin rename UX when store renameTargetPath points to an item in this view
  useEffect(() => {
    if (!renameTargetPath) return
    const f = files.find(ff => ff.path === renameTargetPath)
    if (!f) return
    setRenameText(f.name)
    requestAnimationFrame(() => {
      const el = renameInputRef.current
      if (el) {
        el.focus()
        const baseLen = (() => {
          if (f.is_directory) return f.name.toLowerCase().endsWith('.app') ? Math.max(0, f.name.length - 4) : f.name.length
          const idx = f.name.lastIndexOf('.')
          return idx > 0 ? idx : f.name.length
        })()
        try { el.setSelectionRange(0, baseLen) } catch {}
      }
    })
  }, [renameTargetPath, files])

  const commitRename = async () => {
    const name = (renameText || '').trim()
    if (!name) { setRenameTarget(undefined); return }
    await renameFile(name)
  }
  const cancelRename = () => setRenameTarget(undefined)

  const nameCollator = useMemo(() => new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' }), [])
  const sortedFiles = [...files].sort((a, b) => {
    // Treat .app as files for sorting purposes
    const aIsApp = a.is_directory && a.name.toLowerCase().endsWith('.app')
    const bIsApp = b.is_directory && b.name.toLowerCase().endsWith('.app')
    const aIsFolder = a.is_directory && !aIsApp
    const bIsFolder = b.is_directory && !bIsApp
    
    // Optionally sort directories first (but not .app files)
    if (preferences.foldersFirst) {
      if (aIsFolder && !bIsFolder) return -1
      if (!aIsFolder && bIsFolder) return 1
    }

    let compareValue = 0
    switch (preferences.sortBy) {
      case 'name':
        compareValue = nameCollator.compare(a.name, b.name)
        break
      case 'size':
        compareValue = a.size - b.size
        break
      case 'modified':
        compareValue = new Date(a.modified).getTime() - new Date(b.modified).getTime()
        break
      case 'type':
        compareValue = (a.extension || '').localeCompare(b.extension || '')
        break
    }

    return preferences.sortOrder === 'asc' ? compareValue : -compareValue
  })

  const filteredFiles = preferences.showHidden 
    ? sortedFiles 
    : sortedFiles.filter(file => !file.is_hidden)
    
  

  if (filteredFiles.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center text-app-muted">
        <div className="text-center">
          <Folder className="w-16 h-16 mx-auto mb-4 opacity-50" />
          <p>This folder is empty</p>
        </div>
      </div>
    )
  }

  const handleBackgroundClick = (e: React.MouseEvent) => {
    // Only clear when the click is directly on the background container
    if (e.target === e.currentTarget) {
      setSelectedFiles([])
    }
  }



  return (
    <div 
      className="p-2 select-none" 
      onClick={handleBackgroundClick}
    >
      <div
        className="grid gap-2 file-grid"
        style={{
        gridTemplateColumns: `repeat(auto-fill, minmax(${tile + 24}px, 1fr))`
        }}
      >
        {filteredFiles.map((file) => {
          const isSelected = selectedFiles.includes(file.path)
          const isDragged = draggedFile !== null && (draggedFile === file.path || selectedFiles.includes(file.path))
          
          return (
            <div
              key={file.path}
              className={`relative flex flex-col items-center px-3 py-2 rounded-md cursor-pointer transition-all duration-75 ${
                isSelected ? 'bg-accent-selected' : hoveredFile === file.path ? 'bg-app-light/70' : ''
              } ${isDragged ? 'opacity-50' : ''} ${
                file.is_hidden ? 'opacity-60' : ''
              }`}
              data-file-item="true"
              data-file-path={file.path}
              data-tauri-drag-region={false}
              onClick={(e) => { e.stopPropagation(); handleFileClick(file, e.ctrlKey || e.metaKey) }}
              onDoubleClick={(e) => { e.stopPropagation(); handleDoubleClick(file) }}
              onMouseDown={(e) => handleMouseDownForFile(e, file)}
              onMouseEnter={() => setHoveredFile(file.path)}
              onMouseLeave={() => setHoveredFile(null)}
              draggable={false}

            >
              <div className="mb-2 flex-shrink-0" style={{ width: tile, display: 'flex', justifyContent: 'center', height: Math.max(48, Math.min(tile - Math.max(3, Math.min(8, Math.round(tile * 0.03))) * 2, 320)) }}>
                <GridFilePreview file={file} isMac={isMac} fallbackIcon={getFileIcon(file)} tile={tile} />
              </div>
              
              <div className="text-center w-full">
                {renameTargetPath === file.path ? (
                  <input
                    ref={renameInputRef}
                    className={`text-sm font-medium bg-app-dark border border-app-border rounded px-2 py-[3px] ${isSelected ? 'text-white' : ''}`}
                    style={{ maxWidth: `${tile}px` }}
                    value={renameText}
                    onChange={(e) => setRenameText(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') { e.preventDefault(); void commitRename() }
                      if (e.key === 'Escape') { e.preventDefault(); cancelRename() }
                    }}
                    onBlur={cancelRename}
                    data-tauri-drag-region={false}
                  />
                ) : (
                  <FileNameDisplay
                    file={file}
                    maxWidth={tile - 16} // Account for padding
                    isSelected={isSelected}
                    variant="grid"
                    showSize={true}
                    style={{ width: '100%' }}
                  />
                )}
              </div>
            </div>
          )
        })}
      </div>
    </div>
  )
}


