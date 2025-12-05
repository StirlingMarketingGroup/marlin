# 🐠 Marlin File Browser

<div align="center">
  
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Tauri](https://img.shields.io/badge/Tauri-2.0-blue.svg)](https://tauri.app/)
[![React](https://img.shields.io/badge/React-18-blue.svg)](https://reactjs.org/)
[![Rust](https://img.shields.io/badge/Rust-1.77+-orange.svg)](https://www.rust-lang.org/)
[![TypeScript](https://img.shields.io/badge/TypeScript-5.6-blue.svg)](https://www.typescriptlang.org/)
[![Stars](https://img.shields.io/github/stars/StirlingMarketingGroup/marlin?style=social)](https://github.com/StirlingMarketingGroup/marlin/stargazers)

_A modern, sleek file browser with native performance and beautiful UI_

[🚀 Features](#features) • [📦 Installation](#installation) • [🛠️ Development](#development) • [🎨 Screenshots](#screenshots) • [🤝 Contributing](#contributing)

</div>

## ✨ Features

### 🎨 **Modern Design**

- **Dark-first UI** with system theme detection
- **Smooth animations** and clean modern interface
- **Customizable sidebar** with collapsible file tree
- **Native system colors** integration

### 🚀 **Lightning Fast Performance**

- **10x smaller** than Electron alternatives (~10-20 MiB vs 200+ MiB)
- **Native performance** with Rust backend
- **Memory efficient** - uses system WebView instead of Chromium
- **Instant file operations** with async processing

### 📁 **Powerful File Management**

- **Editable path bar** with autocomplete (always-on editing)
- **Multiple view modes**: Grid, List, and Details
- **Smart sorting** by name, size, date, or type
- **Per-directory preferences** that persist
- **Hidden files toggle** with visual distinction
- **Advanced file operations**: Copy, Cut, Paste, Rename, Delete

### 🔧 **Developer Experience**

- **Cross-platform**: Windows, macOS, and Linux
- **Modern tech stack**: Tauri 2.0 + React 18 + TypeScript
- **Extensible architecture** with plugin support
- **Hot reload** development with Vite

## 🏗️ Architecture

Marlin uses **Tauri** instead of Electron for superior performance:

```
┌─────────────────┐    ┌──────────────────┐
│   Frontend      │    │   Backend        │
│                 │    │                  │
│ React 18        │◄──►│ Rust (Tokio)     │
│ TypeScript      │    │ File Operations  │
│ Tailwind CSS    │    │ System APIs      │
│ Zustand         │    │ Native Perf      │
└─────────────────┘    └──────────────────┘
```

### Why Tauri over Electron?

| Feature          | Marlin (Tauri)   | Electron Apps   |
| ---------------- | ---------------- | --------------- |
| **Bundle Size**  | ~10-20 MiB       | ~200+ MiB       |
| **Memory Usage** | ~50-80 MiB       | ~200-400 MiB    |
| **Startup Time** | <1 second        | 2-5 seconds     |
| **Security**     | Sandboxed + Rust | Node.js runtime |
| **Native Feel**  | System WebView   | Chromium        |

## 📦 Installation

### Desktop installers

Publishing a Git tag that starts with `v` (for example `v0.1.0`) triggers our release
workflow. Once the build finishes you will find platform installers on the
[Releases](https://github.com/StirlingMarketingGroup/marlin/releases) page:

- **macOS**: `Marlin_<version>_universal.dmg`
- **Windows**: `Marlin_<version>_x64_en-US.msi` (Intel/AMD) and `Marlin_<version>_arm64_en-US.msi`
- **Linux**: `Marlin_<version>_amd64.AppImage`/`.deb` and `Marlin_<version>_aarch64.AppImage`/`_arm64.deb`

### Command line installs

Use the latest published release by querying the GitHub API in each snippet below.

**YOLO (macOS & Linux x86_64/arm64)**

```bash
curl -fsSL https://raw.githubusercontent.com/StirlingMarketingGroup/marlin/main/scripts/install.sh | bash
```

> Installs to `/Applications` on macOS and `/usr/local/bin/marlin` on Linux using
> the latest release. Review the script before piping to `bash` if you prefer.

**npm global CLI (macOS, Windows, Linux)**

```bash
npm install -g github:StirlingMarketingGroup/marlin
marlin-install
```

> Installs a cross-platform Node.js helper (requires Node.js 18+) that fetches and
> runs the latest Marlin desktop installer for your OS. Re-run `marlin-install`
> whenever you want to pick up a newer release. On macOS you may need to prefix
> the command with `sudo` so it can write to `/Applications`.

**macOS (Terminal)**

```bash
TAG=$(curl -fsSL https://api.github.com/repos/StirlingMarketingGroup/marlin/releases/latest \
  | python3 -c "import sys, json; print(json.load(sys.stdin)['tag_name'])")
VERSION=${TAG#v}
curl -L -o Marlin.dmg \
  "https://github.com/StirlingMarketingGroup/marlin/releases/download/$TAG/Marlin_${VERSION}_universal.dmg"
hdiutil attach Marlin.dmg
sudo cp -R /Volumes/Marlin/Marlin.app /Applications # may prompt for your password
hdiutil detach /Volumes/Marlin
```

**Windows (PowerShell)**

```powershell
$tag = (Invoke-RestMethod https://api.github.com/repos/StirlingMarketingGroup/marlin/releases/latest).tag_name
$version = $tag.TrimStart('v')
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x64' }
$asset = "Marlin_${version}_${arch}_en-US.msi"
Invoke-WebRequest "https://github.com/StirlingMarketingGroup/marlin/releases/download/$tag/$asset" `
  -OutFile "$env:TEMP\$asset"
Start-Process msiexec.exe -Wait -ArgumentList "/i `"$env:TEMP\$asset`""
```

**Linux (AppImage)**

```bash
TAG=$(curl -fsSL https://api.github.com/repos/StirlingMarketingGroup/marlin/releases/latest \
  | python3 -c "import sys, json; print(json.load(sys.stdin)['tag_name'])")
ARCH=$(uname -m)
case "$ARCH" in
  x86_64|amd64)
    SUFFIX="amd64"
    ;;
  aarch64|arm64)
    SUFFIX="aarch64"
    ;;
  *)
    echo "Unsupported architecture: $ARCH" >&2
    exit 1
    ;;
esac
ASSET="Marlin_${TAG#v}_${SUFFIX}.AppImage"
curl -L -o "$ASSET" \
  "https://github.com/StirlingMarketingGroup/marlin/releases/download/$TAG/$ASSET"
chmod +x "$ASSET"
sudo mv "$ASSET" /usr/local/bin/marlin # or move it anywhere on your PATH
marlin
```

Prefer to build locally or script your own pipeline? See the
[Development](#development) section for source-based setup instructions.

## 🛠️ Development

### Prerequisites

- [Node.js](https://nodejs.org/) 18+
- [Rust](https://rustup.rs/) 1.77+
- [Tauri CLI](https://tauri.app/v1/guides/getting-started/prerequisites)
- Linux (Debian/Ubuntu) builds also need the GTK/WebKit toolchain:

  ```bash
  sudo apt update
  sudo apt install build-essential pkg-config libssl-dev \
    libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev
  sudo apt install libwebkit2gtk-4.1-dev || sudo apt install libwebkit2gtk-4.0-dev
  ```

  > Use whichever `libwebkit2gtk` version your distro provides (24.04 ships 4.1,
  > while older LTS releases still use 4.0).

### Setup

```bash
# Clone the repository
git clone https://github.com/StirlingMarketingGroup/marlin.git
cd marlin

# Install dependencies
npm install

# Start development server
npm run tauri dev
```

> Tip: running inside a Snap-distributed terminal/editor can inject an old
> `LD_LIBRARY_PATH`. Launch the dev server with `env -u LD_LIBRARY_PATH npm run tauri dev`
> if you hit glibc-related symbol errors.

### Build

```bash
# Build for production
npm run tauri build
```

## 🚀 Release process

1. Make sure `npm run check` passes on the commit you plan to tag.
2. Create an annotated tag that follows the `v*` convention, for example:
   `git tag -a v0.1.0 -m "Marlin 0.1.0"`.
3. Push the tag: `git push origin v0.1.0`.
4. Watch the **Release** workflow; when it finishes the DMG, MSI, AppImage, and Debian
   installers will be attached to the tag's release page.
5. The **Sync Version** workflow runs right after publishing and bumps
   `package.json`, `package-lock.json`, and `src-tauri/tauri.conf.json` to match the tag.
6. (Optional) Run `marlin-install` or the YOLO script on each platform/architecture
   (x64 + ARM) to spot-check the uploaded binaries.

After pushing `v0.1.0` you'll have the first pre-1.0 release ready to share.

The installation snippets above always fetch the latest release tag, so no README
updates are needed after tagging.

> Every pull request runs the release workflow in a dry-run matrix to make sure the
> desktop bundles continue to build across macOS, Windows (x64/ARM64), and Linux
> (x64/ARM64) before you tag a real release.

The YOLO installer script and the npm helper consume the same release assets; if you
change asset names adjust `scripts/install.sh` and `scripts/install.mjs` accordingly
before tagging.

### Project Structure

```
marlin/
├── src/                # React frontend
│   ├── components/     # UI components
│   ├── hooks/          # Custom hooks
│   ├── store/          # State management
│   └── types/          # TypeScript types
├── src-tauri/          # Rust backend
│   ├── src/            # Rust source code
│   └── Cargo.toml      # Rust dependencies
├── public/             # Static assets
└── docs/               # Documentation
```

## 🎨 Screenshots

### Main Interface

![Main Interface](docs/screenshots/main-interface.png)

### Grid View

![Grid View](docs/screenshots/grid-view.png)

### Dark Theme

![Dark Theme](docs/screenshots/dark-theme.png)

## 🗺️ Roadmap

See our [detailed roadmap](ROADMAP.md) for planned features and development timeline.

### Phase 1: Foundation ✅

- [x] Basic Tauri + React setup
- [x] Modern UI design
- [x] File system operations
- [x] Editable path bar

### Phase 2: Core Features 🚧

- [ ] File operations (copy, cut, paste)
- [ ] Search functionality
- [ ] Context menus
- [ ] Keyboard shortcuts

### Phase 3: Advanced Features 📋

- [ ] File preview panel
- [ ] Plugin system
- [ ] Themes and customization
- [ ] Performance optimizations

## 🤝 Contributing

We welcome contributions! Please see our [Contributing Guide](CONTRIBUTING.md) for details.

### Quick Start

1. Fork the repository
2. Create a feature branch: `git checkout -b feature/amazing-feature`
3. Commit your changes: `git commit -m 'Add amazing feature'`
4. Push to the branch: `git push origin feature/amazing-feature`
5. Open a Pull Request

### Development Guidelines

- Follow [Rust](https://doc.rust-lang.org/1.0.0/style/style/naming/README.html) and [TypeScript](https://typescript-eslint.io/) style guides
- Add tests for new features
- Update documentation as needed
- Ensure CI passes before submitting PR

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgments

- [Tauri](https://tauri.app/) - For the amazing framework
- [Lucide](https://lucide.dev/) - For beautiful icons
- [Tailwind CSS](https://tailwindcss.com/) - For utility-first CSS

---

<div align="center">

**[⭐ Star this repo](https://github.com/StirlingMarketingGroup/marlin) if you find it useful!**

Made with ❤️ by the Marlin team

</div>
