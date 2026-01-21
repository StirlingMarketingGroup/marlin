import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import FolderSizeWindow from './windows/FolderSizeWindow';
import ArchiveProgressWindow from './windows/ArchiveProgressWindow';
import DeleteProgressWindow from './windows/DeleteProgressWindow';
import ClipboardProgressWindow from './windows/ClipboardProgressWindow';
import SmbConnectWindow from './windows/SmbConnectWindow';
import PermissionsWindow from './windows/PermissionsWindow';
import './index.css';

const params = new URLSearchParams(window.location.search);
const view = params.get('view');

const Root =
  view === 'folder-size'
    ? FolderSizeWindow
    : view === 'archive-progress'
      ? ArchiveProgressWindow
      : view === 'delete-progress'
        ? DeleteProgressWindow
        : view === 'clipboard-progress'
          ? ClipboardProgressWindow
          : view === 'smb-connect'
            ? SmbConnectWindow
            : view === 'permissions'
              ? PermissionsWindow
              : App;

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>
);
