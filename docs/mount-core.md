# Mount core (`marlin-vfs`)

`marlin-vfs` is the layer between a storage provider and an OS mount adapter. An adapter asks it for nodes and bytes. It asks a provider for items. The [mount helper](mount-helper.md) builds one `Vfs` per mount. The [Linux mount adapter](mount-linux.md) and the [macOS mount adapter](mount-macos.md) serve it to the OS. The first real provider is the [Google Drive mount provider](mount-gdrive.md). The Windows adapter (#375) comes next. The core can make a folder, remove an item, and rename or move an item. It cannot write a file's bytes yet (#595), and no adapter calls the three changes yet (#598, #599).

**Covers:** `src-tauri/crates/marlin-vfs/`
**Checked against the code:** 2026-11-18

The design is in [decision 0001](decisions/0001-mount-architecture.md). [Decision 0002](decisions/0002-node-ids-follow-lookup-counts.md) sets how long a node ID lasts. [Decision 0005](decisions/0005-mount-writes.md) sets how a mount takes writes.

## The parts

| File          | Holds                                                                                                       |
| ------------- | ----------------------------------------------------------------------------------------------------------- |
| `provider.rs` | `Provider`, the trait a storage backend implements, keyed by the backend's own `ItemId`.                    |
| `vfs.rs`      | `Vfs`, what an adapter calls. It owns the node table, the open handles, and the provider answers it reuses. |
| `names.rs`    | The rules that make the names in one folder unique and safe for the OS.                                     |
| `error.rs`    | `VfsError`, the one error type. Each adapter maps it to an `errno` or an `NTSTATUS`.                        |
| `memory.rs`   | `MemoryProvider`, a provider that lives in memory. Tests here and in later crates run on top of it.         |

The crate has no FUSE, WinFsp, Tauri, or serde types. It depends on `tokio` and `async-trait` only.

## What a provider implements

Five read calls: `root`, `item`, `list`, `read` (a byte range), and `space`. A provider returns the names the remote uses and does not make them unique or safe.

Three change calls, each optional: `create_folder`, `remove`, and `rename`. A provider that leaves one out answers `ReadOnly` for it. The Google Drive provider leaves all three out until #600.

- `remove` and `rename` get the folder the core believes the item is in. The core's listing can be a few seconds old, so a provider that can tell answers `NotFound` when the item is no longer there.
- `remove` refuses a folder that holds anything (`NotEmpty`). A remote that has a trash moves the item there.
- `rename` moves and renames in one call. It may get an item to replace. The provider swaps the two in one step when the remote can. When it cannot, it moves first and removes second, so a call that fails halfway never leaves the name with no item. A remote that has a trash moves the replaced item there.
- `rename` with nothing to replace never removes or overwrites another item. A remote that keeps one name per folder answers `Conflict` when the name is taken.
- `rename` returns the item as the remote now has it. The id may have changed.

There is no call that writes a file's bytes yet (#596).

When a file's bytes change, its modified time or its size must change too. An adapter may keep bytes it already read until one of the two changes, as the [Linux mount adapter](mount-linux.md) does.

The text of a `VfsError::Other` holds no secret and no file name. The mount helper logs it.

An `ItemId` should stay with an item when the remote renames or moves it. Google Drive's file ID does. A provider that only has paths, such as SFTP, uses the path, and a rename that another client makes there looks like one item removed and another added. For this mount's own rename, see "A rename that changes the item's ID" below.

## What an adapter calls

| Call                                                    | What it does                                                                                                                                                                                           |
| ------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `lookup(parent, name)`                                  | Finds a name in a folder and returns its node. Counts one lookup on that node.                                                                                                                         |
| `forget(node, lookups)`                                 | Gives lookups back. The adapter must call it for every lookup, or the node table grows.                                                                                                                |
| `attributes(node)`                                      | Returns the kind, size, and modified time. Asks the provider only when the node holds no fresh answer.                                                                                                 |
| `open_folder`, `read_folder`, `release`                 | Takes a fixed copy of a folder's listing and reads it in pages. The copy does not change while it is open.                                                                                             |
| `open`, `read`, `release`                               | Opens a file and reads any byte range. Opening for write fails with `ReadOnly`.                                                                                                                        |
| `create_folder(parent, name)`                           | Makes a folder and returns its node. Counts one lookup on that node. A name that is taken is `Conflict`.                                                                                               |
| `remove(parent, name, kind)`                            | Removes a file (`unlink`) or an empty folder (`rmdir`). The wrong kind is `IsAFolder` or `NotAFolder`.                                                                                                 |
| `rename(parent, name, new_parent, new_name, if_exists)` | Renames or moves. When the new name is taken, `IfExists::Replace` replaces that item and `IfExists::Refuse` fails with `Conflict`. A folder never replaces a file, and a file never replaces a folder. |
| `space()`                                               | Total and free bytes. Asks the provider only when the core holds no fresh answer.                                                                                                                      |
| `refresh()`                                             | Drops the cached listings and makes every stored attributes and space answer stale.                                                                                                                    |

## Rules the core keeps

- **Nodes.** The root is node 1. A node lives while the OS holds a lookup on it or a handle is open on it. An open folder holds its own node and the node of every item in its listing. Then the core drops it. An ID is never reused in a mount. A node that is gone gives `StaleHandle`.
- **Stable identity.** A node is tied to the provider's `ItemId`, not to a path. An item the remote renamed or moved keeps its node and its open handles.
- **Names.** `escape` turns each character the OS forbids into `%XX`, and `unescape` reverses it exactly. A name that occurs once in a folder is kept. Items that share a name are taken in `ItemId` order: each keeps the name if it is still free, or gets the first free ` (2)`, ` (3)` before the extension. The mapping is worked out again for each listing and is not stored, so a suffix can change when a duplicate is added or removed.
- **Name rules per OS.** `NameRules` holds two functions: which characters are forbidden, and the key that decides whether two names are the same name. `POSIX` is the only rule set today. Windows reserved names come with the Windows adapter (#375). macOS mounts use `POSIX` until they get rules of their own (#542).
- **Limits.** Each mount runs at most `max_calls` provider calls at once (default 16). A call that does not finish within `call_timeout` (default 30 seconds), waiting included, fails with `TimedOut`. Dropping the future of a core call cancels its provider call.
- **Listing cache.** A folder's listing is reused for `listing_ttl` (default 5 seconds). The cache holds at most `max_listings` folders (default 64), and 0 turns it off. A listing that started before a `refresh()` is never put in the cache.
- **Attributes.** A node keeps the provider's latest answer about its item. The answer comes from a listing or from an `item` call. `attributes`, and the folder check in `open`, reuse it for `attributes_ttl` (default 5 seconds), and 0 turns that off. So a listing and then the attributes of each item in it cost one provider call. The answer goes away with its node, so the node table is the bound and there is no size option. `lookup` and `open_folder` return the latest answer the node holds. A stored size never limits a `read`. An `open` in that time does not notice that the remote removed the file.
- **Free space.** The core reuses the provider's answer for `space_ttl` (default 30 seconds), and 0 turns that off. A failed call is not kept. macOS asks for the free space at every open of a file ([macOS mount adapter](mount-macos.md#how-a-mount-behaves)).
- **Age and order.** A listing, an attributes answer, and a space answer are each fresh for their time limit from when the answer came. So a listing that took longer than `listing_ttl` is still reused. The core orders answers by when it asked: one asked for earlier never replaces one asked for later, however late it comes.
- **Changes run one at a time.** `create_folder`, `remove`, and `rename` share one lock per mount, held from the first name lookup to the end of the change. So two calls that make the same name cannot both succeed. A change that waits longer than `call_timeout` for the lock fails with `TimedOut`. Reads and listings never take the lock.
- **The core shows its own changes at once.** After the provider confirms a change, the core edits the listing it holds for each folder the change named, by item ID. It does not ask the provider again. A listing the core asked for before the change ended, and that answers after it, is not put in the cache.
- **A change with no clear answer drops the listing.** When a change call fails, runs out of time, or is cancelled, the remote may or may not have changed. The core drops the cached listing of each folder the call named, and the next lookup asks the provider.
- **Names a mount can make.** The provider gets `unescape` of the name the OS sent. A name that would not come back the same from `escape`, such as `%41` or `My%20File.pdf`, is refused with `NotSupported` before any provider call (#594).
- **A rename that changes the item's ID.** The core moves the item's node to the new ID, and open handles follow. The nodes of items inside a moved folder are not moved: with a provider whose IDs are paths they go stale (#603).
- **A rename onto a name that the remote holds twice.** Google Drive allows two items with one name in a folder. After a rename replaces one of them, the mount can show the moved item with a ` (2)` suffix and the other duplicate under the plain name (#594).
- **A removed item's node** stays until the OS forgets it. Calls on it get whatever the provider answers for that ID.
- **Locking.** All state is behind one mutex that is never held across a provider call. The lock that makes changes run one at a time is a second one, and it is held across a change's provider call.
- **Call results.** `Vfs::watch_calls` gives a mount's owner the result of every provider call, including the core's own `TimedOut`. A cancelled call, a cached listing, attributes a node already holds, and a space answer the core already holds report nothing. The mount helper builds a mount's health on it ([Health](mount-helper.md#health)).

Every mount uses the six defaults. [Google's limits](mount-gdrive.md#googles-limits) says how they fit the Drive API. Nobody has measured them against a real account.

## Testing

`cd src-tauri && cargo test -p marlin-vfs` runs the name tests and the conformance tests in `tests/conformance.rs`. They need no kernel driver, no network, and no account. Tests that depend on time run on tokio's paused clock, so none of them sleeps. `MemoryProvider` implements the three change calls. Its `remote_rename` and `remote_remove` change the tree behind the mount's back, as another user of the remote would.
