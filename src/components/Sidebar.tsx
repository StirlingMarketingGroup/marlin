import {
  House,
  Desktop,
  FileText,
  DownloadSimple,
  ImageSquare,
  VideoCamera,
  SquaresFour,
  UsersThree,
  HardDrives,
  Eject,
  CircleNotch,
  Trash,
  Recycle,
  Folder,
  MusicNotes,
  GoogleLogo,
  Plus,
  SignOut,
  ShareNetwork,
  Terminal,
} from 'phosphor-react';
import GitRepoBadge from './GitRepoBadge';
import SymlinkBadge from './SymlinkBadge';
import type { IconProps } from 'phosphor-react';
import { useAppStore } from '../store/useAppStore';
import { useCallback, useEffect, useState, MouseEvent, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { SystemDrive, PinnedDirectory, GoogleAccountInfo, SmbServerInfo, SftpServerInfo } from '../types';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useToastStore } from '../store/useToastStore';
import { useDragStore } from '../store/useDragStore';
import { useSidebarDropZone } from '../hooks/useDragDetector';
import { usePlatform } from '@/hooks/usePlatform';
import QuickTooltip from './QuickTooltip';

type SidebarLink = {
  name: string;
  path: string | null;
  iconType: React.ComponentType<IconProps>;
  weight: 'fill' | 'regular';
};

