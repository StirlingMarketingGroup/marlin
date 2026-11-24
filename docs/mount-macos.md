# macOS mount adapter (`marlin-fuse`)

`marlin-fuse` serves one [mount core](mount-core.md) `Vfs` to macOS through macFUSE. The [mount helper](mount-helper.md) uses it on macOS. The same crate serves Linux: see [Linux mount adapter](mount-linux.md). A mount takes writes when its core can write: see [Writes](#writes). [Decision 0005](decisions/0005-mount-writes.md) is the design.

The helper mounts a Google account through it: see [Providers](mount-helper.md#providers). No part of the UI asks for a mount yet (#377).

**Covers:** `src-tauri/crates/marlin-fuse/src/lib.rs`, `src-tauri/crates/marlin-fuse/src/macos.rs`, `src-tauri/crates/marlin-fuse/src/macos/`, `src-tauri/crates/marlin-fuse/test-on-mac.sh`
**Checked against the code:** 2026-11-23

The design is in [decision 0001](decisions/0001-mount-architecture.md), choices 2 and 6.

## The parts

| File               | Holds                                                                                                                                                                                    |
| ------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/lib.rs`       | What both systems share: the errno table, the limits, the made-up permissions, the name rule, the open access rule, the `setattr` order, the names that stay local, and `is_dead_mount`. |
| `src/macos.rs`     | `mount`, `Mount::unmount`, `unmount_dead`, `Error`, the session thread, and the macOS version check.                                                                                     |
| `src/macos/sys.rs` | The libfuse structs and the functions Marlin calls, loaded when the first mount is made.                                                                                                 |
| `src/macos/fs.rs`  | The libfuse callbacks and the attribute rules.                                                                                                                                           |
| `tests/mount.rs`   | Tests that mount for real, shared with Linux. They are marked `#[ignore]`, and `test-on-mac.sh` runs them.                                                                               |
| `test-on-mac.sh`   | Runs every test of `marlin-fuse` and `marlin-mount` on this Mac, then [`test-sweep.sh`](mount-helper.md#dead-mounts).                                                                    |

- The macOS code is behind `cfg(target_os = "macos")`, and the Linux code is behind `cfg(target_os = "linux")`. Each system compiles only its own half.
- **No link to macFUSE at build time.** The adapter opens `/usr/local/lib/libfuse3.4.dylib` with `libloading` when the first mount is made. So a build needs no macFUSE, and a Mac without macFUSE can still run Marlin. A failed load is not kept: the next mount tries again, so the user can install macFUSE while the helper runs.
- **The structs are written by hand** for the low-level API of libfuse 3.18.3 with macFUSE's "Darwin extensions" off. With them off, `struct stat` and `struct statvfs` are the system's own. A unit test in `sys.rs` holds every struct's size and field offsets. The numbers are those of the macFUSE 5.5.0 headers with the extensions off. If a macFUSE release changes a struct, measure it with a C program and fix both.
- The adapter always passes `backend=fskit`. macFUSE's other backend is a kernel extension, and macOS makes the user change security settings in Recovery Mode before it loads one.

## What the machine needs

| Need                                                | When it is missing, the mount fails with                                    |
| --------------------------------------------------- | --------------------------------------------------------------------------- |
| macOS 15.4 or later                                 | "Marlin needs macOS 15.4 or later to mount."                                |
| macFUSE 5 or later                                  | Where to get macFUSE, and which two switches to turn on in System Settings. |
| Both macFUSE switches on, in File System Extensions | "macFUSE refused the mount.", and where the two switches are.               |

The helper sends this text to the app as error -1006.

- The third text is a best guess at the reason. macFUSE's mount program writes the real reason to its own stderr, which the adapter cannot read. The adapter gives this text for every mount that ends before its volume appears.
- Not tested: a Mac with the switches off, and a Mac without macFUSE (#543).
- **A signed helper needs `com.apple.security.cs.disable-library-validation`.** macFUSE's library is signed by another team, and the hardened runtime refuses to load it without that entitlement. The mount then fails with the "needs macFUSE" text, though macFUSE is installed. See [Building and bundling](mount-helper.md#building-and-bundling).

## How a mount behaves

- **Mount options:** `backend=fskit`, file system name `marlin`, and a volume name that is the mount folder's name. `ro` comes first when `Vfs::writable` is false. A writable mount passes neither `ro` nor `rw`. The mount table shows `macfuse://<id> on <path> (macfuse, nodev, nosuid, read-only, noowners, noatime, fskit, mounted by <user>)`, without `read-only` for a writable mount. Neither name is in that line. Not tested: where Finder shows the volume name (#543).
- **Only the mounting user can open it.** The adapter never passes `allow_other`.
- **Made-up permissions.** On a writable mount a folder has mode 0700 and a file mode 0600. On a read-only mount they are 0500 and 0400. A file that the provider marks read-only shows 0400 on a writable mount too. Opening it for writing, and renaming a new file over it, fail with `EACCES` ([decision 0008](decisions/0008-read-only-files.md)). Access, change, and creation times repeat the modified time. The 1-second cache of names and attributes, node IDs, listings, and the 255-byte name limit are the same as on Linux: see [How a mount behaves](mount-linux.md#how-a-mount-behaves). macOS does not check these modes on this mount, and macFUSE's FSKit backend ignores `default_permissions` (macFUSE 5.5.0). The adapter has no `access` callback: macOS asks `access` for `W_OK` before it renames or removes a file too, so an answer from the mode would stop a rename or a trash of a read-only file. So an app that asks before it saves, as Cocoa's `isWritableFileAtPath` does, hears yes for a read-only file, and finds out when its open fails with `EACCES`. On a read-only mount macOS refuses every write with `EROFS` before it reaches the adapter.
- **A mount takes one to two seconds.** macFUSE starts the real mount when the session loop starts, and the volume appears a moment later. `mount` returns when the mount table lists the path.
- **Reads answer through `fuse_reply_data`.** With `fuse_reply_buf`, an app that reads 64 KiB or more in one call gets "Input/output error" (macFUSE 5.5.0, macOS 27.2). libfuse reports success for that reply, and macFUSE's extension then fails the read. The test that reads a 300,000-byte file whole guards this.
- **File bytes.** The adapter does not ask macOS to keep a file's bytes, as the Linux adapter does. macOS keeps them anyway: in the test with a fake Google, a second read of a whole file makes no call to Google. Not tested: when macOS drops the bytes after the file changes on the remote (#543).
- **Free space.** `statfs` reports the provider's total and free bytes in 4,096-byte blocks. macOS counts blocks in 32 bits, so an account over 16 TiB, or one with no limit, shows 16 TiB. macOS asks when it mounts and answers a program's later `statfs` calls from that. In an 8-second test no such call reached the adapter. Not tested: when one does (#543). macOS itself asks the adapter again at every open of a file: in a test, 100 opens made over 100 requests. The core reuses the provider's answer for 30 seconds, so they cost at most one provider call in that time.
- **Names.** A name is matched by its exact bytes, as on Linux. A file with a composed accent in its name (`é` as U+00E9) is listed and opens by that name. It does not open by the decomposed form of the name. macOS name rules are #542.
- **A listing from offset 0 is read again.** macFUSE opens a folder once, keeps the handle, and serves every later listing from any program by reading that handle from offset 0. `opendir` takes no listing. Each `readdir` at offset 0 calls `Vfs::rewind_folder` in a task, which takes the folder's listing then, so every listing shows the folder as the core sees it at that moment.
- **macOS sends forget calls.** They go straight to the core ([decision 0002](decisions/0002-node-ids-follow-lookup-counts.md)).
- **A slow call blocks nothing else.** One OS thread per mount runs libfuse's loop. Each request that may reach the provider becomes a tokio task, and the thread goes back for the next request at once. `forget`, `opendir`, `readdir` past offset 0, and the two release calls answer in place. They touch the core's own state, and the last writer's release also writes the file's small spool record, with no sync.

## Writes

The callbacks are the Linux adapter's, through libfuse's low-level API: see [Writes](mount-linux.md#writes). Each calls the same core function. What differs on macOS:

- `create` arrives with `O_RDWR`, `O_CREAT`, and `O_EXCL`. macFUSE then sets the owner with `setattr`, and releases that handle at once. The bytes come later, through a new `open`.
- `setattr` can arrive with nothing set, or with only the owner. It answers the attributes. It reads the size and the modified time, and nothing else.
- An overwrite (`>`) arrives as `open` with `O_WRONLY`, then `setattr` with size 0. macFUSE does not offer `FUSE_CAP_ATOMIC_O_TRUNC`. The core cuts a clean file to zero without reading it.
- **macOS keeps written bytes in its own cache first.** A small `dd` into the middle of a file reaches the adapter as one write of the whole file from offset 0. The writes reach the adapter before the app's `close` or `fsync` returns.
- **`fsync` never reaches the adapter.** macFUSE 5.5.0 writes the cached bytes, then answers the app's `fsync` itself. So after an app's `fsync` the bytes are in the spool, but the spool file may not be on the disk yet ([decision 0005](decisions/0005-mount-writes.md), choice 5, holds only on Linux). The adapter serves `fsync` for a macFUSE that sends it.
- **Rename flags.** macFUSE sends `RENAME_EXCL` (`0x4`) for a rename onto a free name, and no flag for a rename over an item. `RENAME_SWAP` fails with `ENOTSUP`. macOS never passes an app's own `renamex_np` flags: it answers `EEXIST` for `RENAME_EXCL` onto a taken name, and `ENOTSUP` for `RENAME_EXCL` onto a free name and for `RENAME_SWAP`. A folder moved into itself fails with `EINVAL` before it reaches the adapter.
- `link` and `symlink` answer `ENOTSUP`, which `link(2)` documents for a filesystem without links. Left out, macOS would pass on libfuse's `ENOSYS`, "Function not implemented".
- `flush`, `mknod`, `fsyncdir`, `fallocate`, `access`, `readlink`, and the extended attribute calls are left out. Closes succeed without `flush`. macFUSE uses `create`, not `mknod`. `cp` sends `fallocate`, gets `ENOSYS`, and writes anyway.

## Finder's files

macFUSE keeps every extended attribute in a `._name` file beside the item, because the adapter has no extended attribute calls. It writes one for every new file and folder: macOS gives each new file a `com.apple.provenance` attribute. Answering `ENOTSUP` to the extended attribute calls does not change this: macFUSE asks, then writes the `._name` file anyway. Finder writes `.DS_Store` files too.

- Both stay on this computer ([decision 0005](decisions/0005-mount-writes.md), choice 14): `marlin_fuse::stays_local` is `marlin_vfs::finder_files` on macOS. The helper passes it as `Spool::stays_local`. The write tests pass it too. A test checks that a Finder copy puts no `._name` or `.DS_Store` file on the provider.
- A `._name` file stays behind when its file is renamed or removed. macFUSE does not move or remove it. It lives in the spool until the mount ends.
- Removing a folder that holds only `._name` files works. The core answers "not empty", and macOS removes the `._name` files and tries again.
- **Finder copies through placeholders.** It makes an empty file for each item first, then removes each one and writes the real file. When the copy takes longer than the settle time, a placeholder uploads empty, is removed on the remote, and the real file uploads under a new ID (#629).
- **Finder checks the free space first.** When a copy needs more than `statfs` reports free, Finder stops before it writes a byte, with "An item with the same name already exists". The empty placeholder files and the folders it made stay, and upload.
- Move to Trash on a mount removes the item at once. Finder makes no `.Trashes` folder.
- A Cocoa app's safe save (`NSDocument`, as TextEdit does) writes the new file into a folder named `(A Document Being Saved By <app>)` at the mount's root, renames it over the old file, and removes the folder. The saved file keeps its ID, because it is still only in the spool when it is renamed ([decision 0005](decisions/0005-mount-writes.md), choice 9). But the folder is made on the remote and removed there on every save: two provider calls, and on Google Drive a folder in the trash (#628).
- Foundation's atomic write writes `<name>.sb-<random>` in the same folder, then renames it over `<name>`.

## The life of a session

One thread per mount creates the libfuse session, runs its loop, and destroys it. No other code destroys a session.

- **A refused mount shows as a loop that ends.** `fuse_session_mount` reports success even for a mount that will fail, for example a mount on a file. So `mount` watches two things: the mount table, and whether the loop has ended.
- **`mount` waits 30 seconds for the volume.** Then it gives up, runs `umount -f` on the path, and returns an error. A session thread that is still inside `fuse_session_mount` then skips the loop and cleans up. One that is already in the loop stays until macOS ends the session: libfuse has no call that wakes it.
- **Requests are counted.** A callback counts its request in, and the reply counts it out. After the loop ends, the thread waits for the count to reach zero before it destroys the session, because a reply needs the session. A task that is dropped before it answers replies "Input/output error", so the count always returns to zero.
- **"Has this session ended?" is asked of the session, never of the path.** Another mount may be at the path by then.

## Errors

The table in [Linux mount adapter](mount-linux.md#errors) holds for macOS: both systems share it (`errno` in `src/lib.rs`), and the test of every row passes on macOS. The names are the same on both systems. Some numbers differ: `ENOTSUP` is 45 on macOS and 95 on Linux.

## Unmounting

- `Mount::unmount` runs `umount <path>`. This is a plain unmount: it fails with "Resource busy" while any file or folder in the mount is open, and the mount stays served.
- `unmount` returns when the session loop has ended, or 5 seconds after the unmount if the loop has not. It does not wait for a provider call that the unmount itself causes. A request macOS sent before the unmount can hold it: see below.
- **A mount removed from outside.** A user can run `umount` or eject the volume in Finder. The loop ends, and the next `unmount` succeeds without running anything. Until that `unmount`, the helper still lists the mount (#488).
- **A forced unmount waits for a slow provider.** While a request is waiting on the provider, macOS holds `umount -f` until the request is answered. The core answers a provider call after 30 seconds at most (`call_timeout`), so that is the longest wait. This holds for a user's `umount -f` and for the one a dropped `Mount` runs.
- **A request macOS sent itself can hold a plain unmount too.** macOS sends a new mount about ten requests in its first moments. While one of them waits on a slow provider, `umount` returns only when it is answered. A request that the unmount itself causes does not hold it.
- Dropping a `Mount` whose session has not ended runs `umount -f`. Dropping one whose session has ended does nothing, so a later mount at the same path is safe.
- A helper that is killed leaves a dead mount. A listing of it fails with "Device not configured". `is_dead_mount` reports that, and `unmount_dead` runs `umount -f`. See [Dead mounts](mount-helper.md#dead-mounts).

## Testing

Run `src-tauri/crates/marlin-fuse/test-on-mac.sh` on a Mac that has macFUSE 5 with both switches on. CI does not run it: the runners have no macFUSE.

- It runs the `marlin-fuse` tests with the ignored ones, the `marlin-mount` tests with the ignored ones, and `crates/marlin-mount/test-sweep.sh`. The helper's ignored tests mount a Google account against a fake Google with the in-memory keychain: see [Mount helper](mount-helper.md#testing). No test touches a real account or the real keychain.
- The mount tests are the Linux ones: see [Testing](mount-linux.md#testing). These differ:
  - `editor_saves_keep_one_file` runs `sed -i ''` and `/usr/bin/vim` instead of `sed -i` and `vim.tiny`, and leaves `._name` files out of its listing check. `names_change_through_the_mount` checks the macOS errnos: `EPERM` for `unlink` of a folder, which `unlink(2)` documents, `ENOTSUP` for a link, and the `renamex_np` answers above.
  - `a_cocoa_safe_save_keeps_the_item` is macOS only. It saves through a real `NSDocument`, from `osascript -l JavaScript`, and checks that the file keeps its ID.
  - The write tests use the system's `rsync`, `truncate`, `sed`, and `/usr/bin/vim`. macOS 27.2 has all four.
  - `finder_copies_reach_the_provider` is macOS only. It copies a file and a folder with Finder through `osascript`, as a drag does, and checks the provider holds exactly those bytes. The first run asks to let the terminal control Finder. It copies less than the 250,000 bytes `MemoryProvider` reports free.
  - The test of bytes kept across opens is Linux only. It tests a flag the macOS adapter does not set.
  - A mount on a file must fail fast. This test is macOS only: Linux lets a mount sit on a file.
- The unit tests need no macFUSE: the struct layouts, the attribute rules, the free-space counts, the version check, the three messages, and what is not a dead mount.
- **macFUSE can stop working during a run.** The script mounts about 30 times. After many mounts in a row, macOS can stop finding macFUSE's extension, and every later mount fails (macFUSE issue 1232, open). Turn both switches on again in System Settings, General, Login Items & Extensions, File System Extensions.
- A failed mount prints lines from macFUSE on stderr, such as "Failed to mount volume: mount(8) returned 69". The test of a mount on a file prints one.

## Tested, and not

| What                                                                                                                       | State                                                                      |
| -------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| macOS 27.2 on Apple silicon, macFUSE 5.5.0, mounts made by test programs                                                   | Tested by `test-on-mac.sh`, 2026-11-23                                     |
| A test program signed ad hoc with the hardened runtime and the app's entitlements                                          | Mounts, 2026-11-16. With no entitlements, macOS refuses macFUSE's library. |
| A build signed with the Developer ID certificate and notarized                                                             | Not tested (#543).                                                         |
| Writes through the FSKit backend: `cp`, `cp -p`, `rsync`, `dd`, `truncate`, `sed -i`, `vim`, rename, remove, a Finder copy | Tested by `test-on-mac.sh`, 2026-11-23                                     |
| An `NSDocument` safe save                                                                                                  | Tested by `test-on-mac.sh`, 2026-11-23                                     |
| Quick Look, file dialogs, sleep and wake, a Finder drag and a TextEdit save by hand                                        | Not tested (#543).                                                         |
| A mount started from the app, and a real Google account                                                                    | Not tested (#543).                                                         |
| The switches off, and no macFUSE                                                                                           | Not tested (#543).                                                         |
| An Intel Mac, and macOS 15.4 to 26                                                                                         | Not tested.                                                                |
| Read speed, and the calls Finder makes in a large folder                                                                   | Not measured (#543).                                                       |

Known limits:

- A file or folder whose name is longer than 255 bytes is hidden.
- The free space can be out of date, and it stops at 16 TiB.
- macFUSE and libfuse write their own plain lines to the helper's stderr, so those lines land in the helper's log between its JSON lines.
- After many mounts, macOS can stop finding macFUSE's extension until the user turns it on again (macFUSE issue 1232).
- The library path is fixed: `/usr/local/lib/libfuse3.4.dylib`, where macFUSE 5 installs it.
- The adapter does not handle libfuse's interrupt request. A call that an app gave up on still runs until the core's time limit ends it.
- After an app's `fsync`, the bytes are in the spool but may not be on the disk.
- A `._name` file stays in the spool after its file is renamed or removed, until the mount ends.
- A Finder copy bigger than the free space leaves empty placeholders, which upload.
- macOS's own `vim` saves by renaming the old file away, so the saved file gets a new ID (risk 6 of [decision 0005](decisions/0005-mount-writes.md)).
- macOS refuses an app's `renamex_np` with `RENAME_EXCL` or `RENAME_SWAP`.
- A mount says it is case-insensitive, but matches names by their exact bytes (#542).
- A Cocoa app's safe save makes and removes a folder on the remote (#628).
- A slow Finder copy of several items uploads and removes an empty placeholder for each item after the first (#629).
