import { House, Desktop, FileText, DownloadSimple, ImageSquare, SquaresFour, UsersThree, HardDrives, Eject, CircleNotch, Trash, Recycle, Folder } from 'phosphor-react'
import { useAppStore } from '../store/useAppStore'
import { useCallback, useEffect, useState, MouseEvent, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { SystemDrive, PinnedDirectory } from '../types'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { useToastStore } from '../store/useToastStore'
import { useSidebarDropZone } from '../hooks/useDragDetector'

export default function Sidebar() {
  const { 
    currentPath, 
    navigateTo, 
    showSidebar, 
    sidebarWidth, 
    homeDir, 
    pinnedDirectories, 
    removePinnedDirectory,
    addPinnedDirectory
  } = useAppStore()

  const [systemDrives, setSystemDrives] = useState<SystemDrive[]>([])
  const [ejectingDrives, setEjectingDrives] = useState<Set<string>>(new Set())
  const [isDragOver, setIsDragOver] = useState(false)
  const sidebarRef = useRef<HTMLDivElement>(null)
  
  // Use the new drag detector hook for native drop detection
  const handleDragEnter = useCallback(() => setIsDragOver(true), [])
  const handleDragLeave = useCallback(() => setIsDragOver(false), [])

  useSidebarDropZone(async (paths) => {
    // Handle dropped directories
    for (const path of paths) {
      try {
        await addPinnedDirectory(path)
        const { addToast } = useToastStore.getState()
        addToast({
          type: 'success',
          message: `Pinned ${path.split('/').pop() || 'directory'} to sidebar`
        })
      } catch (error) {
        console.error('Failed to pin directory:', error)
        const { addToast } = useToastStore.getState()
        addToast({
          type: 'error',
          message: 'Failed to pin directory'
        })
      }
    }
    setIsDragOver(false)
  }, {
    onDragEnter: handleDragEnter,
    onDragOver: handleDragEnter,
    onDragLeave: handleDragLeave
  })
  
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

  const handleUnpinDirectory = async (pin: PinnedDirectory, event: React.MouseEvent) => {
    event.stopPropagation()
    const { addToast } = useToastStore.getState()
    
    try {
      await removePinnedDirectory(pin.path)
      
      // Show success toast with undo action
      addToast({
        message: `Removed "${pin.name}" from pinned folders`,
        type: 'success',
        duration: 8000, // Give more time for undo
        action: {
          label: 'Undo',
          onClick: async () => {
            try {
              await addPinnedDirectory(pin.path)
              addToast({
                message: `Restored "${pin.name}" to pinned folders`,
                type: 'success',
                duration: 3000
              })
            } catch (error) {
              console.error('Failed to restore pinned directory:', error)
              addToast({
                message: 'Failed to restore pinned folder',
                type: 'error',
                duration: 5000
              })
            }
          }
        }
      })
    } catch (error) {
      console.error('Failed to unpin directory:', error)
      addToast({
        message: 'Failed to remove pinned folder',
        type: 'error',
        duration: 5000
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
    <IconComponent className={`w-5 h-5 ${isActive ? 'text-accent' : ''}`} weight={weight} />
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
      ref={sidebarRef}
      className={`flex flex-col h-full bg-app-gray rounded-xl overflow-hidden transition-all duration-200 ${
        isDragOver ? 'drag-over ring-2 ring-accent bg-accent/10 shadow-lg shadow-accent/20' : ''
      }`}
      style={{ width: sidebarWidth }}
      data-tauri-drag-region={false}
      data-sidebar="true"
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
        {/* Favorites section */}
        <div className="px-1 py-1 text-xs text-app-muted select-none">Favorites</div>
        {/* User directories */}
        {links.slice(0, 6).map(item => {
          const isDisabled = item.path == null
          const isActive = !isDisabled && currentPath === item.path
          return (
            <button
              key={item.name}
              onClick={() => !isDisabled && navigateTo(item.path!)}
              className={`w-full flex items-center gap-2 px-1.5 py-1 rounded-md text-left leading-5 text-[13px] ${
                isActive ? 'bg-app-light' : 'hover:bg-app-light/70'
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
        
        {/* Pinned directories section */}
        {pinnedDirectories.length > 0 && (
          <>
            <div className="px-1 pt-3 pb-1 text-xs text-app-muted select-none">Pinned</div>
            {[...pinnedDirectories].sort((a, b) => a.name.localeCompare(b.name)).map(pin => {
              const isActive = currentPath === pin.path
              return (
                <div
                  key={pin.path}
                  className={`w-full flex items-center gap-2 px-1.5 py-1 rounded-md text-left leading-5 text-[13px] group ${
                    isActive ? 'bg-app-light' : 'hover:bg-app-light/70'
                  }`}
                >
                  <button
                    onClick={() => navigateTo(pin.path)}
                    className="flex items-center gap-2 flex-1 min-w-0"
                    title={pin.path}
                    data-tauri-drag-region={false}
                  >
                    {createIcon(Folder, "fill", isActive)}
                    <span className={`truncate ${isActive ? 'text-accent' : ''}`}>{pin.name}</span>
                  </button>
                  
                  <button
                    onClick={(e) => handleUnpinDirectory(pin, e)}
                    className="ml-auto p-0.5 rounded hover:bg-app-light/50 text-app-muted hover:text-accent transition-colors"
                    title="Remove from pinned"
                    data-tauri-drag-region={false}
                  >
                    <Trash 
                      className="w-3.5 h-3.5" 
                      weight="regular" 
                    />
                  </button>
                </div>
              )
            })}
          </>
        )}
        
        {/* Locations section */}
        <div className="px-1 pt-3 pb-1 text-xs text-app-muted select-none">System</div>
        {(() => {
          const systemLinks = links.slice(6) // Applications, Users, System
          return systemLinks.map(item => {
            const isDisabled = item.path == null
            const isActive = !isDisabled && currentPath === item.path
            return (
              <button
                key={`sys-${item.name}`}
                onClick={() => !isDisabled && navigateTo(item.path!)}
                className={`w-full flex items-center gap-2 px-1.5 py-1 rounded-md text-left leading-5 text-[13px] ${
                  isActive ? 'bg-app-light' : 'hover:bg-app-light/70'
                } ${isDisabled ? 'opacity-50 cursor-not-allowed' : ''}`}
                title={item.path || ''}
                data-tauri-drag-region={false}
                disabled={isDisabled}
              >
                {createIcon(item.iconType, item.weight, isActive)}
                <span className={`truncate ${isActive ? 'text-accent' : ''}`}>{item.name}</span>
              </button>
            )
          })
        })()}

        {/* System drives */}
        {systemDrives.length > 0 && (
          <>
            <div className="px-1 pt-3 pb-1 text-xs text-app-muted select-none">Drives</div>
            {systemDrives.map(drive => {
              const isActive = currentPath === drive.path
              const isEjecting = ejectingDrives.has(drive.path)
              return (
                <div
                  key={drive.path}
                  className={`w-full flex items-center gap-2 px-1.5 py-1 rounded-md text-left leading-5 text-[13px] group ${
                    isActive ? 'bg-app-light' : 'hover:bg-app-light/70'
                  }`}
                >
                  <button
                    onClick={() => navigateTo(drive.path)}
                    className="flex items-center gap-2 flex-1 min-w-0"
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
