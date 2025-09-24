import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import FolderSizeWindow from './windows/FolderSizeWindow';
import ArchiveProgressWindow from './windows/ArchiveProgressWindow';
import './index.css';

const params = new URLSearchParams(window.location.search);
const view = params.get('view');

const Root =
  view === 'folder-size'
    ? FolderSizeWindow
    : view === 'archive-progress'
      ? ArchiveProgressWindow
      : App;

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>
);
