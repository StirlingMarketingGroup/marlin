# 🗺️ Marlin File Browser Roadmap

This roadmap outlines the planned development phases for Marlin, a modern file browser built with Tauri and React.

## 🎯 Vision

Create the most intuitive, performant, and beautiful file browser that feels like a natural extension of your operating system, with a clean modern interface that developers love.

---

## 📅 Development Timeline

### Phase 1: Foundation (Weeks 1-2) ✅ **COMPLETED**

**Goal**: Establish core project structure and basic functionality

#### ✅ Completed Features

- [x] **Project Setup**
  - [x] Tauri 2.0 + React 18 + TypeScript configuration
  - [x] Tailwind CSS with modern dark color palette
  - [x] Development environment and build scripts

- [x] **Core UI Framework**
  - [x] Modern layout with sidebar and main panel
  - [x] Responsive design system
  - [x] Dark theme implementation
  - [x] Custom scrollbars and UI components

- [x] **Basic File Operations**
  - [x] Rust backend with file system APIs
  - [x] Directory reading and navigation
  - [x] File metadata extraction (size, modified date, type)
  - [x] Hidden files detection

- [x] **Navigation System**
  - [x] Editable path bar (always-on editing)
  - [x] Breadcrumb navigation
  - [x] Back/forward history
  - [x] Home directory shortcut

---

## 🚀 Performance Foundation (HIGH PRIORITY)

**Goal**: Build world-class performance for large directories and SMB/NAS networks

### 🎯 Performance Targets
- **Open 50k-file folder**: First paint ≤ 120ms (names only), interactive ≤ 200ms
- **Scroll 50k items**: ≥ 60fps, no long tasks > 16ms in renderer
- **SMB 20k over 10ms RTT**: First 1k names ≤ 400ms, progressive fill thereafter
- **Memory budget**: < 150MB with 50k entries, aggressive LRU eviction
- **Thumbnail budget**: ≤ 4 concurrent per spindle/host, never block UI

### 📁 Directory Read Pipeline
- [ ] **Names-first rendering**: Render immediately on `readdir`, metadata later
- [ ] **Native bulk APIs**: 
  - macOS `getattrlistbulk`
  - Windows `FindFirstFileExW(FIND_FIRST_EX_LARGE_FETCH)`  
  - Linux `getdents64` + `statx` as needed
- [ ] **Adaptive batching**: 512–2048 entries per chunk, backpressure-aware
- [ ] **Virtualized views**: Windowed list/grid, O(visible) DOM, no layout thrash

### 🌐 SMB/NAS Optimization
- [ ] **Network awareness**: Treat `smb://`, mounted shares, UNC paths as high-latency
- [ ] **Timeouts & retries**: Jittered backoff, "degraded mode" (no thumbs, names-only)
- [ ] **I/O budgets**: Coalesce `stat` calls, avoid `realpath` per item, cache dir inodes
- [ ] **Per-host rate limits**: Prevent SMB server overload

### ⚡ Async & Caching
- [ ] **Thread pool**: Per-device concurrency caps, cancel off-screen jobs
- [ ] **Persistent cache**: Dir-entry cache (names + minimal attrs) by dir inode + mtime
- [ ] **Heuristic invalidation**: Watcher events or parent mtime change
- [ ] **Memory control**: Evict metadata/thumbnail LRU aggressively

### 📊 File Watching & Benchmarks
- [ ] **Platform watchers**: macOS FSEvents, Windows ReadDirectoryChangesW, Linux inotify
- [ ] **Event collapsing**: Collapse burst events, debounce updates
- [ ] **Bench harness**: Synthetic dir generator (1k→100k files), repeatable perf tests

---

### Phase 2: Core Features (Weeks 3-4) 🚧 **IN PROGRESS**

**Goal**: Implement essential file management capabilities

#### 🚧 Current Sprint

- [ ] **Core File Operations**
  - [ ] Copy/Cut/Paste with true cut semantics
  - [ ] Inline rename (F2) with stem selected
  - [ ] Trash vs Delete with confirmation
  - [ ] Drag and drop support
  - [ ] Multi-file selection with keyboard (Ctrl/Cmd+click)

