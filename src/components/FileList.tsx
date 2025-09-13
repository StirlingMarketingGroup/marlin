import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { Folder, File, ImageSquare, MusicNote, VideoCamera, FileZip, FileText, CaretUp, CaretDown, AppWindow, Package, FilePdf, PaintBrush, Palette, Disc, Cube } from 'phosphor-react'
import { FileItem, ViewPreferences } from '../types'
import { useAppStore } from '../store/useAppStore'
import AppIcon from '@/components/AppIcon'
import { FileTypeIcon, resolveVSCodeIcon } from '@/components/FileTypeIcon'
import { open } from '@tauri-apps/plugin-shell'

import { createDragImageForSelection, createDragImageForSelectionAsync } from '@/utils/dragImage'
import { invoke } from '@tauri-apps/api/core'
// no direct invoke here; background opens the menu
import { useThumbnail } from '@/hooks/useThumbnail'
import { useVisibility } from '@/hooks/useVisibility'
import FileNameDisplay from './FileNameDisplay'

interface FileListProps {
  files: FileItem[]
  preferences: ViewPreferences
}

// Stable, top-level preview component to avoid remount flicker
function ListFilePreview({ file, isMac, fallbackIcon }: { file: FileItem; isMac: boolean; fallbackIcon: ReactNode }) {
  const { ref, stage } = useVisibility({ nearMargin: '800px', visibleMargin: '0px' })
  const ext = file.extension?.toLowerCase()
  const isImage = !!ext && ['jpg', 'jpeg', 'png', 'gif', 'webp', 'bmp', 'tiff', 'tga', 'ico', 'svg'].includes(ext || '')
  const isPdf = ext === 'pdf'
  const isAi = ext === 'ai' || ext === 'eps'
  const isPsd = ext === 'psd' || ext === 'psb'
  const isSvg = ext === 'svg'
  const isStl = ext === 'stl'

  if (isMac) {
    const fileName = file.name.toLowerCase()
    if (file.is_directory && fileName.endsWith('.app')) {
      return (
        <AppIcon
          path={file.path}
          size={64}
          className="w-5 h-5"
          rounded={false}
          priority="high"
          fallback={<AppWindow className="w-5 h-5 text-accent" />}
        />
      )
    }
    if (fileName.endsWith('.pkg')) {
      return <Package className="w-5 h-5 text-blue-500" weight="fill" />
    }
    if (fileName.endsWith('.dmg')) {
      return <Disc className="w-5 h-5 text-app-muted" weight="fill" />
    }
  }

  if (isImage || isPdf || isAi || isPsd || isStl) {
    const dpr = typeof window !== 'undefined' ? Math.min(2, Math.max(1, window.devicePixelRatio || 1)) : 1
    const shouldLoad = stage !== 'far'
    const priority = stage === 'visible' ? 'high' : 'medium'
    const { dataUrl, loading, hasTransparency } = useThumbnail(shouldLoad ? file.path : undefined, { size: Math.round(64 * dpr), quality: 'medium', priority })
    if (dataUrl) {
      const isRaster = isImage && !isSvg
      return (
        <div ref={ref as any} className={`w-5 h-5 rounded-sm border border-app-border ${hasTransparency ? 'bg-checker' : ''} ${isRaster ? '' : 'p-[1px]'} overflow-hidden`}>
          <img
            src={dataUrl}
            alt=""
            className={`w-full h-full`}
            style={{ objectFit: isRaster ? 'contain' as const : 'contain' as const, transform: 'none' }}
            onLoad={(e) => {
              if (!isRaster) return
              const img = e.currentTarget as HTMLImageElement
              const iw = img.naturalWidth || 1
              const ih = img.naturalHeight || 1
              const r = iw / ih
              if (r > 1/1.10 && r < 1.10) {
                img.style.objectFit = 'cover'
                img.style.transform = 'scale(1.01)'
              } else {
                img.style.objectFit = 'contain'
                img.style.transform = 'none'
              }
            }}
            draggable={false}
          />
        </div>
      )
    }
    if (loading) {
      return <div ref={ref as any} className="w-5 h-5 rounded-sm border border-app-border animate-pulse" />
    }
    return <div ref={ref as any} className="w-5 h-5 rounded-sm border border-app-border" />
  }

  return <span ref={ref as any}>{fallbackIcon}</span>
}

