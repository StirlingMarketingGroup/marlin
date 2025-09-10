import { House, Desktop, FileText, DownloadSimple, ImageSquare, SquaresFour, UsersThree, HardDrives, Eject, CircleNotch, Trash, Recycle } from 'phosphor-react'
import { useAppStore } from '../store/useAppStore'
import { useEffect, useState, MouseEvent } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { SystemDrive } from '../types'
import { getCurrentWindow } from '@tauri-apps/api/window'

export default function Sidebar() {
  const { currentPath, navigateTo, showSidebar, sidebarWidth, homeDir } = useAppStore()
  const [systemDrives, setSystemDrives] = useState<SystemDrive[]>([])
  const [ejectingDrives, setEjectingDrives] = useState<Set<string>>(new Set())
  
  // Fetch system drives on component mount
  useEffect(() => {
    fetchSystemDrives()
  }, [])

  const fetchSystemDrives = async () => {
    try {
      const drives = await invoke<SystemDrive[]>('get_system_drives')
      setSystemDrives(drives)
    } catch (error) {
      console.error('Failed to fetch system drives:', error)
    }
  }

  const handleEjectDrive = async (drive: SystemDrive, event: React.MouseEvent) => {
    event.stopPropagation() // Prevent navigation when clicking eject
    
    setEjectingDrives(prev => new Set(prev).add(drive.path))
    
    try {
      await invoke('eject_drive', { path: drive.path })
      // Refresh the drives list after successful ejection
      await fetchSystemDrives()
    } catch (error) {
      console.error('Failed to eject drive:', error)
      // TODO: Show error notification to user
    } finally {
      setEjectingDrives(prev => {
        const newSet = new Set(prev)
        newSet.delete(drive.path)
        return newSet
      })
    }
  }
  
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
  const createIcon = (IconComponent: any, weight: "fill" | "regular" = "fill", isActive: boolean) => (
    <IconComponent className={`w-4 h-4 ${isActive ? 'text-accent' : ''}`} weight={weight} />
  )

  // Platform detection for special folders
  const isMac = typeof navigator !== 'undefined' && navigator.platform.toUpperCase().includes('MAC')
  const isWindows = typeof navigator !== 'undefined' && navigator.userAgent.toUpperCase().includes('WINDOWS')
  const trashPath = isMac ? join(home, '.Trash') : (isWindows ? null : join(home, '.local/share/Trash/files'))
  const trashLabel = isWindows ? 'Recycle Bin' : 'Trash'

  const links = [
    { name: userLabel, path: home || '/', iconType: House, weight: "fill" as const },
    { name: 'Desktop', path: join(home, 'Desktop'), iconType: Desktop, weight: "fill" as const },
    { name: 'Documents', path: join(home, 'Documents'), iconType: FileText, weight: "fill" as const },
    { name: 'Downloads', path: join(home, 'Downloads'), iconType: DownloadSimple, weight: "fill" as const },
    { name: 'Pictures', path: join(home, 'Pictures'), iconType: ImageSquare, weight: "fill" as const },
    { name: trashLabel, path: trashPath, iconType: isWindows ? Recycle : Trash, weight: "fill" as const },
    // macOS locations
    { name: 'Applications', path: '/Applications', iconType: SquaresFour, weight: "fill" as const },
    { name: 'Users', path: '/Users', iconType: UsersThree, weight: "fill" as const },
    { name: 'System', path: '/System', iconType: HardDrives, weight: "regular" as const },
  ] as { name: string; path: string | null; iconType: any; weight: "fill" | "regular" }[]

  return (
    <div 
      className="flex flex-col h-full bg-app-gray rounded-xl overflow-hidden"
      style={{ width: sidebarWidth }}
    >
      {/* Expanded draggable area around traffic lights - covers entire top area */}
      <div
        className="h-16 w-full select-none"
        data-tauri-drag-region
        onMouseDown={async (e: MouseEvent<HTMLDivElement>) => {
          if (e.button !== 0) return
          const target = e.target as HTMLElement
          if (target.closest('[data-tauri-drag-region="false"], button, input, select, textarea, [role="button"]')) return
          try {
            const win = getCurrentWindow()
            await win.startDragging()
          } catch {}
        }}
      />

      {/* Flat list */}
      <div className="flex-1 overflow-y-auto px-2 py-2 pb-2 space-y-[2px] -mt-8">
        {/* User directories */}
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
              {createIcon(item.iconType, item.weight, isActive)}
              <span className={`truncate ${isActive ? 'text-accent' : ''}`}>{item.name}</span>
            </button>
          )
        })}
        
        {/* System drives */}
        {systemDrives.length > 0 && (
          <>
            <div className="h-2" /> {/* Small separator */}
            {systemDrives.map(drive => {
              const isActive = currentPath === drive.path
              const isEjecting = ejectingDrives.has(drive.path)
              return (
                <div
                  key={drive.path}
                  className={`w-full flex items-center gap-1 px-1.5 py-1 rounded-md text-left leading-5 text-[13px] group ${
                    isActive ? 'bg-accent-soft' : 'hover:bg-app-light/70'
                  }`}
                >
                  <button
                    onClick={() => navigateTo(drive.path)}
                    className="flex items-center gap-1 flex-1 min-w-0"
                    title={drive.path}
                    data-tauri-drag-region={false}
                  >
                    {createIcon(HardDrives, "regular", isActive)}
                    <span className={`truncate ${isActive ? 'text-accent' : ''}`}>{drive.name}</span>
                  </button>
                  
                  {drive.is_ejectable && (
                    <button
                      onClick={(e) => handleEjectDrive(drive, e)}
                      disabled={isEjecting}
                      className="ml-auto p-0.5 rounded hover:bg-app-light/50"
                      title={isEjecting ? 'Ejecting...' : 'Eject drive'}
                      data-tauri-drag-region={false}
                    >
                      {isEjecting ? (
                        <CircleNotch 
                          className="w-3 h-3 animate-spin text-app-muted" 
                          weight="regular" 
                        />
                      ) : (
                        <Eject 
                          className="w-3 h-3 text-app-text hover:text-accent" 
                          weight="regular" 
                        />
                      )}
                    </button>
                  )}
                </div>
              )
            })}
          </>
        )}
      </div>
    </div>
  )
}
