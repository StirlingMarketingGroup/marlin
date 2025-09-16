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

- **10x smaller** than Electron alternatives (~10-20MB vs 200MB+)
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
| **Bundle Size**  | ~10-20 MB        | ~200+ MB        |
| **Memory Usage** | ~50-80 MB        | ~200-400 MB     |
| **Startup Time** | <1 second        | 2-5 seconds     |
| **Security**     | Sandboxed + Rust | Node.js runtime |
| **Native Feel**  | System WebView   | Chromium        |

## 📦 Installation

### Download Release

1. Go to [Releases](https://github.com/StirlingMarketingGroup/marlin/releases)
2. Download for your platform:
   - **Windows**: `Marlin_x.x.x_x64_en-US.msi`
   - **macOS**: `Marlin_x.x.x_universal.dmg`
   - **Linux**: `marlin_x.x.x_amd64.deb` or `marlin_x.x.x_amd64.AppImage`

### Package Managers

```bash
# macOS (Homebrew)
brew install --cask marlin

# Windows (Chocolatey)
choco install marlin

# Linux (Snap)
snap install marlin
```

## 🛠️ Development

### Prerequisites

- [Node.js](https://nodejs.org/) 18+
- [Rust](https://rustup.rs/) 1.77+
- [Tauri CLI](https://tauri.app/v1/guides/getting-started/prerequisites)

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

### Build

```bash
# Build for production
npm run tauri build
```

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
