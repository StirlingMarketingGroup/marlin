# Architecture

Marlin is one Tauri 2 desktop app: a Rust backend that owns all file access, and a React frontend that draws the UI in a webview. The frontend never touches the disk. It calls a Rust command and gets data back.

**Covers:** `src-tauri/src/lib.rs`, `src-tauri/src/commands.rs`, `src-tauri/Cargo.toml`, `src-tauri/crates/`, `src-tauri/src/fs_watcher.rs`, `src/main.tsx`, `src/store/`, `src/windows/`, `src/components/PathBar.tsx`, `src/hooks/useDirectoryWatcher.ts`
**Checked against the code:** 2026-11-13

## Processes

| Process        | Built from                                             | Job                                                                                                      |
| -------------- | ------------------------------------------------------ | -------------------------------------------------------------------------------------------------------- |
| `marlin`       | `src-tauri/src/main.rs` → `app_lib::run()` in `lib.rs` | The app. Hosts the webview, runs every command.                                                          |
| `marlin-smb`   | `src-tauri/crates/marlin-smb/`, its own crate          | SMB helper. Keeps `libsmbclient` out of the main app so the app starts even when samba is not installed. |
| `marlin-mount` | `src-tauri/crates/marlin-mount/`, its own crate        | Mount helper. Will own every native mount. The app starts it on demand, and it cannot mount yet.         |

The main app starts `marlin-smb` on demand and talks to it over stdin and stdout. See [Location providers](location-providers.md). `marlin-mount` is controlled over a local socket instead, so that it can outlive the app. The app starts it detached on the first mount command and asks it to stop when the user quits. See [Mount helper](mount-helper.md).

`src-tauri/Cargo.toml` is both the app package and the root of a Cargo workspace. Each helper is a member, so it shares the lockfile and the `target/` folder but none of the app's code. A plain `cargo build`, `check`, or `test` in `src-tauri/` acts on the app only. Name a helper to build or test it: `cargo test -p marlin-smb` (it needs samba installed) or `cargo test -p marlin-mount`. New binaries and shared crates go under `src-tauri/crates/`. A helper reaches the app as a file in `src-tauri/binaries/`: see [Mount helper](mount-helper.md#building-and-bundling).