export default function Sidebar() {
  const {
    currentPath,
    navigateTo,
    showSidebar,
    sidebarWidth,
    homeDir,
    pinnedDirectories,
    removePinnedDirectory,
    addPinnedDirectory,
    googleAccounts,
    loadGoogleAccounts,
    addGoogleAccount,
    removeGoogleAccount,
    smbServers,
    loadSmbServers,
    removeSmbServer,
    pendingSmbCredentialRequest,
    setPendingSmbCredentialRequest,
    sftpServers,
    loadSftpServers,
    removeSftpServer,
    pendingSftpCredentialRequest,
    setPendingSftpCredentialRequest,
  } = useAppStore();

  const [systemDrives, setSystemDrives] = useState<SystemDrive[]>([]);
  const [ejectingDrives, setEjectingDrives] = useState<Set<string>>(new Set());
  const [isDragOver, setIsDragOver] = useState(false);
  const [addingGoogleAccount, setAddingGoogleAccount] = useState(false);
  const [disconnectingAccounts, setDisconnectingAccounts] = useState<Set<string>>(new Set());
  const [disconnectingSmbServers, setDisconnectingSmbServers] = useState<Set<string>>(new Set());
  const [disconnectingSftpServers, setDisconnectingSftpServers] = useState<Set<string>>(new Set());
  const inAppDropTargetId = useDragStore((state) => state.inAppDropTargetId);
  const sidebarRef = useRef<HTMLDivElement>(null);
  const isProcessingDropRef = useRef(false);
  const windowRef = useRef(getCurrentWindow());
  const isInAppDragOver = inAppDropTargetId === 'sidebar';
  const isDragOverCombined = isDragOver || isInAppDragOver;

  const handleDragRegionMouseDown = useCallback(async (event: MouseEvent<HTMLElement>) => {
    if (event.button !== 0) return;
    const target = event.target as HTMLElement | null;
    if (
      target?.closest(
        '[data-tauri-drag-region="false"], button, input, select, textarea, [role="button"]'
      )
    ) {
      return;
    }
    try {
      await windowRef.current.startDragging();
    } catch (error) {
      console.warn('Sidebar drag start failed:', error);
    }
  }, []);

  // Use the new drag detector hook for native drop detection
  const handleDragEnter = useCallback(() => {
    setIsDragOver(true);
  }, []);
  const handleDragLeave = useCallback(() => setIsDragOver(false), []);

  useEffect(() => {
    if (!pendingSmbCredentialRequest) return;

    (async () => {
      try {
        await invoke('open_smb_connect_window', {
          initialHostname: pendingSmbCredentialRequest.hostname,
          targetPath: pendingSmbCredentialRequest.targetPath,
        });
        setPendingSmbCredentialRequest(null);
      } catch (error) {
        console.warn('Failed to open SMB connect window:', error);
      }
    })();
  }, [pendingSmbCredentialRequest, setPendingSmbCredentialRequest]);

  useEffect(() => {
    if (!pendingSftpCredentialRequest) return;

    (async () => {
      try {
        await invoke('open_sftp_connect_window', {
          initialHostname: pendingSftpCredentialRequest.hostname,
          initialPort: pendingSftpCredentialRequest.port,
          initialUsername: pendingSftpCredentialRequest.username,
          targetPath: pendingSftpCredentialRequest.targetPath,
        });
        setPendingSftpCredentialRequest(null);
      } catch (error) {
        console.warn('Failed to open SFTP connect window:', error);
      }
    })();
  }, [pendingSftpCredentialRequest, setPendingSftpCredentialRequest]);

  useSidebarDropZone(
    async (paths) => {
      if (isProcessingDropRef.current) {
        return;
      }

      isProcessingDropRef.current = true;

      try {
        const uniquePaths = Array.from(new Set(paths));

        if (uniquePaths.length === 0) {
          return;
        }

        const pinnedNames: string[] = [];
        const skippedFiles: string[] = [];
        const otherErrors: { name: string; message: string }[] = [];

        const getName = (path: string) => {
          const parts = path.split(/[/\\\\]+/).filter(Boolean);
          return parts[parts.length - 1] || path;
        };

        for (const path of uniquePaths) {
          try {
            const pin = await addPinnedDirectory(path);
            pinnedNames.push(pin.name || getName(path));
          } catch (error) {
            const message = error instanceof Error ? error.message : String(error);
            const normalizedMessage = message.toLowerCase();
            if (normalizedMessage.includes('already pinned')) {
              continue;
            }
            if (
              normalizedMessage.includes('not a directory') ||
              normalizedMessage.includes('is not a directory')
            ) {
              skippedFiles.push(getName(path));
              continue;
            }

            console.error('Failed to pin directory:', error);
            otherErrors.push({ name: getName(path), message });
          }
        }

        const { addToast } = useToastStore.getState();

        if (pinnedNames.length > 0) {
          const list = pinnedNames.length === 1 ? pinnedNames[0] : `${pinnedNames.length} folders`;
          addToast({
            type: 'success',
            message: `Pinned ${list} to sidebar`,
          });
        }

        if (skippedFiles.length > 0) {
          const detail =
            skippedFiles.length === 1
              ? `“${skippedFiles[0]}” is a file.`
              : `${skippedFiles.length} files were skipped.`;
          addToast({
            type: 'error',
            message: `Only folders can be pinned to the sidebar. ${detail}`,
          });
        }

        if (otherErrors.length > 0) {
          const first = otherErrors[0];
          const suffix = otherErrors.length > 1 ? ' (others skipped)' : '';
          addToast({
            type: 'error',
            message: `Failed to pin ${first.name}: ${first.message}${suffix}`,
          });
        }
      } finally {
        isProcessingDropRef.current = false;
        setIsDragOver(false);
      }
    },
    {
      onDragEnter: handleDragEnter,
      onDragOver: handleDragEnter,
      onDragLeave: handleDragLeave,
      width: sidebarWidth,
    }
  );

  // Fetch system drives, Google accounts, and SMB servers on component mount
  useEffect(() => {
    fetchSystemDrives();
    loadGoogleAccounts();
    loadSmbServers();
    loadSftpServers();
  }, [loadGoogleAccounts, loadSmbServers, loadSftpServers]);

  const fetchSystemDrives = async () => {
    try {
      const drives = await invoke<SystemDrive[]>('get_system_drives');
      setSystemDrives(drives);
    } catch (error) {
      console.error('Failed to fetch system drives:', error);
    }
  };

  const handleEjectDrive = async (drive: SystemDrive, event: React.MouseEvent) => {
    event.stopPropagation(); // Prevent navigation when clicking eject

    setEjectingDrives((prev) => new Set(prev).add(drive.path));

    try {
      await invoke('eject_drive', { path: drive.path });
      // Refresh the drives list after successful ejection
      await fetchSystemDrives();
    } catch (error) {
      console.error('Failed to eject drive:', error);
      // TODO: Show error notification to user
    } finally {
      setEjectingDrives((prev) => {
        const newSet = new Set(prev);
        newSet.delete(drive.path);
        return newSet;
      });
    }
  };

  const handleAddGoogleAccount = async () => {
    if (addingGoogleAccount) return;
    setAddingGoogleAccount(true);
    const { addToast } = useToastStore.getState();

    try {
      const account = await addGoogleAccount();
      if (account) {
        addToast({
          type: 'success',
          message: `Connected ${account.displayName || account.email}`,
          duration: 3000,
        });
      }
    } catch (error) {
      console.error('Failed to add Google account:', error);
      addToast({
        type: 'error',
        message: 'Failed to connect Google account',
        duration: 5000,
      });
    } finally {
      setAddingGoogleAccount(false);
    }
  };

  const handleDisconnectGoogleAccount = async (
    account: GoogleAccountInfo,
    event: React.MouseEvent
  ) => {
    event.stopPropagation();
    const { addToast } = useToastStore.getState();

    setDisconnectingAccounts((prev) => new Set(prev).add(account.email));

    try {
      await removeGoogleAccount(account.email);
      addToast({
        type: 'success',
        message: `Disconnected ${account.displayName || account.email}`,
        duration: 3000,
      });
    } catch (error) {
      console.error('Failed to disconnect Google account:', error);
      addToast({
        type: 'error',
        message: 'Failed to disconnect Google account',
        duration: 5000,
      });
    } finally {
      setDisconnectingAccounts((prev) => {
        const newSet = new Set(prev);
        newSet.delete(account.email);
        return newSet;
      });
    }
  };

  const handleDisconnectSmbServer = async (server: SmbServerInfo, event: React.MouseEvent) => {
    event.stopPropagation();
    const { addToast } = useToastStore.getState();

    setDisconnectingSmbServers((prev) => new Set(prev).add(server.hostname));

    try {
      await removeSmbServer(server.hostname);
      addToast({
        type: 'success',
        message: `Disconnected from ${server.hostname}`,
        duration: 3000,
      });
    } catch (error) {
      console.error('Failed to disconnect SMB server:', error);
      addToast({
        type: 'error',
        message: 'Failed to disconnect from server',
        duration: 5000,
      });
    } finally {
      setDisconnectingSmbServers((prev) => {
        const newSet = new Set(prev);
        newSet.delete(server.hostname);
        return newSet;
      });
    }
  };

  const handleDisconnectSftpServer = async (server: SftpServerInfo, event: React.MouseEvent) => {
    event.stopPropagation();
    const { addToast } = useToastStore.getState();
    const serverKey = `${server.hostname}:${server.port}`;

    setDisconnectingSftpServers((prev) => new Set(prev).add(serverKey));

    try {
      await removeSftpServer(server.hostname, server.port);
      addToast({
        type: 'success',
        message: `Disconnected from ${server.hostname}`,
        duration: 3000,
      });
    } catch (error) {
      console.error('Failed to disconnect SFTP server:', error);
      addToast({
        type: 'error',
        message: 'Failed to disconnect from server',
        duration: 5000,
      });
    } finally {
      setDisconnectingSftpServers((prev) => {
        const newSet = new Set(prev);
        newSet.delete(serverKey);
        return newSet;
      });
    }
  };

  const handleUnpinDirectory = async (pin: PinnedDirectory, event: React.MouseEvent) => {
    event.stopPropagation();
    const { addToast } = useToastStore.getState();

    try {
      await removePinnedDirectory(pin.path);

      // Show success toast with undo action
      addToast({
        message: `Removed "${pin.name}" from pinned folders`,
        type: 'success',
        duration: 8000, // Give more time for undo
        action: {
          label: 'Undo',
          onClick: async () => {
            try {
              await addPinnedDirectory(pin.path);
              addToast({
                message: `Restored "${pin.name}" to pinned folders`,
                type: 'success',
                duration: 3000,
              });
            } catch (error) {
              console.error('Failed to restore pinned directory:', error);
              addToast({
                message: 'Failed to restore pinned folder',
                type: 'error',
                duration: 5000,
              });
            }
          },
        },
      });
    } catch (error) {
      console.error('Failed to unpin directory:', error);
      addToast({
        message: 'Failed to remove pinned folder',
        type: 'error',
        duration: 5000,
      });
    }
  };

  // Platform detection for special folders - must be before early return
  const { isMac, isWindows } = usePlatform();

  if (!showSidebar) return null;
  // Safe join that returns null until base (home) is known
  const join = (base?: string, sub?: string): string | null => {
    if (!base) return null;
    if (!sub) return base;
    const sep = base.endsWith('/') ? '' : '/';
    return `${base}${sep}${sub}`;
  };

  const home = homeDir;
  const userLabel = (() => {
    if (!home) return 'Home';
    // Cross-platform basename: split on both / and \\
    const parts = home.split(/[/\\\\]+/).filter(Boolean);
    return parts[parts.length - 1] || 'Home';
  })();
  const createIcon = (
    IconComponent: React.ComponentType<IconProps>,
    weight: 'fill' | 'regular' = 'fill',
    isActive: boolean
  ) => <IconComponent className={`w-5 h-5 ${isActive ? 'text-accent' : ''}`} weight={weight} />;
  const trashPath = isMac
    ? join(home, '.Trash')
    : isWindows
      ? null
      : join(home, '.local/share/Trash/files');
  const trashLabel = isWindows ? 'Recycle Bin' : 'Trash';
  const videoFolderLabel = isMac ? 'Movies' : 'Videos';

  const favoriteLinks: SidebarLink[] = [
    { name: userLabel, path: home || '/', iconType: House, weight: 'fill' },
    { name: 'Desktop', path: join(home, 'Desktop'), iconType: Desktop, weight: 'fill' },
    { name: 'Documents', path: join(home, 'Documents'), iconType: FileText, weight: 'fill' },
    { name: 'Downloads', path: join(home, 'Downloads'), iconType: DownloadSimple, weight: 'fill' },
    { name: 'Pictures', path: join(home, 'Pictures'), iconType: ImageSquare, weight: 'fill' },
    {
      name: videoFolderLabel,
      path: join(home, videoFolderLabel),
      iconType: VideoCamera,
      weight: 'fill',
    },
    {
      name: 'Music',
      path: join(home, 'Music'),
      iconType: MusicNotes,
      weight: 'fill',
    },
    {
      name: trashLabel,
      path: trashPath,
      iconType: isWindows ? Recycle : Trash,
      weight: 'fill',
    },
  ];

  const systemLinks: SidebarLink[] = [
    // macOS locations
    { name: 'Applications', path: '/Applications', iconType: SquaresFour, weight: 'fill' },
    { name: 'Users', path: '/Users', iconType: UsersThree, weight: 'fill' },
    { name: 'System', path: '/System', iconType: HardDrives, weight: 'regular' },
  ];

  const dragRegionHeightClass = isMac ? 'h-16' : 'h-0';
  const listTopOffsetClass = isMac ? '-mt-8' : '';

  return (
    <div
      ref={sidebarRef}
      className={`flex flex-col h-full bg-app-gray rounded-xl overflow-hidden transition-all duration-200 ${
        isDragOverCombined
          ? 'drag-over ring-2 ring-accent bg-accent/10 shadow-lg shadow-accent/20'
          : ''
      }`}
      style={{ width: sidebarWidth }}
      data-tauri-drag-region={false}
      data-sidebar="true"
      data-drop-zone-id="sidebar"
    >
      {/* Expanded draggable area around traffic lights - covers entire top area */}
      <div
        className={`${dragRegionHeightClass} w-full select-none`}
        data-tauri-drag-region
        onMouseDown={handleDragRegionMouseDown}
      />

      {/* Flat list */}
      <div className={`flex-1 overflow-y-auto px-2 pb-2 space-y-[2px] ${listTopOffsetClass}`}>
        <div
          className="w-full h-[6px]"
          data-tauri-drag-region
          onMouseDown={handleDragRegionMouseDown}
        />
        {/* Favorites section */}
        <div
          className="px-1 py-1 text-xs text-app-muted select-none"
          data-tauri-drag-region
          onMouseDown={handleDragRegionMouseDown}
        >
          Favorites
        </div>
        {/* User directories */}
        {favoriteLinks.map((item) => {
          const isDisabled = item.path == null;
          const isActive = !isDisabled && currentPath === item.path;
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
          );
        })}

        {/* Pinned directories section */}
        {pinnedDirectories.length > 0 && (
          <>
            <div className="px-1 pt-3 pb-1 text-xs text-app-muted select-none">Pinned</div>
            {[...pinnedDirectories]
              .sort((a, b) => a.name.localeCompare(b.name))
              .map((pin) => {
                const isActive = currentPath === pin.path;
                return (
                  <QuickTooltip key={pin.path} text={pin.path}>
                    {({ onBlur, onFocus, onMouseEnter, onMouseLeave, ref }) => (
                      <div
                        ref={ref}
                        onMouseEnter={onMouseEnter}
                        onMouseLeave={onMouseLeave}
                        onFocus={onFocus}
                        onBlur={onBlur}
                        className={`w-full flex items-center gap-2 px-1.5 py-1 rounded-md text-left leading-5 text-[13px] group ${
                          isActive ? 'bg-app-light' : 'hover:bg-app-light/70'
                        }`}
                      >
                        <button
                          onClick={() => navigateTo(pin.path)}
                          className="flex items-center gap-2 flex-1 min-w-0"
                          data-tauri-drag-region={false}
                        >
                          <span className="relative flex-shrink-0 w-5 h-5">
                            <div className="w-full h-full flex items-center justify-center">
                              {createIcon(Folder, 'fill', isActive)}
                            </div>
                            {pin.is_git_repo && (
                              <GitRepoBadge size="sm" style={{ bottom: -2, right: -2 }} />
                            )}
                            {pin.is_symlink && (
                              <SymlinkBadge size="sm" style={{ bottom: -2, left: -2 }} />
                            )}
                          </span>
                          <span className={`truncate ${isActive ? 'text-accent' : ''}`}>
                            {pin.name}
                          </span>
                        </button>

                        <button
                          onClick={(e) => handleUnpinDirectory(pin, e)}
                          className="ml-auto p-0.5 rounded hover:bg-app-light/50 text-app-muted hover:text-accent transition-colors cursor-pointer"
                          title="Remove from pinned"
                          data-tauri-drag-region={false}
                        >
                          <Trash className="w-3.5 h-3.5" weight="regular" />
                        </button>
                      </div>
                    )}
                  </QuickTooltip>
                );
              })}
          </>
        )}

        {/* Cloud Storage section */}
        <div className="px-1 pt-3 pb-1 text-xs text-app-muted select-none">Cloud Storage</div>
        {googleAccounts.map((account) => {
          const gdrivePath = `gdrive://${account.email}/`;
          const isActive = currentPath.startsWith(`gdrive://${account.email}`);
          const isDisconnecting = disconnectingAccounts.has(account.email);
          return (
            <QuickTooltip key={account.email} text={account.email}>
              {({ onBlur, onFocus, onMouseEnter, onMouseLeave, ref }) => (
                <div
                  ref={ref}
                  onMouseEnter={onMouseEnter}
                  onMouseLeave={onMouseLeave}
                  onFocus={onFocus}
                  onBlur={onBlur}
                  className={`w-full flex items-center gap-2 px-1.5 py-1 rounded-md text-left leading-5 text-[13px] group ${
                    isActive ? 'bg-app-light' : 'hover:bg-app-light/70'
                  }`}
                >
                  <button
                    onClick={() => navigateTo(gdrivePath)}
                    className="flex items-center gap-2 flex-1 min-w-0"
                    data-tauri-drag-region={false}
                  >
                    <GoogleLogo
                      className={`w-5 h-5 flex-shrink-0 ${isActive ? 'text-accent' : ''}`}
                      weight="fill"
                    />
                    <span className={`truncate ${isActive ? 'text-accent' : ''}`}>
                      {account.displayName || account.email}
                    </span>
                  </button>

                  <button
                    onClick={(e) => handleDisconnectGoogleAccount(account, e)}
                    disabled={isDisconnecting}
                    className="ml-auto p-0.5 rounded hover:bg-app-light/50 text-app-muted hover:text-accent transition-colors cursor-pointer opacity-0 group-hover:opacity-100"
                    title={isDisconnecting ? 'Disconnecting...' : 'Disconnect account'}
                    data-tauri-drag-region={false}
                  >
                    {isDisconnecting ? (
                      <CircleNotch className="w-3.5 h-3.5 animate-spin" weight="regular" />
                    ) : (
                      <SignOut className="w-3.5 h-3.5" weight="regular" />
                    )}
                  </button>
                </div>
              )}
            </QuickTooltip>
          );
        })}

        {/* Add Google Account button */}
        <button
          onClick={handleAddGoogleAccount}
          disabled={addingGoogleAccount}
          className="w-full flex items-center gap-2 px-1.5 py-1 rounded-md text-left leading-5 text-[13px] text-app-muted hover:bg-app-light/70 hover:text-app-text transition-colors"
          data-tauri-drag-region={false}
        >
          {addingGoogleAccount ? (
            <CircleNotch className="w-5 h-5 animate-spin" weight="regular" />
          ) : (
            <Plus className="w-5 h-5" weight="regular" />
          )}
          <span className="truncate">
            {addingGoogleAccount ? 'Connecting...' : 'Add Google Account...'}
          </span>
        </button>

        {/* Network Shares section */}
        <div className="px-1 pt-3 pb-1 text-xs text-app-muted select-none">Network</div>
        {smbServers.map((server) => {
          const smbPath = `smb://${server.hostname}/`;
          const isActive = currentPath.startsWith(`smb://${server.hostname}`);
          const isDisconnecting = disconnectingSmbServers.has(server.hostname);
          const displayName = server.username
            ? `${server.hostname} (${server.username})`
            : server.hostname;
          return (
            <QuickTooltip key={server.hostname} text={`${server.username}@${server.hostname}`}>
              {({ onBlur, onFocus, onMouseEnter, onMouseLeave, ref }) => (
                <div
                  ref={ref}
                  onMouseEnter={onMouseEnter}
                  onMouseLeave={onMouseLeave}
                  onFocus={onFocus}
                  onBlur={onBlur}
                  className={`w-full flex items-center gap-2 px-1.5 py-1 rounded-md text-left leading-5 text-[13px] group ${
                    isActive ? 'bg-app-light' : 'hover:bg-app-light/70'
                  }`}
                >
                  <button
                    onClick={() => navigateTo(smbPath)}
                    className="flex items-center gap-2 flex-1 min-w-0"
                    data-tauri-drag-region={false}
                  >
                    <ShareNetwork
                      className={`w-5 h-5 flex-shrink-0 ${isActive ? 'text-accent' : ''}`}
                      weight="fill"
                    />
                    <span className={`truncate ${isActive ? 'text-accent' : ''}`}>
                      {displayName}
                    </span>
                  </button>

                  <button
                    onClick={(e) => handleDisconnectSmbServer(server, e)}
                    disabled={isDisconnecting}
                    className="ml-auto p-0.5 rounded hover:bg-app-light/50 text-app-muted hover:text-accent transition-colors cursor-pointer opacity-0 group-hover:opacity-100"
                    title={isDisconnecting ? 'Disconnecting...' : 'Disconnect server'}
                    data-tauri-drag-region={false}
                  >
                    {isDisconnecting ? (
                      <CircleNotch className="w-3.5 h-3.5 animate-spin" weight="regular" />
                    ) : (
                      <SignOut className="w-3.5 h-3.5" weight="regular" />
                    )}
                  </button>
                </div>
              )}
            </QuickTooltip>
          );
        })}

        {/* Add Network Share button */}
        <button
          onClick={() => void invoke('open_smb_connect_window', {})}
          className="w-full flex items-center gap-2 px-1.5 py-1 rounded-md text-left leading-5 text-[13px] text-app-muted hover:bg-app-light/70 hover:text-app-text transition-colors"
          data-tauri-drag-region={false}
        >
          <Plus className="w-5 h-5" weight="regular" />
          <span className="truncate">Add Network Share...</span>
        </button>

        {/* SFTP Servers */}
        {sftpServers.map((server) => {
          const portSuffix = server.port === 22 ? '' : `:${server.port}`;
          const sftpPath = `sftp://${server.username}@${server.hostname}${portSuffix}/`;
          const isActive = currentPath.startsWith(sftpPath) || currentPath.startsWith(`sftp://${server.username}@${server.hostname}:${server.port}/`);
          const serverKey = `${server.hostname}:${server.port}`;
          const isDisconnecting = disconnectingSftpServers.has(serverKey);
          const displayName = server.port === 22
            ? `${server.hostname} (${server.username})`
            : `${server.hostname}:${server.port} (${server.username})`;
          return (
            <QuickTooltip key={serverKey} text={`${server.username}@${server.hostname}${portSuffix}`}>
              {({ onBlur, onFocus, onMouseEnter, onMouseLeave, ref }) => (
                <div
                  ref={ref}
                  onMouseEnter={onMouseEnter}
                  onMouseLeave={onMouseLeave}
                  onFocus={onFocus}
                  onBlur={onBlur}
                  className={`w-full flex items-center gap-2 px-1.5 py-1 rounded-md text-left leading-5 text-[13px] group ${
                    isActive ? 'bg-app-light' : 'hover:bg-app-light/70'
                  }`}
                >
                  <button
                    onClick={() => navigateTo(sftpPath)}
                    className="flex items-center gap-2 flex-1 min-w-0"
                    data-tauri-drag-region={false}
                  >
                    <Terminal
                      className={`w-5 h-5 flex-shrink-0 ${isActive ? 'text-accent' : ''}`}
                      weight="fill"
                    />
                    <span className={`truncate ${isActive ? 'text-accent' : ''}`}>
                      {displayName}
                    </span>
                  </button>

                  <button
                    onClick={(e) => handleDisconnectSftpServer(server, e)}
                    disabled={isDisconnecting}
                    className="ml-auto p-0.5 rounded hover:bg-app-light/50 text-app-muted hover:text-accent transition-colors cursor-pointer opacity-0 group-hover:opacity-100"
                    title={isDisconnecting ? 'Disconnecting...' : 'Disconnect server'}
                    data-tauri-drag-region={false}
                  >
                    {isDisconnecting ? (
                      <CircleNotch className="w-3.5 h-3.5 animate-spin" weight="regular" />
                    ) : (
                      <SignOut className="w-3.5 h-3.5" weight="regular" />
                    )}
                  </button>
                </div>
              )}
            </QuickTooltip>
          );
        })}

        {/* Add SFTP Server button */}
        <button
          onClick={() => void invoke('open_sftp_connect_window', {})}
          className="w-full flex items-center gap-2 px-1.5 py-1 rounded-md text-left leading-5 text-[13px] text-app-muted hover:bg-app-light/70 hover:text-app-text transition-colors"
          data-tauri-drag-region={false}
        >
          <Plus className="w-5 h-5" weight="regular" />
          <span className="truncate">Add SFTP Server...</span>
        </button>

        {/* Locations section */}
        <div className="px-1 pt-3 pb-1 text-xs text-app-muted select-none">System</div>
        {systemLinks.map((item) => {
          const isDisabled = item.path == null;
          const isActive = !isDisabled && currentPath === item.path;
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
          );
        })}

        {/* System drives */}
        {systemDrives.length > 0 && (
          <>
            <div className="px-1 pt-3 pb-1 text-xs text-app-muted select-none">Drives</div>
            {systemDrives.map((drive) => {
              const isActive = currentPath === drive.path;
              const isEjecting = ejectingDrives.has(drive.path);
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
                    {createIcon(HardDrives, 'regular', isActive)}
                    <span className={`truncate ${isActive ? 'text-accent' : ''}`}>
                      {drive.name}
                    </span>
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
              );
            })}
          </>
        )}
      </div>
    </div>
  );
}
