# Architecture

Marlin is one Tauri 2 desktop app: a Rust backend that owns all file access, and a React frontend that draws the UI in a webview. The frontend never touches the disk. It calls a Rust command and gets data back.

**Covers:** `src-tauri/src/lib.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/menu.rs`, `src-tauri/Cargo.toml`, `src-tauri/crates/`, `src-tauri/src/fs_watcher.rs`, `src/main.tsx`, `src/App.tsx`, `src/store/`, `src/windows/`, `src/components/PathBar.tsx`, `src/components/FilterInput.tsx`, `src/hooks/useDirectoryWatcher.ts`, `src/hooks/useFileAnimations.ts`, `src/hooks/useRevealTarget.ts`, `src/components/MainPanel.tsx`, `src/components/FileGrid.tsx`, `src/components/FileList.tsx`, `src/utils/filterMatch.ts`, `src/components/FileNameDisplay.tsx`
**Checked against the code:** 2026-11-19

## Processes

| Process        | Built from                                             | Job                                                                                                      |
| -------------- | ------------------------------------------------------ | -------------------------------------------------------------------------------------------------------- |
| `marlin`       | `src-tauri/src/main.rs` → `app_lib::run()` in `lib.rs` | The app. Hosts the webview, runs every command.                                                          |
| `marlin-smb`   | `src-tauri/crates/marlin-smb/`, its own crate          | SMB helper. Keeps `libsmbclient` out of the main app so the app starts even when samba is not installed. |
| `marlin-mount` | `src-tauri/crates/marlin-mount/`, its own crate        | Mount helper. Owns every native mount. The app starts it on demand. It mounts Google on Linux and macOS. |

The main app starts `marlin-smb` on demand, through `marlin-smb-client`, and talks to it over stdin and stdout. See [Location providers](location-providers.md). `marlin-mount` is controlled over a local socket instead, so that it can outlive the app. The app starts it detached on the first mount command and asks it to stop when the user quits. See [Mount helper](mount-helper.md).

`src-tauri/Cargo.toml` is both the app package and the root of a Cargo workspace. Each helper is a member, so it shares the lockfile and the `target/` folder but none of the app's code. A plain `cargo build`, `check`, or `test` in `src-tauri/` acts on the app only. Name a helper to build or test it: `cargo test -p marlin-smb` (it needs samba installed) or `cargo test -p marlin-mount`. New binaries and shared crates go under `src-tauri/crates/`. A helper reaches the app as a file in `src-tauri/binaries/`: see [Mount helper](mount-helper.md#building-and-bundling).

