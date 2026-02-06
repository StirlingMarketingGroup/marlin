import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import FolderSizeWindow from './windows/FolderSizeWindow';
import ArchiveProgressWindow from './windows/ArchiveProgressWindow';
import CompressProgressWindow from './windows/CompressProgressWindow';
import DeleteProgressWindow from './windows/DeleteProgressWindow';
import ClipboardProgressWindow from './windows/ClipboardProgressWindow';
import SmbConnectWindow from './windows/SmbConnectWindow';
import SftpConnectWindow from './windows/SftpConnectWindow';
import PermissionsWindow from './windows/PermissionsWindow';
import PreferencesWindow from './windows/PreferencesWindow';
import ConflictWindow from './windows/ConflictWindow';
import { useThemePreference, useThemeSync } from '@/hooks/useTheme';
import { useThemeRegistry } from '@/hooks/useThemeRegistry';
import './index.css';

const params = new URLSearchParams(window.location.search);
const view = params.get('view');

const Root =
  view === 'folder-size'
    ? FolderSizeWindow
    : view === 'archive-progress'
      ? ArchiveProgressWindow
      : view === 'compress-progress'
        ? CompressProgressWindow
        : view === 'delete-progress'
          ? DeleteProgressWindow
          : view === 'clipboard-progress'
            ? ClipboardProgressWindow
            : view === 'smb-connect'
              ? SmbConnectWindow
              : view === 'sftp-connect'
                ? SftpConnectWindow
                : view === 'permissions'
                  ? PermissionsWindow
                  : view === 'preferences'
                    ? PreferencesWindow
                    : view === 'conflict'
                      ? ConflictWindow
                    : App;

function ThemeBridge() {
  const preference = useThemePreference();
  const themes = useThemeRegistry(preference.customThemes);
  useThemeSync(preference, themes);
  return null;
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <ThemeBridge />
    <Root />
  </React.StrictMode>
);
