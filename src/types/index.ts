export interface FileItem {
  name: string;
  path: string;
  size: number;
  modified: string; // ISO 8601 string from Rust DateTime<Utc>
  is_directory: boolean; // Match Rust snake_case
  is_hidden: boolean; // Match Rust snake_case
  is_symlink: boolean; // Match Rust snake_case
  is_git_repo: boolean; // Match Rust snake_case
  extension?: string;
  child_count?: number; // Shallow file count for directories
}

export interface LocationSummary {
  raw: string;
  scheme: string;
  authority?: string | null;
  path: string;
  displayPath: string;
}

export interface LocationCapabilities {
  scheme: string;
  displayName: string;
  canRead: boolean;
  canWrite: boolean;
  canCreateDirectories: boolean;
  canDelete: boolean;
  canRename: boolean;
  canCopy: boolean;
  canMove: boolean;
  supportsWatching: boolean;
  requiresExplicitRefresh: boolean;
}

export interface DirectoryListingResponse {
  location: LocationSummary;
  capabilities: LocationCapabilities;
  entries: FileItem[];
}

export interface ViewPreferences {
  viewMode: 'grid' | 'list' | 'details';
  sortBy: 'name' | 'size' | 'modified' | 'type';
  sortOrder: 'asc' | 'desc';
  showHidden: boolean;
  foldersFirst: boolean;
  // Grid (thumbnail) view tile size in px (min column width)
  gridSize?: number;
}

export type DirectoryPreferencesMap = Record<string, Partial<ViewPreferences>>;

export interface DirectoryState {
  currentPath: string;
  files: FileItem[];
  loading: boolean;
  error?: string;
  preferences: ViewPreferences;
}

export type Theme = 'system' | 'dark' | 'light';

export interface SystemDrive {
  name: string;
  path: string;
  drive_type: string;
  is_ejectable: boolean;
}

export interface DiskUsage {
  path: string;
  totalBytes: number;
  availableBytes: number;
}

export interface GitStatus {
  repositoryRoot: string;
  branch?: string;
  detached: boolean;
  ahead: number;
  behind: number;
  dirty: boolean;
  hasUntracked: boolean;
  remoteUrl?: string;
  remoteBranchUrl?: string;
}

export interface PinnedDirectory {
  name: string;
  path: string;
  pinned_at: string; // ISO 8601 string from Rust DateTime<Utc>
  is_git_repo: boolean;
  is_symlink: boolean;
}

export interface PersistedPreferences {
  lastDir?: string;
  globalPreferences?: Partial<ViewPreferences>;
  directoryPreferences?: DirectoryPreferencesMap;
}

export interface DirectoryChangeEventPayload {
  path: string;
  changeType: string;
  affectedFiles: string[];
}

export interface FolderSizeProgressPayload {
  requestId: string;
  totalBytes: number;
  totalApparentBytes: number;
  totalItems: number;
  currentPath?: string | null;
  finished: boolean;
  cancelled: boolean;
  error?: string | null;
}

export interface FolderSizeTargetPayload {
  path: string;
  name: string;
  isDirectory: boolean;
}

export interface FolderSizeInitPayload {
  requestId: string;
  targets: FolderSizeTargetPayload[];
  autoStart: boolean;
  initialError?: string | null;
}

export interface ArchiveProgressPayload {
  fileName: string;
  destinationDir: string;
  format?: string;
}

export interface ArchiveProgressUpdatePayload {
  archiveName: string;
  entryName?: string;
  format?: string;
  finished?: boolean;
}

export interface TrashPathsResponse {
  trashed: string[];
  undoToken?: string;
  fallbackToPermanent: boolean;
}

export interface UndoTrashResponse {
  restored: string[];
}

export interface DeleteItemPayload {
  path: string;
  name: string;
  isDirectory?: boolean;
}

export interface DeleteProgressPayload {
  requestId: string;
  totalItems: number;
  items: DeleteItemPayload[];
}

export interface DeleteProgressUpdatePayload {
  requestId: string;
  currentPath?: string;
  completed: number;
  total: number;
  finished: boolean;
  error?: string;
}

export interface DeletePathsResponse {
  deleted: string[];
}
