import { Grid, List, MoreHorizontal, SortAsc, SortDesc, Eye, EyeOff } from 'lucide-react'
import { useAppStore } from '../store/useAppStore'
import FileGrid from './FileGrid'
import FileList from './FileList'

export default function MainPanel() {
  const {
    files,
    loading,
    error,
    globalPreferences,
    currentPath,
    directoryPreferences,
    updateGlobalPreferences,
    updateDirectoryPreferences,
  } = useAppStore()

  const currentPrefs = {
    ...globalPreferences,
    ...directoryPreferences[currentPath],
  }

  const toggleViewMode = () => {
    const newMode = currentPrefs.viewMode === 'grid' ? 'list' : 'grid'
    updateDirectoryPreferences(currentPath, { viewMode: newMode })
  }

  const toggleSortOrder = () => {
    const newOrder = currentPrefs.sortOrder === 'asc' ? 'desc' : 'asc'
    updateDirectoryPreferences(currentPath, { sortOrder: newOrder })
  }

  const changeSortBy = (sortBy: typeof currentPrefs.sortBy) => {
    updateDirectoryPreferences(currentPath, { sortBy })
  }

  const toggleHidden = () => {
    updateGlobalPreferences({ showHidden: !currentPrefs.showHidden })
  }

  if (loading) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <div className="text-app-muted">Loading...</div>
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <div className="text-app-red">Error: {error}</div>
      </div>
    )
  }

  return (
    <div className="flex-1 flex flex-col">
      {/* Toolbar */}
      <div className="h-12 bg-app-gray border-b border-app-border flex items-center justify-between px-4">
        <div className="flex items-center gap-2">
          {/* View mode toggle */}
          <button
            onClick={toggleViewMode}
            className="p-2 rounded-md hover:bg-app-light transition-colors"
            title={`Switch to ${currentPrefs.viewMode === 'grid' ? 'list' : 'grid'} view`}
          >
            {currentPrefs.viewMode === 'grid' ? (
              <List className="w-4 h-4" />
            ) : (
              <Grid className="w-4 h-4" />
            )}
          </button>

          {/* Sort controls */}
          <div className="flex items-center gap-1">
            <select
              value={currentPrefs.sortBy}
              onChange={(e) => changeSortBy(e.target.value as typeof currentPrefs.sortBy)}
              className="bg-app-darker border border-app-border rounded-md px-2 py-1 text-sm"
            >
              <option value="name">Name</option>
              <option value="size">Size</option>
              <option value="modified">Modified</option>
              <option value="type">Type</option>
            </select>
            
            <button
              onClick={toggleSortOrder}
              className="p-2 rounded-md hover:bg-app-light transition-colors"
              title={`Sort ${currentPrefs.sortOrder === 'asc' ? 'descending' : 'ascending'}`}
            >
              {currentPrefs.sortOrder === 'asc' ? (
                <SortAsc className="w-4 h-4" />
              ) : (
                <SortDesc className="w-4 h-4" />
              )}
            </button>
          </div>

          <div className="w-px h-6 bg-app-border" />

          {/* Hidden files toggle */}
          <button
            onClick={toggleHidden}
            className={`p-2 rounded-md transition-colors ${
              currentPrefs.showHidden
                ? 'bg-app-accent/20 text-app-accent hover:bg-app-accent/30'
                : 'hover:bg-app-light'
            }`}
            title={`${currentPrefs.showHidden ? 'Hide' : 'Show'} hidden files`}
          >
            {currentPrefs.showHidden ? (
              <EyeOff className="w-4 h-4" />
            ) : (
              <Eye className="w-4 h-4" />
            )}
          </button>
        </div>

        <div className="flex items-center gap-2 text-sm text-app-muted">
          <span>{files.length} items</span>
          <button className="p-1 rounded hover:bg-app-light">
            <MoreHorizontal className="w-4 h-4" />
          </button>
        </div>
      </div>

      {/* File content */}
      <div className="flex-1 overflow-hidden">
        {currentPrefs.viewMode === 'grid' ? (
          <FileGrid files={files} preferences={currentPrefs} />
        ) : (
          <FileList files={files} preferences={currentPrefs} />
        )}
      </div>
    </div>
  )
}