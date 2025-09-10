import { useEffect, useRef, useState, type ReactNode } from 'react'
import { Folder, File, ImageSquare, MusicNote, VideoCamera, FileZip, FileText, AppWindow, Package, FilePdf, PaintBrush, Palette, Disc } from 'phosphor-react'
import { FileItem, ViewPreferences } from '../types'
import { useAppStore } from '../store/useAppStore'
import AppIcon from '@/components/AppIcon'
import { FileTypeIcon, resolveVSCodeIcon } from '@/components/FileTypeIcon'
import { open } from '@tauri-apps/plugin-shell'
import { useThumbnail } from '@/hooks/useThumbnail'
import { truncateMiddle } from '@/utils/truncate'

interface FileGridProps {
  files: FileItem[]
  preferences: ViewPreferences
}

// Stable, top-level preview component to avoid remount flicker
function GridFilePreview({ file, isMac, fallbackIcon }: { file: FileItem; isMac: boolean; fallbackIcon: ReactNode }) {
  const ext = file.extension?.toLowerCase()
  const isImage = !!ext && ['jpg', 'jpeg', 'png', 'gif', 'webp', 'bmp', 'tiff', 'tga', 'ico', 'svg'].includes(ext || '')
  const isPdf = ext === 'pdf'
  const isAi = ext === 'ai' || ext === 'eps'
  const isPsd = ext === 'psd' || ext === 'psb'

  // Prefer native app icons/DMG if applicable
  if (isMac) {
    const fileName = file.name.toLowerCase()
    if (file.is_directory && fileName.endsWith('.app')) {
      return (
        <AppIcon
          path={file.path}
          size={96}
          className="w-16 h-16"
          priority="high"
          fallback={<AppWindow className="w-14 h-14 text-accent" />}
        />
      )
    }
    if (fileName.endsWith('.pkg')) {
      return <Package className="w-12 h-12 text-blue-500" weight="fill" />
    }
    if (fileName.endsWith('.dmg')) {
      return <Disc className="w-12 h-12 text-app-muted" weight="fill" />
    }
  }

    if (isImage || isPdf || isAi || isPsd) {
    const { dataUrl, loading } = useThumbnail(file.path, { size: 192, quality: 'medium', priority: 'high', format: 'png' })
    if (dataUrl) {
      return (
        <img
          src={dataUrl}
          alt={file.name}
          className="w-16 h-16 rounded-md object-cover border border-app-border bg-checker"
          draggable={false}
        />
      )
    }
    if (loading) {
      return <div className="w-16 h-16 rounded-md border border-app-border bg-checker animate-pulse" />
    }
  }

  // Non-image fallback icon
  return <>{fallbackIcon}</>
}

