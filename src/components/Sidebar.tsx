import { useState } from 'react'
import { Folder, FolderOpen, HardDrive, Home, Settings, ChevronRight, ChevronDown } from 'lucide-react'
import { useAppStore } from '../store/useAppStore'

interface TreeItem {
  name: string
  path: string
  isOpen: boolean
  children?: TreeItem[]
}

export default function Sidebar() {
  const { currentPath, navigateTo, showSidebar, sidebarWidth } = useAppStore()
  const [treeState, setTreeState] = useState<Record<string, boolean>>({})
  
  if (!showSidebar) return null

  const mockTreeData: TreeItem[] = [
    {
      name: 'Home',
      path: '~',
      isOpen: true,
      children: [
        { name: 'Documents', path: '~/Documents', isOpen: false },
        { name: 'Downloads', path: '~/Downloads', isOpen: false },
        { name: 'Pictures', path: '~/Pictures', isOpen: false },
      ]
    },
    {
      name: 'System',
      path: '/',
      isOpen: false,
      children: [
        { name: 'Applications', path: '/Applications', isOpen: false },
        { name: 'Users', path: '/Users', isOpen: false },
        { name: 'System', path: '/System', isOpen: false },
      ]
    }
  ]

  const toggleFolder = (path: string) => {
    setTreeState(prev => ({
      ...prev,
      [path]: !prev[path]
    }))
  }

  const renderTreeItem = (item: TreeItem, level = 0) => {
    const isSelected = currentPath === item.path
    const isOpen = treeState[item.path] ?? item.isOpen
    const hasChildren = item.children && item.children.length > 0

    return (
      <div key={item.path} className="select-none">
        <div
          className={`flex items-center gap-2 px-2 py-1 rounded-md cursor-pointer hover:bg-app-light transition-colors ${
            isSelected ? 'bg-app-accent/20 text-app-accent' : ''
          }`}
          style={{ paddingLeft: `${8 + level * 20}px` }}
          onClick={() => navigateTo(item.path)}
        >
          {hasChildren && (
            <button
              onClick={(e) => {
                e.stopPropagation()
                toggleFolder(item.path)
              }}
              className="p-1 hover:bg-app-gray rounded"
            >
              {isOpen ? (
                <ChevronDown className="w-3 h-3" />
              ) : (
                <ChevronRight className="w-3 h-3" />
              )}
            </button>
          )}
          
          {!hasChildren && <div className="w-5" />}
          
          <div className="flex items-center gap-2">
            {item.name === 'Home' ? (
              <Home className="w-4 h-4" />
            ) : item.name === 'System' ? (
              <HardDrive className="w-4 h-4" />
            ) : hasChildren ? (
              isOpen ? <FolderOpen className="w-4 h-4" /> : <Folder className="w-4 h-4" />
            ) : (
              <Folder className="w-4 h-4" />
            )}
            
            <span className="text-sm truncate">{item.name}</span>
          </div>
        </div>
        
        {hasChildren && isOpen && (
          <div>
            {item.children?.map(child => renderTreeItem(child, level + 1))}
          </div>
        )}
      </div>
    )
  }

  return (
    <div 
      className="bg-app-gray border-r border-app-border flex flex-col"
      style={{ width: sidebarWidth }}
    >
      {/* Sidebar header */}
      <div className="h-12 flex items-center justify-between px-4 border-b border-app-border">
        <h2 className="font-semibold text-sm">Locations</h2>
        <button className="p-1 hover:bg-app-light rounded transition-colors">
          <Settings className="w-4 h-4" />
        </button>
      </div>
      
      {/* Tree view */}
      <div className="flex-1 overflow-y-auto p-2 space-y-1">
        {mockTreeData.map(item => renderTreeItem(item))}
      </div>
    </div>
  )
}