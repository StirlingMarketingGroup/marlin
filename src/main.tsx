import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import FolderSizeWindow from './windows/FolderSizeWindow';
import './index.css';

const params = new URLSearchParams(window.location.search);
const view = params.get('view');

const Root = view === 'folder-size' ? FolderSizeWindow : App;

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>
);
