# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Development Commands

### Primary Development Workflow
- `npm run tauri dev` - Start the full Tauri app with hot reload (frontend + backend)
- `npm run tauri build` - Build production desktop app for current platform
- `npm run dev` - Start Vite dev server for frontend-only development
- `npm run build` - Build frontend assets to `dist/`
- `cd src-tauri && cargo build` - Build Rust backend only
- `cd src-tauri && cargo check` - Fast compile check for Rust code

### Platform-Specific Notes
- macOS: Requires Xcode command line tools for native drag functionality
- All platforms: Requires Rust 1.77+ and Node.js 18+

## Development Server Management

**IMPORTANT: DO NOT kill running development servers (`npm run tauri dev`)**

- The dev server has excellent hot-reload capabilities for both frontend and Rust changes
- Changes to TypeScript/React files reload instantly
- Changes to Rust files trigger automatic recompilation and app restart
- If you believe a manual restart is needed, ask the developer first
- The server is very robust and rarely needs manual intervention

## Code Quality Verification

**ALWAYS run test builds to catch ALL errors and warnings before completing work:**

### Required Build Verification Steps
1. **Frontend Build**: `npm run build`
   - Fix all TypeScript errors and warnings
   - Ensure Vite build succeeds without issues

2. **Backend Build**: `cd src-tauri && cargo build`
   - Fix all Rust compilation errors
   - Fix all Rust warnings (warnings often indicate potential bugs)
   - Ensure clean compilation

3. **Full Production Build**: `npm run tauri build` (when appropriate)
   - Verifies end-to-end build pipeline
   - Only run for major changes to avoid long build times

### Why This Matters
- TypeScript warnings often reveal type safety issues
- Rust warnings can indicate memory safety or logic problems
- Production builds may catch issues not visible in dev mode
- Clean builds ensure code quality and maintainability

## Architecture Overview

### Technology Stack
- **Frontend**: React 18 + TypeScript + Tailwind CSS + Vite
- **Backend**: Rust + Tauri 2.0 + Tokio for async operations
- **State Management**: Zustand (`src/store/useAppStore.ts`)
- **Icons**: Phosphor React + VSCode Icons + native system icons on macOS

### Key Backend Modules
- `commands.rs` - Main Tauri commands API (file operations, navigation, thumbnails)
- `thumbnails/` - Advanced thumbnail generation system with caching and queuing
- `native_drag.rs` - Platform-specific drag-and-drop implementation
- `fs_utils.rs` - Cross-platform file system utilities
- `macos_icons.rs` - macOS-specific app icon extraction

### Key Frontend Architecture
- `App.tsx` - Main layout with sidebar, path bar, and file display
- `store/useAppStore.ts` - Global state with navigation history, file lists, preferences
- `components/FileGrid.tsx` & `FileList.tsx` - Main file display components with drag support
- `components/PathBar.tsx` - Editable path navigation with autocomplete

## Important Implementation Details

### Thumbnail System
- Located in `src-tauri/src/thumbnails/`
- Uses content-hash + mtime for cache keys
- Supports PNG, JPEG, WebP, GIF, PDF, SVG
- Concurrent generation with configurable limits (4 by default)
- LRU cache eviction with memory management

### Drag & Drop
- macOS native implementation in `native_drag.rs` using Cocoa/Objective-C
- Custom drag images and visual feedback
- Supports dragging files to external applications
- Integration points in FileGrid.tsx and FileList.tsx

### File Operations & Navigation
- Navigation state managed in Zustand store with history
- Path validation and resolution in backend
- Per-directory view preferences (grid/list, sort, hidden files)
- Async file loading with cancellation support

### State Management Patterns
- Zustand store (`useAppStore`) for global app state
- File selection state with multi-select support
- View preferences persisted per directory
- Icon generation queue with concurrency limiting (macOS)

## Testing & Code Quality

### Current Status
- No formal test suite configured yet
- Manual testing across macOS, Windows, Linux
- Performance testing with large directories (50k+ files target)

### When Adding Tests
- Frontend: Use Vitest + React Testing Library
- Backend: Use `cargo test` with `*_test.rs` files
- Focus on critical paths: navigation, file operations, thumbnail generation

## Performance Considerations

### Large Directory Handling
- Virtual scrolling planned for 50k+ files
- Thumbnail generation uses worker queues
- Memory management with LRU eviction
- Progressive loading for network shares (SMB/NAS)

### Platform-Specific Optimizations
- macOS: Uses `getattrlistbulk` for bulk metadata (planned)
- Windows: `FindFirstFileExW` with large fetch (planned)
- Linux: `getdents64` + selective `statx` (planned)

## Common Development Tasks

### Adding New File Operations
1. Add Rust command in `src-tauri/src/commands.rs`
2. Add TypeScript types in `src/types/index.ts`
3. Update store actions in `src/store/useAppStore.ts`
4. Add UI integration in relevant components

### Adding New View Components
- Follow existing patterns in `src/components/`
- Use Tailwind utility classes for styling
- Integrate with Zustand store for state
- Consider responsive design and dark theme support

### Platform-Specific Features
- Add conditional compilation in Rust (`#[cfg(target_os = "...")]`)
- Use optional dependencies in `Cargo.toml`
- Test across all supported platforms

## File Structure Notes

### Frontend (`src/`)
- `components/` - UI components (PascalCase .tsx files)
- `hooks/` - Custom React hooks
- `store/` - Zustand state management
- `types/` - TypeScript type definitions
- `utils/` - Utility functions

### Backend (`src-tauri/src/`)
- `main.rs` - App entry point
- `lib.rs` - Library configuration and module declarations
- `commands.rs` - Main API commands (file ops, thumbnails, etc.)
- `thumbnails/` - Thumbnail generation subsystem
- `fs_utils.rs` - File system utilities
- Platform-specific modules for native functionality

### Configuration Files
- `src-tauri/tauri.conf.json` - Tauri app configuration
- `package.json` - Frontend dependencies and scripts
- `src-tauri/Cargo.toml` - Rust dependencies
- `tailwind.config.js` - Tailwind CSS configuration

## Coding Conventions

### TypeScript/React
- Strict TypeScript mode enabled
- 2-space indentation
- PascalCase for components, camelCase for functions/variables
- Functional components with hooks
- Use `@/` path alias for imports when available

### Rust
- Follow Rust naming conventions (snake_case)
- Use `#[tauri::command]` for exposed functions
- Handle errors with proper Result types
- Use async/await for I/O operations

### Commit Messages
- Use prefixes: `feat:`, `fix:`, `refactor:`, `docs:`, `chore:`
- Keep first line under 50 characters
- Reference issues when applicable