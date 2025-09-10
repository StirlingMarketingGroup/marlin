export interface FileItem {
  name: string
  path: string
  size: number
  modified: string // ISO 8601 string from Rust DateTime<Utc>
  is_directory: boolean // Match Rust snake_case
  is_hidden: boolean // Match Rust snake_case
  extension?: string
}

export interface ViewPreferences {
  viewMode: 'grid' | 'list' | 'details'
  sortBy: 'name' | 'size' | 'modified' | 'type'
  sortOrder: 'asc' | 'desc'
  showHidden: boolean
  foldersFirst: boolean
  // Grid (thumbnail) view tile size in px (min column width)
  gridSize?: number
}

export interface DirectoryState {
  currentPath: string
  files: FileItem[]
  loading: boolean
  error?: string
  preferences: ViewPreferences
}

export type Theme = 'system' | 'dark' | 'light'

export interface SystemDrive {
  name: string
  path: string
  drive_type: string
  is_ejectable: boolean
}
