# Mount core (`marlin-vfs`)

`marlin-vfs` is the layer between a storage provider and an OS mount adapter. An adapter asks it for nodes and bytes. It asks a provider for items. The [mount helper](mount-helper.md) builds one `Vfs` per mount. On Linux the [Linux mount adapter](mount-linux.md) serves it to the OS. The macOS and Windows adapters (#373, #375) and the first real provider (#372) come next. It is read-only until #376.

**Covers:** `src-tauri/crates/marlin-vfs/`
**Checked against the code:** 2026-11-13

The design is in [decision 0001](decisions/0001-mount-architecture.md). [Decision 0002](decisions/0002-node-ids-follow-lookup-counts.md) sets how long a node ID lasts.

## The parts

| File          | Holds                                                                                               |
| ------------- | --------------------------------------------------------------------------------------------------- |
| `provider.rs` | `Provider`, the trait a storage backend implements, keyed by the backend's own `ItemId`.            |
| `vfs.rs`      | `Vfs`, what an adapter calls. It owns the node table, the open handles, and the listing cache.      |
| `names.rs`    | The rules that make the names in one folder unique and safe for the OS.                             |
| `error.rs`    | `VfsError`, the one error type. Each adapter maps it to an `errno` or an `NTSTATUS`.                |
| `memory.rs`   | `MemoryProvider`, a provider that lives in memory. Tests here and in later crates run on top of it. |

The crate has no FUSE, WinFsp, Tauri, or serde types. It depends on `tokio` and `async-trait` only.

## What a provider implements

Five calls: `root`, `item`, `list`, `read` (a byte range), and `space`. A provider returns the names the remote uses and does not make them unique or safe. It has no write calls yet.

An `ItemId` should stay with an item when the remote renames or moves it. Google Drive's file ID does. A provider that only has paths, such as SFTP, uses the path, and a rename there looks like one item removed and another added.

## What an adapter calls

| Call                                    | What it does                                                                                               |
| --------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| `lookup(parent, name)`                  | Finds a name in a folder and returns its node. Counts one lookup on that node.                             |
| `forget(node, lookups)`                 | Gives lookups back. The adapter must call it for every lookup, or the node table grows.                    |
| `attributes(node)`                      | Asks the provider for the kind, size, and modified time. Not cached.                                       |
| `open_folder`, `read_folder`, `release` | Takes a fixed copy of a folder's listing and reads it in pages. The copy does not change while it is open. |
| `open`, `read`, `release`               | Opens a file and reads any byte range. Opening for write fails with `ReadOnly`.                            |
| `space()`                               | Total and free bytes.                                                                                      |
| `refresh()`                             | Drops the cached listings.                                                                                 |

## Rules the core keeps

- **Nodes.** The root is node 1. A node lives while the OS holds a lookup on it or a handle is open on it. An open folder holds its own node and the node of every item in its listing. Then the core drops it. An ID is never reused in a mount. A node that is gone gives `StaleHandle`.
- **Stable identity.** A node is tied to the provider's `ItemId`, not to a path. An item the remote renamed or moved keeps its node and its open handles.
- **Names.** `escape` turns each character the OS forbids into `%XX`, and `unescape` reverses it exactly. A name that occurs once in a folder is kept. Items that share a name are taken in `ItemId` order: each keeps the name if it is still free, or gets the first free ` (2)`, ` (3)` before the extension. The mapping is worked out again for each listing and is not stored, so a suffix can change when a duplicate is added or removed.
- **Name rules per OS.** `NameRules` holds two functions: which characters are forbidden, and the key that decides whether two names are the same name. `POSIX` is the only rule set today. Windows reserved names and macOS Unicode forms belong to their adapters (#375, #373).
- **Limits.** Each mount runs at most `max_calls` provider calls at once (default 16). A call that does not finish within `call_timeout` (default 30 seconds), waiting included, fails with `TimedOut`. Dropping the future of a core call cancels its provider call.
- **Listing cache.** A folder's listing is reused for `listing_ttl` (default 5 seconds). The cache holds at most `max_listings` folders (default 64), and 0 turns it off. A listing that started before a `refresh()` is never put in the cache.
- **Locking.** All state is behind one mutex that is never held across a provider call.
- **Call results.** `Vfs::watch_calls` gives a mount's owner the result of every provider call, including the core's own `TimedOut`. A cancelled call and a cached listing report nothing. The mount helper builds a mount's health on it ([Health](mount-helper.md#health)).

The four defaults are first guesses. Issue #372 tunes them against the real Google Drive API.

## Testing

`cd src-tauri && cargo test -p marlin-vfs` runs the name tests and the conformance tests in `tests/conformance.rs`. They need no kernel driver, no network, and no account. Tests that depend on time run on tokio's paused clock, so none of them sleeps.