- [ ] **View Management**
  - [ ] Grid/list toggle with icon size slider
  - [ ] Per-directory view preferences persistence
  - [ ] Smart sorting (name, size, date, type)
  - [ ] Status line: count, size, free space

- [ ] **Navigation & UX**
  - [ ] Enhanced path bar with paste + Tab autocomplete
  - [ ] Keyboard model: arrows, Enter open, Backspace/Cmd↑ up, Space select
  - [ ] Context menus (right-click actions)
  - [ ] New window opens last dir or user default

#### 📋 Upcoming

- [ ] **Search Functionality**
  - [ ] Real-time search in current directory
  - [ ] File content search (text files)
  - [ ] Search history and filters
  - [ ] Fuzzy search algorithm

---

## 🖼️ Thumbnails & Media

**Goal**: High-performance thumbnail system with intelligent caching

- [ ] **Thumbnail Engine**
  - [ ] Disk cache by content-hash+mtime; sizes 32/64/96/128/256
  - [ ] Worker queue with cancellation; offscreen eviction
  - [ ] Built-ins: JPG/PNG, GIF first frame, SVG, PDF page 1
  - [ ] Optional OS providers: QuickLook, Windows Shell, XDG cache
  - [ ] Fallback icons; invalidate by hash on rename

---

## 🏷️ Batch Rename (Rules Engine)

**Goal**: Powerful, safe batch renaming with preview and rollback

- [ ] **Rules System**
  - [ ] Find/replace (regex), prefix/suffix, numbering `{n}`
  - [ ] Tokens: `{name}{ext}{parent}{yyyy}{MM}{dd}`
  - [ ] Dry-run diff with collision detection + illegal char fixes per OS
  - [ ] Apply with rollback capability

---

## 🤖 AI Operations Framework  

**Goal**: Safe AI-powered file operations with strict JSON schema

- [ ] **Safety-First Architecture**
  - [ ] JSON-only system prompt; reject non-schema output
  - [ ] Schema: `rename`, `image.resize`, `image.trim`, `image.convert`, `image.compress`, `move`, `copy`
  - [ ] Never execute raw shell commands

- [ ] **Provider Integration**
  - [ ] Provider config: OpenAI/Anthropic/Gemini API keys
  - [ ] Vision caption cache (perceptual hash)
  - [ ] Freehand prompt → structured JSON plan

- [ ] **User Control**
  - [ ] Dry-run UI with per-operation toggles
  - [ ] Export/import plan as JSON
  - [ ] Quotas per apply; paging for huge plans

### 🎨 Built-in Presets
- [ ] SEO rename from image content
- [ ] Resize ≤2000px (preserve aspect)
- [ ] Trim whitespace margins
- [ ] Convert to WebP with target size
- [ ] Generate `-thumb` 512px square white background

---

## 🎨 Image Operations (Native)

**Goal**: Fast native image processing without external dependencies

- [ ] **Core Operations**
  - [ ] Resize ≤ W×H, preserve aspect, skip if smaller
  - [ ] Trim transparent/solid margins (threshold configurable)
  - [ ] Convert PNG↔JPEG↔WebP↔AVIF
  - [ ] PNG optimize; JPEG recompress with quality slider

- [ ] **Performance**
  - [ ] Streamed processing with progress bars
  - [ ] Cancellation support for large batches
  - [ ] Memory-efficient processing (no full load)

---

## 💼 Productivity Features

**Goal**: Professional workflow enhancements

- [ ] **Multi-Window & Tabs**
  - [ ] Dual-pane toggle for side-by-side comparison
  - [ ] Tabs with session restore
  - [ ] New window opens last directory or user default