The workspace has seven library-only crates under `src-tauri/crates/`. `marlin-vfs` is the core for native mounts. `marlin-gdrive` is the Google Drive provider for mounts and the saved Google sign-in. The app links it for the sign-in, and the helper links it to mount a Google account. `marlin-sftp` holds the saved SFTP servers, their host key check, and the connection. The app links it. `marlin-smb-client` holds the protocol types `marlin-smb` speaks, the client that runs one `marlin-smb` process, and the saved SMB servers. The app links it. `marlin-smb` links its protocol types only (`default-features = false`), and the crate never links Samba. `marlin-test-keychain` is an in-memory keychain that tests link, and that the app links only in its test build (the `app-test` feature, see [Testing](testing.md#real-app-tests)). `marlin-fuse` is the mount adapter for Linux and macOS. `marlin-mount-client` holds the helper's wire types and the client that speaks them. `marlin-mount` links `marlin-vfs`, `marlin-gdrive`, and `marlin-mount-client` everywhere and `marlin-fuse` on Linux and macOS. The app links `marlin-mount-client`, `marlin-gdrive`, `marlin-sftp`, and `marlin-smb-client`. See [Mount core](mount-core.md), [Google Drive mount provider](mount-gdrive.md), [Linux mount adapter](mount-linux.md), [macOS mount adapter](mount-macos.md), and [Mount helper](mount-helper.md). `npm run check` checks them and `marlin-mount` along with the app.

## How the frontend and backend talk

- **Commands.** The frontend calls `invoke('command_name', args)`. The backend registers about 130 commands in the `generate_handler!` list in `lib.rs`. Almost all of them live in `commands.rs`, which is 8,600 lines and the first place to look for any backend behavior.
- **Events.** The backend pushes data with `emit`. Two matter most: `directory-batch` streams a folder listing in chunks, and `directory-changed` reports a change on disk from the file watcher.
- **Types.** Shared shapes are hand-written twice: Rust structs with `serde(rename_all = "camelCase")`, and TypeScript types in `src/types/index.ts`. Nothing generates one from the other, so change both.

## Backend modules (`src-tauri/src/`)

| Module                   | Job                                                                                                       |
| ------------------------ | --------------------------------------------------------------------------------------------------------- |
| `lib.rs`                 | Entry point. Raises the open-file limit. Registers plugins, commands, and shared state.                   |
| `commands.rs`            | Nearly every command: listing, file operations, paste, archives, windows, preferences, remote setup.      |
| `locations/`             | Storage backends behind one trait. See [Location providers](location-providers.md).                       |
| `thumbnails/`            | Thumbnail service, cache, and per-format generators. See [Thumbnails](thumbnails.md).                     |
| `clipboard.rs`           | File copy, cut, and paste through the OS clipboard, with one implementation per OS.                       |
| `fs_utils.rs`            | Local path expansion, `FileItem` building, local copy helpers.                                            |
| `delete_entry.rs`        | Deletes a local file, link, or folder. See [decision 0007](decisions/0007-permanent-delete.md).           |
| `fs_watcher.rs`          | Watches local folders and emits `directory-changed`.                                                      |
| `mount_helper.rs`        | The eight mount commands and the request to stop the helper at quit. See [Mount helper](mount-helper.md). |
| `state.rs`               | Shared state types: directory streams, folder-size jobs, menu state, trash undo.                          |
| `menu.rs`                | The native menu bar.                                                                                      |
| `native_drag.rs`         | macOS only. Starts a native drag of files out of the app.                                                 |
| `plugins/drag_detector/` | Detects files dragged into the window, with one file per OS.                                              |
| `macos_security.rs`      | macOS security-scoped bookmarks, so folder access survives a restart.                                     |
| `macos_icons.rs`         | macOS only. Extracts app icons.                                                                           |

## Frontend (`src/`)

**Windows.** `src/main.tsx` reads the `view` query parameter and renders one root component. No parameter means the main browser window (`App.tsx`). Each other value is a small separate OS window from `src/windows/`: `folder-size`, `archive-progress`, `compress-progress`, `delete-progress`, `clipboard-progress`, `smb-connect`, `sftp-connect`, `permissions`, `preferences`, `conflict`. The backend opens these windows from commands in `commands.rs`.

**Main window.** `App.tsx` lays out `Sidebar`, `PathBar`, `MainPanel`, and `StatusBar`. `MainPanel` shows either `FileGrid` or `FileList`. Both virtualize rows with `@tanstack/react-virtual`.

**Opening items.** Two store actions in `useAppStore` decide what "open" means. `openItem` is the double-click: a folder or an archive is browsed in this window, and any other item, a macOS bundle included, opens in its app through `openFile`. `openSelected` opens the whole selection. The file context menu, File > Open, Cmd/Ctrl+O, Cmd+Down on macOS, and Enter on Windows and Linux all call it. (Return on macOS renames.)

- One selected item goes to `openItem`.
- With several items, each folder and archive opens in a new window (`new_window`), and the other items open in their apps.
- On macOS, two or more files go to the system in one `open` call (`open_paths`), as Finder does. Files for the same app then arrive together: two images share one Preview window. Windows and Linux have no such call, so there each file opens by itself through `openFile`.
- More than 10 items asks first.
- Enter belongs to a focused button or link, not to the selection. The arrow keys blur a clicked button, so Enter reaches the files again.
- A new window gets its folder as `index.html?path=`, encoded once by `new_window`. `App.tsx` reads it with `URLSearchParams`, which decodes it. Do not decode it again: an archive URI holds an encoded part of its own.

**Revealing an item.** `pendingRevealTarget` in `useAppStore` names an item to select and scroll to once it is listed. `navigateTo(path, { reveal })` sets it for the folder being opened. Creating, pasting, or extracting an item sets it for the current folder. `navigateTo` without `reveal`, back, and forward clear it. `useRevealTarget` acts on it once the item is listed and the listing has its sizes and dates (`isMetadataComplete`), because those can reorder the list. A row that is not rendered is reached through the virtualizer. A file path typed into the path bar uses `pendingFileSelectionRef` in `App.tsx` instead.

**Going up.** `parentPath` in `src/utils/pathUtils.ts` is the one rule for the folder above a path. `goUp` and the Up button both use it. A root has no folder above it, so the button is off there: `/`, a Windows drive, and the root of a remote such as `smb://server`. The root of an archive stands for the archive file, so the folder above it is the one that holds the file. Going up reveals the folder left, or the archive file.

**Path bar.** `src/components/PathBar.tsx` is a text field that holds a draft of the path. Enter sends the draft to `navigateTo` and gives the focus to the files. Cmd/Ctrl+L or the first click focuses the field and selects the whole path; a later click places the caret. Clicking away drops the draft; switching to another app keeps it.

- Suggestions complete only the end of the text. An edit with the caret anywhere else closes the list, so a suggestion can never replace text after the caret.
- Only Tab, a click on a suggestion, or Enter changes the text. Hover and the arrow keys only move the highlight. Tab never leaves the field: with no suggestion it does nothing.
- The list is derived in render from two things: the query (the text split at its last separator) and the entries of that folder. The current folder's entries are the store's `files`. Any other folder takes one `read_directory` call, which later edits in the same folder reuse.
- Typed text becomes a location in `resolveTypedPath` (`src/utils/pathUtils.ts`): a leading `~` is the home folder, and a Windows share path (`\\server\share\folder`) becomes `smb://server/share/folder`, except on Windows. `navigateTo` calls it for Enter, and the path bar calls it to list a folder for suggestions, so both read the text the same way. `normalizePath` does not do this, because it also handles paths that providers return. `navigateTo` also resolves a pasted Google Drive URL.
- The SMB connect window (`src/windows/SmbConnectWindow.tsx`) reads its Server field with `smbServerAndFolders`, not `resolveTypedPath`, because the field also takes a bare host name. A pasted `\\server\share\folder` is saved under `server` alone, and the share and folder become the folder that opens.

**Filter.** `src/components/FilterInput.tsx` is the box at the bottom right. Typing with the focus on the files shows the box and appends to `filterText`. Cmd/Ctrl+F shows it empty, so a name can be pasted. When the box is already open, the shortcut focuses it and selects its text. Escape or the X button clears the text and hides the box. An empty box stays open.

- A file is shown when it passes two tests. It is not hidden, unless `showHidden` is on. Its name passes `matchesFilter` in `src/utils/filterMatch.ts`. Text with no `*` is a contains test, ignoring case. Text with a `*` must match the whole name, and `*` stands for any run of characters. `?` is a plain character, and `*` has no escape, so a `*` in the text also matches a `*` in a name. The test runs on the real name, not the name as shown: on macOS `*.app` matches an app, and `*ari` does not match `Safari.app`.
- The views, the box, select all, and the suggested zip name each call `matchesFilter`. The highlight in `FileNameDisplay.tsx` marks the ranges from `filterMatchRanges`, run on the name as shown, so a shortened name that no longer matches gets no highlight.
- The box sorts with `useSortedFiles`, as the views do. So its match count is the number of rows on screen, and Down or Up from the box selects the first or last row.

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

**A watched folder on macOS has two sources.** The `notify` watcher reads FSEvents, which passes through the daemon `fseventsd`. When that daemon is at full CPU, its events reach the app seconds to minutes late. So `fs_watcher.rs` also holds a kqueue watch on the folder itself (`FolderEntriesWatch`). The kernel answers it within milliseconds when a file or folder is added, removed, or renamed there, however busy the daemon is. It says nothing when the bytes of an existing file change: only FSEvents reports that, so a changed size or date can still show late. Both sources feed the same 300 ms batch, so a change that both report in time is still one event and one read. A late FSEvents event causes a second read when it comes. The kqueue watch opens the folder with `O_EVTONLY`, so it does not stop the user from ejecting the volume. A folder with nothing changing costs nothing: neither source polls.

**What is in the file list.** `files` in `useAppStore` is the list that both views draw. A read of the folder decides what is in it. These are all the writers:

| When                                                                                                                                 | Writer                                                                                                                                                                                   |
| ------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| A local folder opens, or the app reads it again after an operation (a paste, an undo, a delete that does not fade out)               | `refreshCurrentDirectoryStreaming` empties the list. `appendStreamingBatch` adds each `directory-batch`. `applyMetadataUpdates` fills in sizes and dates.                                |
| A remote folder opens, the app reads it again after an operation, or the user clicks the refresh button in the path bar (any folder) | `refreshCurrentDirectory` replaces the list in one step.                                                                                                                                 |
| A local folder changes on disk                                                                                                       | `useDirectoryWatcher` replaces the list through `setFiles`, as described above. A remote folder has no watcher. F5 or Cmd/Ctrl+R reads the folder and calls `setFiles` too.              |
| The user deletes 10 files or fewer, with reduced motion off                                                                          | `markFilesExiting` marks them, so they fade out. `removeExitedFiles` takes them out of the list when the fade ends, or after 250 ms. It is the one writer that does not read the folder. |
| An image reports its size                                                                                                            | `updateFileDimensions` sets the width and height of one file.                                                                                                                            |

A writer that adds files goes through `uniqueFilesByPath`, so a path is in the list once. `useFileAnimations` marks a new file as entering for two animation frames, so it fades in. `e2e/file-view-width.spec.ts` checks that the view matches the folder after a burst of changes.

A folder with no saved preferences gets a smart default when its listing is complete. Grid is the default when at least half the files have thumbnails, and Downloads, Documents, Desktop and the like get a sort rule. `applySmartViewDefaults` in `src/App.tsx` applies these. The default is saved as the folder's preferences, so it is worked out once per folder.

**The file view never scrolls sideways.** `FileGrid` works out its column count from the width of the element that holds its rows. A `ResizeObserver` follows that element, because it is not rendered while the folder is empty. `FileList` gives each column a share of the row, and a cell cuts its text with an ellipsis. Two cases are still wider than the view: the grid's column count is one render behind a change of width, and a list narrower than about 160 px cannot hold the gaps between its columns. So `MainPanel` clips the view sideways (`overflow-x-clip`), which shows no scrollbar and cannot be scrolled.

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
