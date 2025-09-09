import { useState, KeyboardEvent } from 'react'
import { ChevronLeft, ChevronRight, Home, Edit3 } from 'lucide-react'
import { useAppStore } from '../store/useAppStore'

export default function PathBar() {
  const {
    currentPath,
    canGoBack,
    canGoForward,
    goBack,
    goForward,
    navigateTo,
  } = useAppStore()

  const [isEditing, setIsEditing] = useState(false)
  const [editPath, setEditPath] = useState(currentPath)

  const handleKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') {
      navigateTo(editPath)
      setIsEditing(false)
    } else if (e.key === 'Escape') {
      setEditPath(currentPath)
      setIsEditing(false)
    }
  }

  const handleEdit = () => {
    setEditPath(currentPath)
    setIsEditing(true)
  }

  const pathSegments = currentPath.split('/').filter(Boolean)

  return (
    <div className="h-12 bg-app-gray border-b border-app-border flex items-center px-4 gap-3">
      {/* Navigation buttons */}
      <div className="flex items-center gap-1">
        <button
          onClick={goBack}
          disabled={!canGoBack()}
          className="p-2 rounded-md hover:bg-app-light disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
          title="Back"
        >
          <ChevronLeft className="w-4 h-4" />
        </button>
        <button
          onClick={goForward}
          disabled={!canGoForward()}
          className="p-2 rounded-md hover:bg-app-light disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
          title="Forward"
        >
          <ChevronRight className="w-4 h-4" />
        </button>
        <button
          onClick={() => navigateTo('~')}
          className="p-2 rounded-md hover:bg-app-light transition-colors"
          title="Home"
        >
          <Home className="w-4 h-4" />
        </button>
      </div>

      {/* Path display/editor */}
      <div className="flex-1 flex items-center gap-2">
        {isEditing ? (
          <input
            type="text"
            value={editPath}
            onChange={(e) => setEditPath(e.target.value)}
            onKeyDown={handleKeyDown}
            onBlur={() => setIsEditing(false)}
            className="flex-1 input-field"
            placeholder="Enter path..."
            autoFocus
          />
        ) : (
          <div 
            className="flex-1 flex items-center gap-1 px-3 py-2 bg-app-darker rounded-md cursor-text hover:bg-app-light/50 transition-colors"
            onClick={handleEdit}
          >
            <span className="text-app-muted">/</span>
            {pathSegments.map((segment, index) => (
              <div key={index} className="flex items-center gap-1">
                <button
                  onClick={() => navigateTo('/' + pathSegments.slice(0, index + 1).join('/'))}
                  className="text-app-text hover:text-app-accent transition-colors"
                >
                  {segment}
                </button>
                {index < pathSegments.length - 1 && (
                  <span className="text-app-muted">/</span>
                )}
              </div>
            ))}
          </div>
        )}
        
        <button
          onClick={handleEdit}
          className="p-2 rounded-md hover:bg-app-light transition-colors"
          title="Edit path"
        >
          <Edit3 className="w-4 h-4" />
        </button>
      </div>
    </div>
  )
}