The workspace has four library-only crates under `src-tauri/crates/`. `marlin-vfs` is the core for native mounts. `marlin-gdrive` is the Google Drive provider for mounts, and nothing links it yet (#529). `marlin-fuse` is the Linux mount adapter. `marlin-mount-client` holds the helper's wire types and the client that speaks them. `marlin-mount` links `marlin-vfs` and `marlin-mount-client` everywhere and `marlin-fuse` on Linux. The app links `marlin-mount-client` and none of the others. See [Mount core](mount-core.md), [Google Drive mount provider](mount-gdrive.md), [Linux mount adapter](mount-linux.md), and [Mount helper](mount-helper.md). `npm run check` checks all five along with the app.

## How the frontend and backend talk

- **Commands.** The frontend calls `invoke('command_name', args)`. The backend registers about 130 commands in the `generate_handler!` list in `lib.rs`. Almost all of them live in `commands.rs`, which is 8,600 lines and the first place to look for any backend behavior.
- **Events.** The backend pushes data with `emit`. Two matter most: `directory-batch` streams a folder listing in chunks, and `directory-changed` reports a change on disk from the file watcher.
- **Types.** Shared shapes are hand-written twice: Rust structs with `serde(rename_all = "camelCase")`, and TypeScript types in `src/types/index.ts`. Nothing generates one from the other, so change both.

## Backend modules (`src-tauri/src/`)

| Module                   | Job                                                                                                       |
| ------------------------ | --------------------------------------------------------------------------------------------------------- |
| `lib.rs`                 | Entry point. Registers plugins, commands, and shared state.                                               |
| `commands.rs`            | Nearly every command: listing, file operations, paste, archives, windows, preferences, remote setup.      |
| `locations/`             | Storage backends behind one trait. See [Location providers](location-providers.md).                       |
| `thumbnails/`            | Thumbnail service, cache, and per-format generators. See [Thumbnails](thumbnails.md).                     |
| `clipboard.rs`           | File copy, cut, and paste through the OS clipboard, with one implementation per OS.                       |
| `fs_utils.rs`            | Local path expansion, `FileItem` building, local copy helpers.                                            |
| `fs_watcher.rs`          | Watches local folders and emits `directory-changed`.                                                      |
| `mount_helper.rs`        | The five mount commands, and the request to stop the helper at quit. See [Mount helper](mount-helper.md). |
| `state.rs`               | Shared state types: directory streams, folder-size jobs, menu state, trash undo.                          |
| `menu.rs`                | The native menu bar.                                                                                      |
| `native_drag.rs`         | macOS only. Starts a native drag of files out of the app.                                                 |
| `plugins/drag_detector/` | Detects files dragged into the window, with one file per OS.                                              |
| `macos_security.rs`      | macOS security-scoped bookmarks, so folder access survives a restart.                                     |
| `macos_icons.rs`         | macOS only. Extracts app icons.                                                                           |

## Frontend (`src/`)

**Windows.** `src/main.tsx` reads the `view` query parameter and renders one root component. No parameter means the main browser window (`App.tsx`). Each other value is a small separate OS window from `src/windows/`: `folder-size`, `archive-progress`, `compress-progress`, `delete-progress`, `clipboard-progress`, `smb-connect`, `sftp-connect`, `permissions`, `preferences`, `conflict`. The backend opens these windows from commands in `commands.rs`.

**Main window.** `App.tsx` lays out `Sidebar`, `PathBar`, `MainPanel`, and `StatusBar`. `MainPanel` shows either `FileGrid` or `FileList`. Both virtualize rows with `@tanstack/react-virtual`.

**Path bar.** `src/components/PathBar.tsx` is a text field that holds a draft of the path. Enter sends the draft to `navigateTo` and gives the focus to the files. Cmd/Ctrl+L or the first click focuses the field and selects the whole path; a later click places the caret. Clicking away drops the draft; switching to another app keeps it.

- Suggestions complete only the end of the text. An edit with the caret anywhere else closes the list, so a suggestion can never replace text after the caret.
- Only Tab, a click on a suggestion, or Enter changes the text. Hover and the arrow keys only move the highlight. Tab never leaves the field: with no suggestion it does nothing.
- The list is derived in render from two things: the query (the text split at its last separator) and the entries of that folder. The current folder's entries are the store's `files`. Any other folder takes one `read_directory` call, which later edits in the same folder reuse.
- Typed text becomes a location in `resolveTypedPath` (`src/utils/pathUtils.ts`): a leading `~` is the home folder, and a Windows share path (`\\server\share\folder`) becomes `smb://server/share/folder`, except on Windows. `navigateTo` calls it for Enter, and the path bar calls it to list a folder for suggestions, so both read the text the same way. `normalizePath` does not do this, because it also handles paths that providers return. `navigateTo` also resolves a pasted Google Drive URL.
- The SMB connect window (`src/windows/SmbConnectWindow.tsx`) reads its Server field with `smbServerAndFolders`, not `resolveTypedPath`, because the field also takes a bare host name. A pasted `\\server\share\folder` is saved under `server` alone, and the share and folder become the folder that opens.

**State.** Zustand stores in `src/store/`:

| Store                                     | Holds                                                                                                                 |
| ----------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `useAppStore`                             | Almost everything: current path, history, file list, selection, preferences, clipboard, remote accounts. 3,000 lines. |
| `useUndoStore`                            | The undo stack for file operations.                                                                                   |
| `useDragStore`                            | The drag in progress.                                                                                                 |
| `useConflictStore`                        | The name-conflict question the backend is waiting on.                                                                 |
| `useToastStore`                           | Toast messages.                                                                                                       |
| `use*ProgressStore`, `useFolderSizeStore` | State for one progress window each.                                                                                   |

**Hooks worth knowing.** `useDirectoryStream` consumes `directory-batch`. `useDirectoryWatcher` reacts to `directory-changed`. `useThumbnail` requests thumbnails. `useDragDetector` handles files dropped onto the window.

**How a change on disk reaches the list.** `fs_watcher.rs` collects watcher events for 300 ms and emits one `directory-changed`. `useDirectoryWatcher` treats that event only as a signal. It waits for 400 ms with no new event, reads the whole folder again with `read_directory`, and replaces the list. When events keep coming, it reads anyway 2 seconds after the first event it has not acted on. A folder that is still loading, or a read that is still running, puts the next read off until it is done. The 2-second cap matters on Linux: inotify reports every write, so a file that is still downloading sends an event about every 300 ms. macOS reports a file written through one open handle only when it is created and when it is closed or renamed.

## Where settings live

| Data                                  | Place                                                                                                            |
| ------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| Preferences, per-folder view settings | `<config dir>/Marlin/preferences.json`, through `read_preferences` and `write_preferences`                       |
| SMB servers                           | `<config dir>/marlin/smb-servers.json`, passwords in the OS keychain                                             |
| SFTP servers                          | `<config dir>/marlin/sftp-servers.json`, passwords in the OS keychain, trusted host keys in `~/.ssh/known_hosts` |
| Google accounts                       | `<config dir>/marlin/gdrive-accounts.json`, refresh tokens in the OS keychain                                    |
| Thumbnails                            | `<cache dir>/marlin_thumbnails`                                                                                  |
| User themes                           | The themes folder listed in `README.md`                                                                          |

## Known rough spots

- Preferences use the folder `Marlin` and everything else uses `marlin`. That is one folder on macOS and two on Linux.
- `commands.rs` and `useAppStore.ts` are each one very large file that mixes unrelated concerns.
- Code decides what to do by matching on the URI scheme string (`gdrive://`, `smb://`, ...) in many files on both sides. See [Location providers](location-providers.md) for why, and epic #368 for the plan to remove it.
