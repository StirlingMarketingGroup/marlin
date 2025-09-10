import { useState } from 'react'
import { Folder, File, ImageSquare, MusicNote, VideoCamera, FileZip, Code, FileText, AppWindow } from 'phosphor-react'
import { FileItem, ViewPreferences } from '../types'
import { useAppStore } from '../store/useAppStore'
import AppIcon from '@/components/AppIcon'

interface FileGridProps {
  files: FileItem[]
  preferences: ViewPreferences
}

export default function FileGrid({ files, preferences }: FileGridProps) {
  const { selectedFiles, setSelectedFiles, navigateTo } = useAppStore()
  const [draggedFile, setDraggedFile] = useState<string | null>(null)

  const isMac = typeof navigator !== 'undefined' && navigator.platform.toUpperCase().includes('MAC')

  const getFileIcon = (file: FileItem) => {
    // Special-case: .app bundles on macOS (any folder)
    if (isMac && file.is_directory && file.name.toLowerCase().endsWith('.app')) {
      return (
        <AppIcon
          path={file.path}
          size={64}
          className="w-12 h-12"
          fallback={<AppWindow className="w-10 h-10 text-accent" />}
        />
      )
    }
    if (file.is_directory) {
      return <Folder className="w-8 h-8 text-accent" weight="fill" />
    }

    const ext = file.extension?.toLowerCase()
    if (!ext) return <File className="w-8 h-8 text-app-muted" />

    // Image files
    if (['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg', 'bmp'].includes(ext)) {
      return <ImageSquare className="w-8 h-8 text-app-green" />
    }

    // Audio files
    if (['mp3', 'wav', 'flac', 'aac', 'm4a', 'ogg'].includes(ext)) {
      return <MusicNote className="w-8 h-8 text-app-yellow" />
    }

    // Video files
    if (['mp4', 'mkv', 'avi', 'mov', 'webm', 'flv'].includes(ext)) {
      return <VideoCamera className="w-8 h-8 text-app-red" />
    }

    // Archive files
    if (['zip', 'rar', '7z', 'tar', 'gz', 'bz2'].includes(ext)) {
      return <FileZip className="w-8 h-8 text-app-muted" />
    }

    // Code files
    if (['js', 'ts', 'jsx', 'tsx', 'py', 'rs', 'go', 'java', 'cpp', 'c', 'h'].includes(ext)) {
      return <Code className="w-8 h-8 text-accent" />
    }

    // Text files
    if (['txt', 'md', 'json', 'xml', 'yml', 'yaml'].includes(ext)) {
      return <FileText className="w-8 h-8 text-app-text" />
    }

    return <File className="w-8 h-8 text-app-muted" />
  }

  const handleFileClick = (file: FileItem, isCtrlClick = false) => {
    if (isCtrlClick) {
      const newSelection = selectedFiles.includes(file.path)
        ? selectedFiles.filter(path => path !== file.path)
        : [...selectedFiles, file.path]
      setSelectedFiles(newSelection)
    } else if (file.is_directory) {
      navigateTo(file.path)
    } else {
      setSelectedFiles([file.path])
    }
  }

  const handleDoubleClick = (file: FileItem) => {
    if (file.is_directory) {
      navigateTo(file.path)
    } else {
      // TODO: Open file with system default app
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

  return (
    <div className="p-4 select-none">
      <div className="grid gap-4" style={{
        gridTemplateColumns: 'repeat(auto-fill, minmax(120px, 1fr))'
      }}>
        {filteredFiles.map((file) => {
          const isSelected = selectedFiles.includes(file.path)
          const isDragged = draggedFile === file.path
          
          return (
            <div
              key={file.path}
              className={`flex flex-col items-center p-3 rounded-md cursor-pointer transition-colors hover:bg-app-light ${
                isSelected ? 'bg-accent-soft outline outline-1 outline-accent' : ''
              } ${isDragged ? 'opacity-50' : ''} ${
                file.is_hidden ? 'opacity-60' : ''
              }`}
              data-tauri-drag-region={false}
              onClick={(e) => handleFileClick(file, e.ctrlKey || e.metaKey)}
              onDoubleClick={() => handleDoubleClick(file)}
              onDragStart={() => setDraggedFile(file.path)}
              onDragEnd={() => setDraggedFile(null)}
              draggable
            >
              <div className="mb-2">
                {getFileIcon(file)}
              </div>
              
              <div className="text-center">
                <div className="text-sm font-medium w-full max-w-[120px] line-clamp-2" title={file.name}>
                  {(isMac && file.is_directory && file.name.toLowerCase().endsWith('.app'))
                    ? file.name.replace(/\.app$/i, '')
                    : file.name}
                </div>
                {!file.is_directory && (
                  <div className="text-xs text-app-muted mt-1">
                    {formatFileSize(file.size)}
                  </div>
                )}
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
