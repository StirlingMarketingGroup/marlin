import { useState } from 'react'
import { Folder, File, Image, Music, Video, Archive, Code, FileText } from 'lucide-react'
import { FileItem, ViewPreferences } from '../types'
import { useAppStore } from '../store/useAppStore'

interface FileListProps {
  files: FileItem[]
  preferences: ViewPreferences
}

export default function FileList({ files, preferences }: FileListProps) {
  const { selectedFiles, setSelectedFiles, navigateTo } = useAppStore()
  const [draggedFile, setDraggedFile] = useState<string | null>(null)

  const getFileIcon = (file: FileItem) => {
    if (file.isDirectory) {
      return <Folder className="w-5 h-5 text-discord-accent" />
    }

    const ext = file.extension?.toLowerCase()
    if (!ext) return <File className="w-5 h-5 text-discord-muted" />

    // Same icon logic as FileGrid but smaller
    if (['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg', 'bmp'].includes(ext)) {
      return <Image className="w-5 h-5 text-discord-green" />
    }
    if (['mp3', 'wav', 'flac', 'aac', 'm4a', 'ogg'].includes(ext)) {
      return <Music className="w-5 h-5 text-discord-yellow" />
    }
    if (['mp4', 'mkv', 'avi', 'mov', 'webm', 'flv'].includes(ext)) {
      return <Video className="w-5 h-5 text-discord-red" />
    }
    if (['zip', 'rar', '7z', 'tar', 'gz', 'bz2'].includes(ext)) {
      return <Archive className="w-5 h-5 text-discord-muted" />
    }
    if (['js', 'ts', 'jsx', 'tsx', 'py', 'rs', 'go', 'java', 'cpp', 'c', 'h'].includes(ext)) {
      return <Code className="w-5 h-5 text-discord-accent" />
    }
    if (['txt', 'md', 'json', 'xml', 'yml', 'yaml'].includes(ext)) {
      return <FileText className="w-5 h-5 text-discord-text" />
    }

    return <File className="w-5 h-5 text-discord-muted" />
  }

  const handleFileClick = (file: FileItem, isCtrlClick = false) => {
    if (isCtrlClick) {
      const newSelection = selectedFiles.includes(file.path)
        ? selectedFiles.filter(path => path !== file.path)
        : [...selectedFiles, file.path]
      setSelectedFiles(newSelection)
    } else if (file.isDirectory) {
      navigateTo(file.path)
    } else {
      setSelectedFiles([file.path])
    }
  }

  const handleDoubleClick = (file: FileItem) => {
    if (file.isDirectory) {
      navigateTo(file.path)
    } else {
      // TODO: Open file with system default app
      console.log('Open file:', file.path)
    }
  }

  const sortedFiles = [...files].sort((a, b) => {
    // Directories first
    if (a.isDirectory && !b.isDirectory) return -1
    if (!a.isDirectory && b.isDirectory) return 1

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
    : sortedFiles.filter(file => !file.isHidden)

  if (filteredFiles.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center text-discord-muted">
        <div className="text-center">
          <Folder className="w-16 h-16 mx-auto mb-4 opacity-50" />
          <p>This folder is empty</p>
        </div>
      </div>
    )
  }

  return (
    <div className="flex-1 overflow-auto">
      {/* Header */}
      <div className="grid grid-cols-12 gap-4 px-4 py-3 border-b border-discord-border text-sm font-medium text-discord-muted bg-discord-gray/50">
        <div className="col-span-6">Name</div>
        <div className="col-span-2">Size</div>
        <div className="col-span-2">Type</div>
        <div className="col-span-2">Modified</div>
      </div>

      {/* File rows */}
      <div className="divide-y divide-discord-border/50">
        {filteredFiles.map((file) => {
          const isSelected = selectedFiles.includes(file.path)
          const isDragged = draggedFile === file.path
          
          return (
            <div
              key={file.path}
              className={`grid grid-cols-12 gap-4 px-4 py-2 hover:bg-discord-light cursor-pointer transition-colors ${
                isSelected ? 'bg-discord-accent/20' : ''
              } ${isDragged ? 'opacity-50' : ''} ${
                file.isHidden ? 'opacity-60' : ''
              }`}
              onClick={(e) => handleFileClick(file, e.ctrlKey || e.metaKey)}
              onDoubleClick={() => handleDoubleClick(file)}
              onDragStart={() => setDraggedFile(file.path)}
              onDragEnd={() => setDraggedFile(null)}
              draggable
            >
              {/* Name column */}
              <div className="col-span-6 flex items-center gap-3 min-w-0">
                {getFileIcon(file)}
                <span className="truncate text-sm" title={file.name}>
                  {file.name}
                </span>
              </div>

              {/* Size column */}
              <div className="col-span-2 flex items-center text-sm text-discord-muted">
                {file.isDirectory ? '—' : formatFileSize(file.size)}
              </div>

              {/* Type column */}
              <div className="col-span-2 flex items-center text-sm text-discord-muted">
                {file.isDirectory ? 'Folder' : (file.extension?.toUpperCase() || 'File')}
              </div>

              {/* Modified column */}
              <div className="col-span-2 flex items-center text-sm text-discord-muted">
                {formatDate(file.modified)}
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

function formatDate(date: Date): string {
  const now = new Date()
  const diffMs = now.getTime() - new Date(date).getTime()
  const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24))
  
  if (diffDays === 0) {
    return new Intl.DateTimeFormat('en', {
      hour: '2-digit',
      minute: '2-digit'
    }).format(date)
  } else if (diffDays < 7) {
    return `${diffDays} days ago`
  } else {
    return new Intl.DateTimeFormat('en', {
      month: 'short',
      day: 'numeric',
      year: diffDays > 365 ? 'numeric' : undefined
    }).format(date)
  }
}