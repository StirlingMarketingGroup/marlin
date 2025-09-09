export interface FileItem {
  name: string
  path: string
  size: number
  modified: Date
  isDirectory: boolean
  isHidden: boolean
  extension?: string
}

export interface ViewPreferences {
  viewMode: 'grid' | 'list' | 'details'
  sortBy: 'name' | 'size' | 'modified' | 'type'
  sortOrder: 'asc' | 'desc'
  showHidden: boolean
}

export interface DirectoryState {
  currentPath: string
  files: FileItem[]
  loading: boolean
  error?: string
  preferences: ViewPreferences
}

export type Theme = 'system' | 'dark' | 'light'