- [ ] **Enhanced Navigation**  
  - [ ] In-folder quick filter (non-indexed, real-time)
  - [ ] Favorites/bookmarks system
  - [ ] Copy path (Unix, Windows, file:// formats)

### Phase 3: Advanced Features (Weeks 5-6) 📋 **PLANNED**

**Goal**: Add power-user features and customization

#### 🔧 Advanced Operations

- [ ] **System Integration**
  - [ ] File compression/extraction (zip, tar, etc.)
  - [ ] File permissions management with inline error fixes
  - [ ] Symlink/junction badges; safe operations on links

- [ ] **System Integration**
  - [ ] Default application associations
  - [ ] "Open with" menu  
  - [ ] System trash integration
  - [ ] Network drive support with timeout handling

#### 🎨 Customization

- [ ] **Theme System**
  - [ ] Light mode implementation
  - [ ] Custom color themes
  - [ ] System theme sync (Windows/macOS)
  - [ ] High contrast accessibility mode

- [ ] **Layout Customization**
  - [ ] Resizable panels
  - [ ] Collapsible sidebar
  - [ ] Customizable toolbar
  - [ ] Multiple tabs support

---

### Phase 4: Performance & Polish (Weeks 7-8) 🚀 **FUTURE**

**Goal**: Optimize performance and add finishing touches

#### ⚡ Performance

- [ ] **File System Optimization**
  - [ ] Lazy loading for large directories
  - [ ] Virtual scrolling for file lists
  - [ ] Background directory indexing
  - [ ] Caching frequently accessed folders

- [ ] **Memory Management**
  - [ ] Efficient image thumbnail generation
  - [ ] Memory usage optimization
  - [ ] Background task cleanup
  - [ ] Startup time optimization

#### 🛡️ Reliability

- [ ] **Error Handling**
  - [ ] Graceful permission error handling
  - [ ] Network failure recovery
  - [ ] Corrupted file handling
  - [ ] User-friendly error messages

- [ ] **Testing & Quality**
  - [ ] Unit tests for Rust backend
  - [ ] Integration tests for file operations
  - [ ] Cross-platform compatibility testing
  - [ ] Accessibility compliance (WCAG 2.1)

---

## 🛡️ Robustness & Error Handling

**Goal**: Bulletproof reliability across all platforms and edge cases

- [ ] **Cross-Platform Compatibility**
  - [ ] Windows long paths support (>260 chars)
  - [ ] macOS Unicode NFC normalization  
  - [ ] Linux extended attributes and permissions

- [ ] **Network & Performance**
  - [ ] Network share timeout/retry with exponential backoff
  - [ ] SMB connection pooling and keep-alive
  - [ ] Graceful degradation on slow connections

- [ ] **Error Recovery**
  - [ ] Inline permission errors with fix hints ("Run as Admin", "Change permissions")
  - [ ] Corrupted file detection and recovery suggestions
  - [ ] Disk space monitoring with warnings
  - [ ] Operation rollback on partial failures

---

## 📦 Packaging & Auto-Updates

**Goal**: Professional CI/CD pipeline with signed, automatic updates

- [ ] **Build Pipeline**
  - [ ] GitHub Actions on tag `v*`; matrix build
  - [ ] macOS `.dmg` with notarization
  - [ ] Windows `.msi` with code signing
  - [ ] Linux `.AppImage` + `.deb`/`.rpm` packages

- [ ] **Distribution**
  - [ ] GitHub Releases with artifacts + `latest.json`
  - [ ] Optional: Homebrew Cask, winget, AppImageHub
  - [ ] Chocolatey package for Windows

- [ ] **Auto-Updater**
  - [ ] Tauri updater: signed, checks `latest.json` at startup/manual
  - [ ] Update keypair: public in config, private in CI secrets
  - [ ] Background downloads, install on restart
  - [ ] Rollback capability for failed updates

---

## ⚙️ Settings & Configuration

**Goal**: Minimal UI with powerful JSON config for power users

- [ ] **User Preferences**
  - [ ] Start directory, default view/sort preferences
  - [ ] Enable/disable OS thumbnail providers
  - [ ] Toggle heavy converters + max input file size limits
  - [ ] AI provider API key management

- [ ] **Configuration**
  - [ ] JSON config file with schema validation
  - [ ] Minimal settings UI for common options
  - [ ] Import/export settings for team deployments

---

### Phase 5: Extensions & Ecosystem (Weeks 9+) 🔮 **FUTURE**

**Goal**: Create an extensible platform for community contributions

#### 🔌 Plugin System

- [ ] **Architecture**
  - [ ] Plugin API design
  - [ ] Sandboxed plugin execution
  - [ ] Plugin marketplace/registry
  - [ ] Hot-reloading for development

- [ ] **Core Plugins**
  - [ ] Git integration (show status, diff)
  - [ ] FTP/SFTP client
  - [ ] Cloud storage integration (Dropbox, Google Drive)
  - [ ] Archive manager
  - [ ] Terminal integration

#### 🌍 Platform Features

- [ ] **Cross-Platform**
  - [ ] Windows native integration
  - [ ] macOS Finder replacement option
  - [ ] Linux desktop environment integration
  - [ ] Mobile companion app (view-only)

- [ ] **Advanced Features**
  - [ ] Dual-pane mode
  - [ ] File comparison tools
  - [ ] Batch renaming with regex
  - [ ] Advanced search with file content indexing

---

## 🎯 Success Metrics

### Performance Targets
- **Large Directories**: Open 50k files ≤ 120ms first paint, ≤ 200ms interactive
- **SMB/NAS Performance**: 20k files over 10ms RTT, first 1k names ≤ 400ms  
- **Scroll Performance**: ≥ 60fps with 50k items, no long tasks > 16ms
- **Memory Budget**: < 150MB with 50k entries, aggressive LRU eviction
- **Bundle Size**: < 20MB installed (vs 200MB+ Electron apps)
- **Startup Time**: < 1 second cold start

### User Experience Goals
- **Accessibility**: WCAG 2.1 AA compliance
- **Internationalization**: Support for 10+ languages
- **User Satisfaction**: 4.5+ stars on app stores
- **Community**: 1000+ GitHub stars, 50+ contributors

### Technical Excellence
- **Test Coverage**: > 85% for critical paths
- **Performance**: Passes Core Web Vitals metrics  
- **Security**: Regular security audits
- **Cross-Platform**: Consistent experience across OS

---

## ✅ Acceptance Tests

**Goal**: Automated verification of core performance and functionality

- [ ] **Performance Benchmarks**
  - [ ] 50k local files: first paint ≤ 120ms; smooth scroll; memory < 150MB
  - [ ] 20k SMB files: progressive load; no UI stalls; degraded mode works
  - [ ] Thumbnail streaming without blocking; cancel on fast scroll

- [ ] **Core Functionality**
  - [ ] Path paste → Enter navigates or shows clear error
  - [ ] Batch rename dry-run results exactly match apply results
  - [ ] AI operations produce valid JSON; collisions resolved as `-1`, `-2` 
  - [ ] Cancel mid-apply leaves filesystem in consistent state

- [ ] **Distribution**
  - [ ] Fresh install via DMG/MSI/AppImage works on clean systems
  - [ ] Auto-update N→N+1 works; rejects tampered updates
  - [ ] Code signing verification passes on all platforms

---

## 🚫 Non-Goals

**What Marlin will NOT include to maintain focus and performance:**

- **No preview sidebar** - Use native system preview instead
- **No cloud client** - Focus on local and network storage only
- **No system indexer** - Keep it lightweight and focused
- **No plugin marketplace** - Maintain security and simplicity

---

## 🤝 Contributing

We welcome contributions to any phase of the roadmap! Here's how you can help:

### 🎯 High Priority Areas
- **File Operations**: Copy/paste, drag & drop
- **Performance**: Large directory handling
- **Accessibility**: Screen reader support
- **Testing**: Automated test coverage

### 📋 Feature Requests
- Create GitHub issues with the `feature-request` label
- Include detailed use cases and mockups
- Consider implementation complexity and user benefit

### 🐛 Bug Reports
- Use the bug report template
- Include reproduction steps and system info
- Test on multiple operating systems when possible

### 📝 Documentation
- Improve README and developer guides
- Create video tutorials
- Write blog posts about features

---

## 📞 Contact & Feedback

- **GitHub Issues**: [Feature requests & bug reports](https://github.com/StirlingMarketingGroup/marlin/issues)
- **Discussions**: [Community discussions](https://github.com/StirlingMarketingGroup/marlin/discussions)
- **Email**: [team@marlin.dev](mailto:team@marlin.dev) (Coming Soon)

---

*This roadmap is a living document and may be adjusted based on community feedback, technical constraints, and emerging opportunities.*

**Last Updated**: September 2025