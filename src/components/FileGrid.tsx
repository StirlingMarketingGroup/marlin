import { useState } from 'react'
import { Folder, File, Image, Music, Video, Archive, Code, FileText } from 'lucide-react'
import { FileItem, ViewPreferences } from '../types'
import { useAppStore } from '../store/useAppStore'

interface FileGridProps {
  files: FileItem[]
  preferences: ViewPreferences
}

export default function FileGrid({ files, preferences }: FileGridProps) {
  const { selectedFiles, setSelectedFiles, navigateTo } = useAppStore()
  const [draggedFile, setDraggedFile] = useState<string | null>(null)

  const getFileIcon = (file: FileItem) => {
    if (file.isDirectory) {
      return <Folder className="w-8 h-8 text-app-accent" />
    }

    const ext = file.extension?.toLowerCase()
    if (!ext) return <File className="w-8 h-8 text-app-muted" />

    // Image files
    if (['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg', 'bmp'].includes(ext)) {
      return <Image className="w-8 h-8 text-app-green" />
    }

    // Audio files
    if (['mp3', 'wav', 'flac', 'aac', 'm4a', 'ogg'].includes(ext)) {
      return <Music className="w-8 h-8 text-app-yellow" />
    }

    // Video files
    if (['mp4', 'mkv', 'avi', 'mov', 'webm', 'flv'].includes(ext)) {
      return <Video className="w-8 h-8 text-app-red" />
    }

    // Archive files
    if (['zip', 'rar', '7z', 'tar', 'gz', 'bz2'].includes(ext)) {
      return <Archive className="w-8 h-8 text-app-muted" />
    }

    // Code files
    if (['js', 'ts', 'jsx', 'tsx', 'py', 'rs', 'go', 'java', 'cpp', 'c', 'h'].includes(ext)) {
      return <Code className="w-8 h-8 text-app-accent" />
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
      <div className="flex-1 flex items-center justify-center text-app-muted">
        <div className="text-center">
          <Folder className="w-16 h-16 mx-auto mb-4 opacity-50" />
          <p>This folder is empty</p>
        </div>
      </div>
    )
  }

  return (
    <div className="flex-1 overflow-auto p-4">
      <div className="grid gap-4" style={{
        gridTemplateColumns: 'repeat(auto-fill, minmax(120px, 1fr))'
      }}>
        {filteredFiles.map((file) => {
          const isSelected = selectedFiles.includes(file.path)
          const isDragged = draggedFile === file.path
          
          return (
            <div
              key={file.path}
              className={`flex flex-col items-center p-3 rounded-lg cursor-pointer transition-all hover:bg-app-light ${
                isSelected ? 'bg-app-accent/20 ring-2 ring-app-accent' : ''
              } ${isDragged ? 'opacity-50' : ''} ${
                file.isHidden ? 'opacity-60' : ''
              }`}
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
                <div className="text-sm font-medium truncate w-full max-w-[100px]" title={file.name}>
                  {file.name}
                </div>
                {!file.isDirectory && (
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