export default function FileList({ files, preferences }: FileListProps) {
  const { selectedFiles, setSelectedFiles, navigateTo } = useAppStore()
  const { renameTargetPath, setRenameTarget, renameFile } = useAppStore()
  const [renameText, setRenameText] = useState<string>('')
  const renameInputRef = useRef<HTMLInputElement>(null)
  const { fetchAppIcon } = useAppStore()
  const [draggedFile, setDraggedFile] = useState<string | null>(null)
  const [hoveredFile, setHoveredFile] = useState<string | null>(null)

  const sortBy = preferences.sortBy
  const sortOrder = preferences.sortOrder
  const toggleSort = (field: typeof preferences.sortBy) => {
    const { updateDirectoryPreferences } = useAppStore.getState()
    if (sortBy === field) {
      updateDirectoryPreferences(useAppStore.getState().currentPath, {
        sortOrder: sortOrder === 'asc' ? 'desc' : 'asc'
      })
    } else {
      const defaultOrder: 'asc' | 'desc' = (field === 'size' || field === 'modified') ? 'desc' : 'asc'
      updateDirectoryPreferences(useAppStore.getState().currentPath, {
        sortBy: field,
        sortOrder: defaultOrder
      })
    }
  }

  const isMac = typeof navigator !== 'undefined' && navigator.platform.toUpperCase().includes('MAC')

  // Optionally warm cache for a small first screenful
  useEffect(() => {
    if (!isMac) return
    const initial = files.filter(f => {
      const fileName = f.name.toLowerCase()
      return f.is_directory && fileName.endsWith('.app')
    }).slice(0, 6)
    initial.forEach(f => { void fetchAppIcon(f.path, 64) })
  }, [isMac, files, fetchAppIcon])

  const getFileIcon = (file: FileItem) => {
    if (isMac) {
      const fileName = file.name.toLowerCase()
      if (file.is_directory && fileName.endsWith('.app')) {
        return (
          <AppIcon
            path={file.path}
            size={64}
            className="w-5 h-5"
            rounded={false}
            priority="high"
            fallback={<AppWindow className="w-5 h-5 text-accent" />}
          />
        )
      }
      
      // PKG files use a package icon
      if (fileName.endsWith('.pkg')) {
        return <Package className="w-5 h-5 text-blue-500" weight="fill" />
      }
      
      // DMG files use a custom icon since they don't have embedded icons
      if (fileName.endsWith('.dmg')) {
        return <Disc className="w-5 h-5 text-app-muted" weight="fill" />
      }
    }
    if (file.is_directory) {
      return <Folder className="w-5 h-5 text-accent" weight="fill" />
    }

    const ext = file.extension?.toLowerCase()
    if (!ext) {
      const special = resolveVSCodeIcon(file.name)
      if (special) return <FileTypeIcon name={file.name} size="small" />
      return <File className="w-5 h-5 text-app-muted" />
    }

    // Same icon logic as FileGrid but smaller
    if (['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg', 'bmp'].includes(ext)) {
      return <ImageSquare className="w-5 h-5 text-app-green" />
    }
    if (ext === 'pdf') {
      return <FilePdf className="w-5 h-5 text-red-500" />
    }
    if (ext === 'ai' || ext === 'eps') {
      return <PaintBrush className="w-5 h-5 text-orange-500" />
    }
    if (ext === 'psd' || ext === 'psb') {
      return <Palette className="w-5 h-5 text-blue-500" />
    }
    if (['mp3', 'wav', 'flac', 'aac', 'm4a', 'ogg'].includes(ext)) {
      return <MusicNote className="w-5 h-5 text-app-yellow" />
    }
    if (['mp4', 'mkv', 'avi', 'mov', 'webm', 'flv'].includes(ext)) {
      return <VideoCamera className="w-5 h-5 text-app-red" />
    }
    if (['zip', 'rar', '7z', 'tar', 'gz', 'bz2'].includes(ext)) {
      return <FileZip className="w-5 h-5 text-app-muted" />
    }
    // 3D model: STL
    if (ext === 'stl') {
      return <Cube className="w-5 h-5 text-app-green" />
    }
    // VSCode-style file icons for code/config types
    if (resolveVSCodeIcon(file.name, ext)) {
      return <FileTypeIcon name={file.name} ext={ext} size="small" />
    }

    if (['txt'].includes(ext)) {
      return <FileText className="w-5 h-5 text-app-text" />
    }
    if (['md', 'json', 'xml', 'yml', 'yaml', 'toml', 'ini'].includes(ext)) {
      return <FileText className="w-5 h-5 text-app-text" />
    }

    return <File className="w-5 h-5 text-app-muted" />
  }

  // (moved FilePreview to top-level ListFilePreview to avoid remounting)

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

  // Begin rename UX when store renameTargetPath points to an item in this view
  useEffect(() => {
    if (!renameTargetPath) return
    const f = files.find(ff => ff.path === renameTargetPath)
    if (!f) return
    setRenameText(f.name)
    const baseLen = (() => {
      if (f.is_directory) {
        return f.name.toLowerCase().endsWith('.app') ? Math.max(0, f.name.length - 4) : f.name.length
      }
      const idx = f.name.lastIndexOf('.')
      return idx > 0 ? idx : f.name.length
    })()
    // Focus/select on next frame after input mounts
    requestAnimationFrame(() => {
      const el = renameInputRef.current
      if (el) {
        el.focus()
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
    <>
    <div className="h-full" onClick={handleBackgroundClick}>
      {/* Header */}
      <div className="grid grid-cols-12 gap-3 px-3 py-2 border-b border-app-border border-t-0 text-[12px] font-medium text-app-muted bg-transparent select-none mb-1">
        <button className={`col-span-5 text-left hover:text-app-text pl-2 ${sortBy === 'name' ? 'text-app-text' : ''}`} onClick={() => toggleSort('name')} data-tauri-drag-region={false}>
          <span className="inline-flex items-center gap-1">Name {sortBy === 'name' && (sortOrder === 'asc' ? <CaretUp className="w-3 h-3"/> : <CaretDown className="w-3 h-3"/> )}</span>
        </button>
        <button className={`col-span-2 text-left hover:text-app-text ${sortBy === 'size' ? 'text-app-text' : ''}`} onClick={() => toggleSort('size')} data-tauri-drag-region={false}>
          <span className="inline-flex items-center gap-1">Size {sortBy === 'size' && (sortOrder === 'asc' ? <CaretUp className="w-3 h-3"/> : <CaretDown className="w-3 h-3"/> )}</span>
        </button>
        <button className={`col-span-2 text-left hover:text-app-text ${sortBy === 'type' ? 'text-app-text' : ''}`} onClick={() => toggleSort('type')} data-tauri-drag-region={false}>
          <span className="inline-flex items-center gap-1">Type {sortBy === 'type' && (sortOrder === 'asc' ? <CaretUp className="w-3 h-3"/> : <CaretDown className="w-3 h-3"/> )}</span>
        </button>
        <button className={`col-span-3 text-left hover:text-app-text ${sortBy === 'modified' ? 'text-app-text' : ''}`} onClick={() => toggleSort('modified')} data-tauri-drag-region={false}>
          <span className="inline-flex items-center gap-1">Modified {sortBy === 'modified' && (sortOrder === 'asc' ? <CaretUp className="w-3 h-3"/> : <CaretDown className="w-3 h-3"/> )}</span>
        </button>
      </div>

      {/* File rows */}
      <div className="space-y-[2px] px-3 py-1 mt-1">
        {filteredFiles.map((file) => {
          const isSelected = selectedFiles.includes(file.path)
          const isDragged = draggedFile !== null && (draggedFile === file.path || selectedFiles.includes(file.path))
          
          return (
            <div
              key={file.path}
              className={`relative grid grid-cols-12 gap-3 py-[2px] leading-5 text-[13px] cursor-pointer transition-colors duration-75 rounded-full ${
                isSelected ? 'bg-accent-selected text-white' : hoveredFile === file.path ? 'bg-app-light' : 'odd:bg-app-gray'
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
              {/* Name column */}
              <div className="col-span-5 flex items-center gap-2 min-w-0 pl-2 pr-2">
                <span className="flex-shrink-0">
                  <ListFilePreview file={file} isMac={isMac} fallbackIcon={getFileIcon(file)} />
                </span>
                {renameTargetPath === file.path ? (
                  <input
                    ref={renameInputRef}
                    className={`flex-1 min-w-0 text-sm bg-app-dark border border-app-border rounded px-2 py-[3px] outline-none ${isSelected ? 'text-white' : 'text-app-text'} focus:border-[var(--accent)] truncate`}
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
                  <div className="flex-1 min-w-0 overflow-hidden" data-name-cell="true">
                    <FileNameDisplay
                      file={file}
                      isSelected={isSelected}
                      variant="list"
                      className="block"
                    />
                  </div>
                )}
              </div>

              {/* Size column */}
              <div className={`col-span-2 flex items-center ${isSelected ? 'text-white' : 'text-app-muted'}`}>
                {file.is_directory ? '—' : formatFileSize(file.size)}
              </div>

              {/* Type column */}
              <div className={`col-span-2 flex items-center ${isSelected ? 'text-white' : 'text-app-muted'}`}>
                {file.is_directory && file.name.toLowerCase().endsWith('.app') 
                  ? 'Application' 
                  : file.is_directory 
                    ? 'Folder' 
                    : (file.extension?.toUpperCase() || 'File')}
              </div>

              {/* Modified column */}
              <div className={`col-span-3 flex items-center ${isSelected ? 'text-white' : 'text-app-muted'} whitespace-nowrap`}>
                {formatDateFull(file.modified)}
              </div>
            </div>
          )
        })}
      </div>
    </div>
    </>
  )
}

function formatFileSize(bytes: number): string {
  if (bytes === 0) return '0 B'
  
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i]
}

function formatDateFull(dateString: string): string {
  const date = new Date(dateString)
  return new Intl.DateTimeFormat(undefined, {
    year: 'numeric',
    month: 'short',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit'
  }).format(date)
}
