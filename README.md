# Marlin

A fast, minimal file browser built with Tauri.

## Screenshots

![Thumbnail grid view](screenshots/thumbs.png)

![List view](screenshots/list.png)

## Features

- Grid view with thumbnails and list view
- Editable path bar with autocomplete
- Collapsible sidebar with file tree
- Per-directory view preferences
- Hidden files toggle
- Drag and drop to external apps
- Dark theme
- Keyboard navigation

## Tech

- **Frontend**: React 18, TypeScript, Tailwind CSS, Zustand
- **Backend**: Rust, Tauri 2.0, Tokio

## Install

Download from [Releases](https://github.com/StirlingMarketingGroup/marlin/releases):

- **macOS**: `.dmg`
- **Windows**: `.msi`
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
