import { useEffect, useState } from 'react'
import { Folder, File, ImageSquare, MusicNote, VideoCamera, FileZip, Code, FileText, CaretUp, CaretDown, AppWindow, HardDrive } from 'phosphor-react'
import { FileItem, ViewPreferences } from '../types'
import { useAppStore } from '../store/useAppStore'
import AppIcon from '@/components/AppIcon'
import { open } from '@tauri-apps/plugin-shell'
import { useThumbnail } from '@/hooks/useThumbnail'

interface FileListProps {
  files: FileItem[]
  preferences: ViewPreferences
}

export default function FileList({ files, preferences }: FileListProps) {
  const { selectedFiles, setSelectedFiles, navigateTo } = useAppStore()
  const { fetchAppIcon } = useAppStore()
  const [draggedFile, setDraggedFile] = useState<string | null>(null)

  const sortBy = preferences.sortBy
  const sortOrder = preferences.sortOrder
  const toggleSort = (field: typeof preferences.sortBy) => {
    const { updateDirectoryPreferences } = useAppStore.getState()
    if (sortBy === field) {
      updateDirectoryPreferences(useAppStore.getState().currentPath, {
        sortOrder: sortOrder === 'asc' ? 'desc' : 'asc'
      })
    } else {
      updateDirectoryPreferences(useAppStore.getState().currentPath, {
        sortBy: field,
        sortOrder: 'asc'
      })
    }
  }

  const isMac = typeof navigator !== 'undefined' && navigator.platform.toUpperCase().includes('MAC')

  // Optionally warm cache for a small first screenful
  useEffect(() => {
    if (!isMac) return
    const initial = files.filter(f => {
      const fileName = f.name.toLowerCase()
      return (f.is_directory && fileName.endsWith('.app')) || 
             fileName.endsWith('.pkg')
    }).slice(0, 6)
    initial.forEach(f => { void fetchAppIcon(f.path, 64) })
  }, [isMac, files, fetchAppIcon])

  const getFileIcon = (file: FileItem) => {
    if (isMac) {
      const fileName = file.name.toLowerCase()
      if ((file.is_directory && fileName.endsWith('.app')) || 
          fileName.endsWith('.pkg')) {
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
      
      // DMG files use a custom icon since they don't have embedded icons
      if (fileName.endsWith('.dmg')) {
        return <HardDrive className="w-5 h-5 text-orange-500" weight="fill" />
      }
    }
    if (file.is_directory) {
      return <Folder className="w-5 h-5 text-accent" weight="fill" />
    }

    const ext = file.extension?.toLowerCase()
    if (!ext) return <File className="w-5 h-5 text-app-muted" />

    // Same icon logic as FileGrid but smaller
    if (['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg', 'bmp'].includes(ext)) {
      return <ImageSquare className="w-5 h-5 text-app-green" />
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
    if (['js', 'ts', 'jsx', 'tsx', 'py', 'rs', 'go', 'java', 'cpp', 'c', 'h'].includes(ext)) {
      return <Code className="w-5 h-5 text-accent" />
    }
    if (['txt', 'md', 'json', 'xml', 'yml', 'yaml'].includes(ext)) {
      return <FileText className="w-5 h-5 text-app-text" />
    }

    return <File className="w-5 h-5 text-app-muted" />
  }

  function FilePreview({ file }: { file: FileItem }) {
    const ext = file.extension?.toLowerCase()
    const isImage = !!ext && ['jpg', 'jpeg', 'png', 'gif', 'webp', 'bmp', 'tiff', 'tga', 'ico'].includes(ext)

    // Prefer native app icons/DMG on macOS
    if (isMac) {
      const fileName = file.name.toLowerCase()
      if ((file.is_directory && fileName.endsWith('.app')) || fileName.endsWith('.pkg')) {
        return (
          <AppIcon
            path={file.path}
            size={64}
            className="w-5 h-5"
            rounded={false}
            priority="medium"
            fallback={<AppWindow className="w-5 h-5 text-accent" />}
          />
        )
      }
      if (fileName.endsWith('.dmg')) {
        return <HardDrive className="w-5 h-5 text-orange-500" weight="fill" />
      }
    }

    if (isImage) {
      const { dataUrl, loading } = useThumbnail(file.path, { size: 64, quality: 'medium', priority: 'medium', format: 'png' })
      if (dataUrl) {
        return <img src={dataUrl} alt="" className="w-5 h-5 rounded-sm object-cover border border-app-border bg-app-darker" draggable={false} />
      }
      if (loading) {
        return <div className="w-5 h-5 rounded-sm border border-app-border bg-app-darker animate-pulse" />
      }
      return <ImageSquare className="w-5 h-5 text-app-green" />
    }

    return getFileIcon(file)
  }

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
    // Directories first
    if (a.is_directory && !b.is_directory) return -1
    if (!a.is_directory && b.is_directory) return 1

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
          const isDragged = draggedFile === file.path
          
          return (
            <div
              key={file.path}
              className={`grid grid-cols-12 gap-3 py-[2px] leading-5 text-[13px] cursor-pointer transition-colors rounded-full ${
                isSelected ? 'bg-accent-soft outline outline-1 outline-accent' : 'odd:bg-app-gray'
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
              {/* Name column */}
              <div className="col-span-5 flex items-center gap-2 min-w-0 pl-2">
                <span className="flex-shrink-0">
                  <FilePreview file={file} />
                </span>
                <span className="truncate text-sm" title={file.name}>
                  {(isMac && file.is_directory && file.name.toLowerCase().endsWith('.app'))
                    ? file.name.replace(/\.app$/i, '')
                    : file.name}
                </span>
              </div>

              {/* Size column */}
              <div className="col-span-2 flex items-center text-app-muted">
                {file.is_directory ? '—' : formatFileSize(file.size)}
              </div>

              {/* Type column */}
              <div className="col-span-2 flex items-center text-app-muted">
                {file.is_directory ? 'Folder' : (file.extension?.toUpperCase() || 'File')}
              </div>

              {/* Modified column */}
              <div className="col-span-3 flex items-center text-app-muted whitespace-nowrap">
                {formatDateFull(file.modified)}
              </div>
            </div>
          )
        })}
      </div>
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
