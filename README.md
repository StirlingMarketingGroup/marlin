# Marlin

A fast, no-nonsense file browser. Gets out of your way and just does files well.

## Screenshots

![Thumbnail grid view](screenshots/thumbs.png)

![List view](screenshots/list.png)

## Features

### Core

- Grid view with thumbnails and list view
- Editable path bar with autocomplete
- Type-to-filter: just start typing to filter files instantly
- Marquee selection: click and drag to select multiple files
- Select all with Cmd/Ctrl+A
- Collapsible sidebar with file tree
- **Pinnable favorites**: Drag any folder to the sidebar to pin it for quick access—works with local folders, SMB shares, and Google Drive
- Per-directory view preferences
- Hidden files toggle
- Drag and drop to external apps
- Copy file paths—name or full path (looking at you, Finder!)
- Paste screenshots directly from clipboard as files
- Trash with undo support
- Double-click archives to extract
- Calculate folder sizes
- Keyboard navigation
- Dark theme (light theme coming soon!)
- **Accent color**: Follows your system accent color by default, or choose a custom color in Preferences (Cmd/Ctrl+,)

### Smart Thumbnails

Marlin generates thumbnails for a wide variety of file types—locally and on remote providers:

- **Images**: JPG, PNG, GIF, WebP, BMP, TIFF, TGA, ICO
- **Design files**: PDF, EPS, Adobe Illustrator
- **Vector**: SVG
- **3D models**: STL with shaded rendering
- **Video**: MP4, MOV, MKV, WebM, AVI, and more (frame preview)
- **macOS apps**: .app, .dmg, .pkg icons

Image dimensions shown under thumbnails. Image-heavy folders automatically switch to grid view.

### Google Drive Integration

- **Account Management**: Connect multiple Google accounts from the sidebar
- **Full Navigation**: Browse My Drive, Shared Drives, and Shared with me
- **URL Paste**: Paste any Google Drive folder URL into the path bar to navigate directly
- **Thumbnails**: Full thumbnail support for all file types

### SMB / Network Shares

- Browse Windows shares, NAS devices, and macOS file sharing
- **Blazing fast**: Handles folders with 100k+ files with ease
- **Remote thumbnails**: Generate thumbnails for files on network shares
- Automatic reconnection and credential management

### For Developers

- **Git folder badges**: Instantly spot Git repositories in any directory
- **Git status bar**: See branch name, dirty/clean state, ahead/behind counts
- **Click to open remote**: Jump to GitHub/GitLab/etc. directly from the status bar

## Tech

- **Frontend**: React 18, TypeScript, Tailwind CSS, Zustand
- **Backend**: Rust, Tauri 2.0, Tokio

## Install

Download from [Releases](https://github.com/StirlingMarketingGroup/marlin/releases):

- **macOS**: `.dmg`
- **Windows**: `.msi` (coming soon)
- **Linux**: `.AppImage` or `.deb`

Or use the install script:

```bash
curl -fsSL https://raw.githubusercontent.com/StirlingMarketingGroup/marlin/main/scripts/install.sh | bash
```

## Development

### Requirements

- Node.js 18+
- Rust 1.77+
- Linux only: GTK/WebKit toolchain

```bash
# Linux (Debian/Ubuntu)
sudo apt update
sudo apt install build-essential pkg-config libssl-dev \
  libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev
sudo apt install libwebkit2gtk-4.1-dev || sudo apt install libwebkit2gtk-4.0-dev
```

### Setup

```bash
git clone https://github.com/StirlingMarketingGroup/marlin.git
cd marlin
npm install
npm run tauri dev
```

### Build

```bash
npm run tauri build
```

### Project Structure

```
marlin/
├── src/                # React frontend
│   ├── components/
│   ├── hooks/
│   ├── store/
│   └── types/
├── src-tauri/          # Rust backend
│   └── src/
└── public/
```

## Contributing

1. Fork the repo
2. Create a branch: `git checkout -b feature/thing`
3. Make changes and commit
4. Push and open a PR

Please run `npm run build` and `cargo build` before submitting to catch errors.

## License

MIT
