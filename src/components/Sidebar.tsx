import { House, Desktop, FileText, DownloadSimple, ImageSquare, SquaresFour, UsersThree, HardDrives } from 'phosphor-react'
import { useAppStore } from '../store/useAppStore'

export default function Sidebar() {
  const { currentPath, navigateTo, showSidebar, sidebarWidth, homeDir } = useAppStore()
  
  if (!showSidebar) return null
  // Safe join that returns null until base (home) is known
  const join = (base?: string, sub?: string): string | null => {
    if (!base) return null
    if (!sub) return base
    const sep = base.endsWith('/') ? '' : '/'
    return `${base}${sep}${sub}`
  }

  const home = homeDir
  const userLabel = (() => {
    if (!home) return 'Home'
    // Cross-platform basename: split on both / and \\
    const parts = home.split(/[/\\\\]+/).filter(Boolean)
    return parts[parts.length - 1] || 'Home'
  })()
  const links = [
    { name: userLabel, path: home || '/', icon: <House className="w-4 h-4 text-accent" weight="fill" /> },
    { name: 'Desktop', path: join(home, 'Desktop'), icon: <Desktop className="w-4 h-4" weight="fill" /> },
    { name: 'Documents', path: join(home, 'Documents'), icon: <FileText className="w-4 h-4" weight="fill" /> },
    { name: 'Downloads', path: join(home, 'Downloads'), icon: <DownloadSimple className="w-4 h-4" weight="fill" /> },
    { name: 'Pictures', path: join(home, 'Pictures'), icon: <ImageSquare className="w-4 h-4" weight="fill" /> },
    // macOS locations
    { name: 'Applications', path: '/Applications', icon: <SquaresFour className="w-4 h-4" weight="fill" /> },
    { name: 'Users', path: '/Users', icon: <UsersThree className="w-4 h-4" weight="fill" /> },
    { name: 'System', path: '/System', icon: <HardDrives className="w-4 h-4" weight="regular" /> },
  ] as { name: string; path: string | null; icon: JSX.Element }[]

  return (
    <div 
      className="flex flex-col bg-app-gray rounded-xl m-2 overflow-hidden"
      style={{ width: sidebarWidth }}
    >
      {/* Expanded draggable area around traffic lights - covers entire top area */}
      <div
        className="h-16 w-full select-none"
        data-tauri-drag-region
      />

      {/* Flat list */}
      <div className="flex-1 overflow-y-auto px-2 py-2 space-y-[2px] -mt-8">
        {links.map(item => {
          const isDisabled = item.path == null
          const isActive = !isDisabled && currentPath === item.path
          return (
            <button
              key={item.name}
              onClick={() => !isDisabled && navigateTo(item.path!)}
              className={`w-full flex items-center gap-1 px-1.5 py-1 rounded-md text-left leading-5 text-[13px] ${
                isActive ? 'bg-accent-soft' : 'hover:bg-app-light/70'
              } ${isDisabled ? 'opacity-50 cursor-not-allowed' : ''}`}
              title={item.path || ''}
              data-tauri-drag-region={false}
              disabled={isDisabled}
            >
              {item.icon}
              <span className="truncate">{item.name}</span>
            </button>
          )
        })}
      </div>
    </div>
  )
}
