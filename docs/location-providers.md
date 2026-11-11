# Location providers

Every path in Marlin is a URI, and its scheme picks a storage backend called a location provider. The provider trait covers listing and whole-item operations only. It has no way to read or write file contents, so every feature that moves bytes between two backends is written by hand for each pair. Epic #368 replaces this with native mounts.

**Covers:** `src-tauri/src/locations/`, `src-tauri/src/smb_sidecar/`, and the paste, drop, and download commands in `src-tauri/src/commands.rs`
**Checked against the code:** 2026-11-11

## The model

`src-tauri/src/locations/mod.rs` defines it:

- **`LocationInput`** is what a command receives: a raw string (`/Users/me`, `smb://nas/share/x`) or an object with `scheme`, `authority`, and `path`. A string with no scheme is `file`.
- **`Location`** is the parsed form.
- **`LocationProvider`** is the trait each backend implements.
- **`REGISTRY`** maps a scheme to its provider. `resolve_location(input)` returns the provider and the parsed location.
- **`LocationCapabilities`** tells the frontend what the backend allows, so the UI can disable actions.

The trait has eight operations, and all of them take whole paths:

```rust
fn scheme(&self) -> &'static str;
fn capabilities(&self, location: &Location) -> LocationCapabilities;
async fn read_directory(&self, location: &Location) -> Result<ProviderDirectoryEntries, String>;
async fn get_file_metadata(&self, location: &Location) -> Result<FileItem, String>;
async fn create_directory(&self, location: &Location) -> Result<(), String>;
async fn delete(&self, location: &Location) -> Result<(), String>;
async fn rename(&self, from: &Location, to: &Location) -> Result<(), String>;
async fn copy(&self, from: &Location, to: &Location) -> Result<(), String>;
async fn move_item(&self, from: &Location, to: &Location) -> Result<(), String>; // defaults to rename
```

`rename`, `copy`, and `move_item` work inside one provider. There is no open, no read of a byte range, and no write. Errors are plain strings.

## The five providers

| Scheme    | Code                 | URI shape                                 | Writes | Watches for changes | How it reaches the storage                                                     |
| --------- | -------------------- | ----------------------------------------- | ------ | ------------------- | ------------------------------------------------------------------------------ |
| `file`    | `locations/file.rs`  | a plain path, or `file:///path`           | yes    | yes                 | The local filesystem, through `fs_utils.rs`.                                   |
| `archive` | `locations/archive/` | `archive:///?src=<archive>&path=<inside>` | no     | no                  | Reads ZIP, RAR, and TAR files. `src` can be another URI, nested up to 10 deep. |
| `gdrive`  | `locations/gdrive/`  | `gdrive://<account email>/<path>`         | yes    | no                  | The Google Drive REST API with OAuth.                                          |
| `sftp`    | `locations/sftp/`    | `sftp://<user>@<host>:<port>/<path>`      | yes    | no                  | `russh` and `russh-sftp`, with a connection pool per server.                   |
| `smb`     | `locations/smb/`     | `smb://<server>/<share>/<path>`           | yes    | no                  | The `marlin-smb` helper process.                                               |

Notes per provider:

- **`file`** renames in two steps when only letter case changes, so the rename works on case-insensitive disks.
- **`gdrive`** sets `requires_explicit_refresh`, because nothing tells Marlin when a remote folder changes. It stores accounts and their OAuth tokens as plain text in `gdrive-accounts.json` (issue #463).
- **`sftp`** allows one transfer at a time per server (`MAX_CONCURRENT_OPS` in `sftp/pool.rs`), because some servers fail on parallel reads. It accepts every host key without checking (issue #238). Passwords are in the OS keychain.
- **`smb`** is not compiled on Windows. On macOS and Linux the main app starts `marlin-smb` and sends it JSON-RPC 2.0 requests, one JSON object per line on stdin, with replies on stdout. A call times out after 30 seconds, a download after 5 minutes, and the app restarts a dead helper up to 3 times (`smb/client.rs`). Passwords are in the OS keychain under the service name `marlin-smb`.

## Where the model breaks

These are the reasons for epic #368. Each one is a place where code outside `locations/` has to know which backend it is talking to.

**1. Copy and move stop at the provider edge.** The `copy_file` and `move_file` commands return "not yet supported" when the two schemes differ.

**2. Paste is a hand-written table of provider pairs.** `paste_items_to_location` in `commands.rs` matches on `(source scheme, destination scheme)` and has a separate block of code for each pair. It also runs every drop, because `useDragDetector.ts` calls it.

| From ↓ / To → | `file` | `gdrive` | `smb` | `sftp` |
| ------------- | ------ | -------- | ----- | ------ |
| `file`        | yes    | yes      | yes   | yes    |
| `gdrive`      | yes    | yes      | yes   | yes    |
| `smb`         | yes    | **no**   | yes   | yes    |
| `sftp`        | yes    | yes      | yes   | yes    |

Same-scheme pairs go through the provider's own `copy`. Anything from an `archive` to another backend is not in the table. A pair that is missing returns "Pasting across providers is not supported for these locations yet". A new provider means a new row and a new column of hand-written code.

**3. Reading a remote file means downloading all of it to a temp file first.** The frontend picks a command by scheme: `download_gdrive_file`, `download_smb_file`, `download_sftp_file`, or `extract_archive_entry_to_temp`. Opening a file, copying it to the OS clipboard, and dragging it out all do this. Nothing streams.

**4. Dragging a remote file out to another app is written per provider.** `FileGrid.tsx` and `FileList.tsx` each have a branch for `gdrive`, `smb`, and `archive`. There is no branch for `sftp`.

**5. A drop is validated as a local path.** `resolve_drop_operation` expands the destination with `fs_utils::expand_path` and fails if that path does not exist on disk. It has no remote case.

**6. Thumbnails know about each provider.** `thumbnails/generators/mod.rs` has a download branch for `archive`, `gdrive`, `smb`, and `sftp`, and `thumbnails/cache.rs` builds cache identity differently for `smb` and `sftp`.

**7. Only local folders refresh on their own.** `supports_watching` is true for `file` alone.

**8. Scheme checks are spread across the frontend.** `useAppStore.ts`, `Sidebar.tsx`, `App.tsx`, `FileGrid.tsx`, `FileList.tsx`, `ContextMenu.tsx`, `StatusBar.tsx`, `MainPanel.tsx`, and `useThumbnail.ts` all test for scheme prefixes.

## Where this is going

Epic #368 turns each connected account or server into a native mount, so the OS sees an ordinary folder. Then local-to-remote and remote-to-remote copies, drag and drop, the clipboard, and other apps all work through normal filesystem calls, and the pair table goes away. The provider code for API calls and sign-in stays. The work is split into issues #369 to #378. The architecture decision from #369 will be the first record in [`decisions/`](decisions/README.md).

## Adding a provider today

1. Add a module in `src-tauri/src/locations/` that implements `LocationProvider`.
2. Register it in `REGISTRY` in `locations/mod.rs`.
3. Add its pairs to `paste_items_to_location`.
4. Add a download command and call it from the open, clipboard, and drag-out code in the frontend.
5. Add a download branch in `thumbnails/generators/mod.rs`.
6. Add the sign-in or connect UI, and the sidebar entry.

Steps 3 to 5 are the cost that the mount work removes.