export default function FileGrid({ files, preferences }: FileGridProps) {
  const { selectedFiles, setSelectedFiles, navigateTo } = useAppStore()
  const [draggedFile, setDraggedFile] = useState<string | null>(null)
  
  // Dynamically compute a safe middle-truncation length for grid captions
  const gridMeasureSpanRef = useRef<HTMLSpanElement>(null)
  const gridLabelProbeRef = useRef<HTMLDivElement>(null)
  const [gridNameCharLimit, setGridNameCharLimit] = useState<number>(30)

  useEffect(() => {
    const recalc = () => {
      const probe = gridLabelProbeRef.current
      const measure = gridMeasureSpanRef.current
      if (!probe || !measure) return

      const width = Math.max(0, probe.getBoundingClientRect().width || 120)
      const sample = measure.textContent || ''
      const sampleWidth = measure.getBoundingClientRect().width || 7.5 * sample.length
      const avgChar = sampleWidth / Math.max(1, sample.length)

      // Two lines worth of characters, with a small safety buffer
      const perLine = Math.max(6, Math.floor(width / Math.max(5, avgChar)))
      const total = Math.max(10, perLine * 2 - 4)
      setGridNameCharLimit(total)
    }

    recalc()
    let ro: ResizeObserver | undefined
    if (typeof ResizeObserver !== 'undefined' && gridLabelProbeRef.current) {
      ro = new ResizeObserver(() => recalc())
      ro.observe(gridLabelProbeRef.current)
    }
    window.addEventListener('resize', recalc)
    return () => {
      window.removeEventListener('resize', recalc)
      if (ro) ro.disconnect()
    }
  }, [])

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
          const { invoke } = await import('@tauri-apps/api/core')
          await invoke('open_path', { path: file.path })
        } catch (err2) {
          console.error('Failed to open file:', error, err2)
        }
      }
    }
  }

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
        compareValue = a.name.localeCompare(b.name)
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
    <div className="p-2 select-none" onClick={handleBackgroundClick}>
      <div className="grid gap-2" style={{
        gridTemplateColumns: 'repeat(auto-fill, minmax(120px, 1fr))'
      }}>
        {filteredFiles.map((file) => {
          const isSelected = selectedFiles.includes(file.path)
          const isDragged = draggedFile === file.path
          
          return (
            <div
              key={file.path}
              className={`relative flex flex-col items-center px-1 py-2 rounded-md cursor-pointer transition-colors duration-75 ${
                isSelected ? 'bg-accent-selected' : 'hover:bg-app-light/70'
              } ${isDragged ? 'opacity-50' : ''} ${
                file.is_hidden ? 'opacity-60' : ''
              }`}
              data-tauri-drag-region={false}
              onClick={(e) => { e.stopPropagation(); handleFileClick(file, e.ctrlKey || e.metaKey) }}
              onDoubleClick={(e) => { e.stopPropagation(); handleDoubleClick(file) }}
              onDragStart={() => setDraggedFile(file.path)}
              onDragEnd={() => setDraggedFile(null)}
              draggable
            >
              <div className="mb-2 flex-shrink-0">
                <GridFilePreview file={file} isMac={isMac} fallbackIcon={getFileIcon(file)} />
              </div>
              
              <div className={`text-center`}>
                {(() => {
                  const raw = (isMac && file.is_directory && file.name.toLowerCase().endsWith('.app'))
                    ? file.name.replace(/\.app$/i, '')
                    : file.name
                  
                  const needsTruncation = raw.length > gridNameCharLimit
                  const needsExpansion = needsTruncation && isSelected
                  
                  return (
                    <>
                      <div 
                        className={`text-sm font-medium ${isSelected ? 'text-accent' : ''} ${needsExpansion ? 'relative z-10' : ''}`}
                        style={{
                          wordBreak: 'break-word',
                          maxWidth: '120px',
                          height: needsExpansion ? 'auto' : '2.5rem',
                          lineHeight: '1.25rem',
                          overflow: needsExpansion ? 'visible' : 'hidden',
                          textAlign: 'center',
                          ...(needsExpansion && {
                            backgroundColor: 'rgb(30 30 30 / 0.95)',
                            padding: '2px 4px',
                            borderRadius: '4px',
                            position: 'absolute',
                            left: '50%',
                            transform: 'translateX(-50%)',
                            minWidth: '120px',
                            zIndex: 20,
                            height: 'auto'
                          })
                        }}
                        ref={(el) => { if (!gridLabelProbeRef.current) gridLabelProbeRef.current = el! }}
                        title={file.name}
                      >
                        {(() => {
                          // When selected AND long, show full name
                          if (needsExpansion) {
                            return raw
                          }
                          
                          // If it fits naturally, show it as-is
                          if (!needsTruncation) {
                            return raw
                          }
                          
                          // Otherwise use middle truncation to ensure extension is visible
                          return truncateMiddle(raw, gridNameCharLimit)
                        })()}
                      </div>
                      {!file.is_directory && !needsExpansion && (
                        <div className="text-xs text-app-muted mt-1">
                          {formatFileSize(file.size)}
                        </div>
                      )}
                    </>
                  )
                })()}
              </div>
            </div>
          )
        })}
      </div>
      {/* Hidden measurement element for accurate char width */}
      <span
        ref={gridMeasureSpanRef}
        className="absolute -left-[9999px] -top-[9999px] whitespace-nowrap text-sm"
        aria-hidden
      >
        ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789
      </span>
    </div>
  )
}

function formatFileSize(bytes: number): string {
  if (bytes === 0) return '0 B'
  
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i]
}
