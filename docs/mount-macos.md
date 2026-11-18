# macOS mount adapter (`marlin-fuse`)

`marlin-fuse` serves one [mount core](mount-core.md) `Vfs` to macOS through macFUSE. The [mount helper](mount-helper.md) uses it on macOS. The same crate serves Linux: see [Linux mount adapter](mount-linux.md). A mount is read-only until the adapter serves writes (#599). [Decision 0005](decisions/0005-mount-writes.md) says how it will.

The helper mounts a Google account through it: see [Providers](mount-helper.md#providers). No part of the UI asks for a mount yet (#377).

**Covers:** `src-tauri/crates/marlin-fuse/src/lib.rs`, `src-tauri/crates/marlin-fuse/src/macos.rs`, `src-tauri/crates/marlin-fuse/src/macos/`, `src-tauri/crates/marlin-fuse/test-on-mac.sh`
**Checked against the code:** 2026-11-17

The design is in [decision 0001](decisions/0001-mount-architecture.md), choices 2 and 6.

## The parts

| File               | Holds                                                                                                                 |
| ------------------ | --------------------------------------------------------------------------------------------------------------------- |
| `src/lib.rs`       | What both systems share: the errno table, the limits, the made-up permissions, and `is_dead_mount`.                   |
| `src/macos.rs`     | `mount`, `Mount::unmount`, `unmount_dead`, `Error`, the session thread, and the macOS version check.                  |
| `src/macos/sys.rs` | The libfuse structs and the functions Marlin calls, loaded when the first mount is made.                              |
| `src/macos/fs.rs`  | The libfuse callbacks and the attribute rules.                                                                        |
| `tests/mount.rs`   | Tests that mount for real, shared with Linux. They are marked `#[ignore]`, and `test-on-mac.sh` runs them.            |
| `test-on-mac.sh`   | Runs every test of `marlin-fuse` and `marlin-mount` on this Mac, then [`test-sweep.sh`](mount-helper.md#dead-mounts). |

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

- **Mount options:** `ro`, `backend=fskit`, file system name `marlin`, and a volume name that is the mount folder's name. The mount table shows `macfuse://<id> on <path> (macfuse, nodev, nosuid, read-only, noowners, noatime, fskit, mounted by <user>)`. Neither name is in that line. Not tested: where Finder shows the volume name (#543).
- **Only the mounting user can open it.** The adapter never passes `allow_other`.
- **Made-up permissions, times, the 1-second cache of names and attributes, node IDs, listings, and the 255-byte name limit** are the same as on Linux: see [How a mount behaves](mount-linux.md#how-a-mount-behaves). The adapter has no access callback. A write fails because the mount is read-only.
- **A mount takes one to two seconds.** macFUSE starts the real mount when the session loop starts, and the volume appears a moment later. `mount` returns when the mount table lists the path.
- **Reads answer through `fuse_reply_data`.** With `fuse_reply_buf`, an app that reads 64 KiB or more in one call gets "Input/output error" (macFUSE 5.5.0, macOS 27.2). libfuse reports success for that reply, and macFUSE's extension then fails the read. The test that reads a 300,000-byte file whole guards this.
- **File bytes.** The adapter does not ask macOS to keep a file's bytes, as the Linux adapter does. macOS keeps them anyway: in the test with a fake Google, a second read of a whole file makes no call to Google. Not tested: when macOS drops the bytes after the file changes on the remote (#543).
- **Free space.** `statfs` reports the provider's total and free bytes in 4,096-byte blocks. macOS counts blocks in 32 bits, so an account over 16 TiB, or one with no limit, shows 16 TiB. macOS asks when it mounts and answers a program's later `statfs` calls from that. In an 8-second test no such call reached the adapter. Not tested: when one does (#543). macOS itself asks the adapter again at every open of a file: in a test, 100 opens made over 100 requests. The core reuses the provider's answer for 30 seconds, so they cost at most one provider call in that time.
- **Names.** A name is matched by its exact bytes, as on Linux. A file with a composed accent in its name (`é` as U+00E9) is listed and opens by that name. It does not open by the decomposed form of the name. macOS name rules are #542.
- **macOS sends forget calls.** They go straight to the core ([decision 0002](decisions/0002-node-ids-follow-lookup-counts.md)).
- **A slow call blocks nothing else.** One OS thread per mount runs libfuse's loop. Each request that may reach the provider becomes a tokio task, and the thread goes back for the next request at once. `forget`, `readdir`, and the two release calls only touch the core's own state, so they answer in place.

## The life of a session

One thread per mount creates the libfuse session, runs its loop, and destroys it. No other code destroys a session.

- **A refused mount shows as a loop that ends.** `fuse_session_mount` reports success even for a mount that will fail, for example a mount on a file. So `mount` watches two things: the mount table, and whether the loop has ended.
- **`mount` waits 30 seconds for the volume.** Then it gives up, runs `umount -f` on the path, and returns an error. A session thread that is still inside `fuse_session_mount` then skips the loop and cleans up. One that is already in the loop stays until macOS ends the session: libfuse has no call that wakes it.
- **Requests are counted.** A callback counts its request in, and the reply counts it out. After the loop ends, the thread waits for the count to reach zero before it destroys the session, because a reply needs the session. A task that is dropped before it answers replies "Input/output error", so the count always returns to zero.
- **"Has this session ended?" is asked of the session, never of the path.** Another mount may be at the path by then.

## Errors

The table in [Linux mount adapter](mount-linux.md#errors) holds for macOS: both systems share it (`errno` in `src/lib.rs`), and the test of every row passes on macOS with the same numbers.

## Unmounting

- `Mount::unmount` runs `umount <path>`. This is a plain unmount: it fails with "Resource busy" while any file or folder in the mount is open, and the mount stays served.
- `unmount` returns when the session loop has ended, or 5 seconds after the unmount if the loop has not. It does not wait for a provider call that the unmount itself causes. A request macOS sent before the unmount can hold it: see below.
- **A mount removed from outside.** A user can run `umount` or eject the volume in Finder. The loop ends, and the next `unmount` succeeds without running anything. Until that `unmount`, the helper still lists the mount (#488).
- **A forced unmount waits for a slow provider.** While a request is waiting on the provider, macOS holds `umount -f` until the request is answered. The core answers a provider call after 30 seconds at most (`call_timeout`), so that is the longest wait. This holds for a user's `umount -f` and for the one a dropped `Mount` runs.
- **A request macOS sent itself can hold a plain unmount too.** macOS sends a new mount about ten requests in its first moments. While one of them waits on a slow provider, `umount` returns only when it is answered. A request that the unmount itself causes does not hold it.
- Dropping a `Mount` whose session has not ended runs `umount -f`. Dropping one whose session has ended does nothing, so a later mount at the same path is safe. This differs from Linux, where a drop detaches whatever mount is at the path.
- A helper that is killed leaves a dead mount. A listing of it fails with "Device not configured". `is_dead_mount` reports that, and `unmount_dead` runs `umount -f`. See [Dead mounts](mount-helper.md#dead-mounts).

## Testing

Run `src-tauri/crates/marlin-fuse/test-on-mac.sh` on a Mac that has macFUSE 5 with both switches on. CI does not run it: the runners have no macFUSE.

- It runs the `marlin-fuse` tests with the ignored ones, the `marlin-mount` tests with the ignored ones, and `crates/marlin-mount/test-sweep.sh`. The helper's ignored tests mount a Google account against a fake Google with the in-memory keychain: see [Mount helper](mount-helper.md#testing). No test touches a real account or the real keychain.
- The mount tests are the Linux ones: see [Testing](mount-linux.md#testing). Three differ:
  - The test of bytes kept across opens is Linux only. It tests a flag the macOS adapter does not set.
  - A mount on a file must fail fast. This test is macOS only: Linux lets a mount sit on a file.
  - A second mount at a path survives the drop of the first `Mount`. This test is macOS only: on Linux the drop would detach it.
- The unit tests need no macFUSE: the struct layouts, the attribute rules, the free-space counts, the version check, the three messages, and what is not a dead mount.
- **macFUSE can stop working during a run.** The script mounts about 25 times. After many mounts in a row, macOS can stop finding macFUSE's extension, and every later mount fails (macFUSE issue 1232, open). Turn both switches on again in System Settings, General, Login Items & Extensions, File System Extensions.
- A failed mount prints lines from macFUSE on stderr, such as "Failed to mount volume: mount(8) returned 69". The test of a mount on a file prints one.

## Tested, and not

| What                                                                              | State                                                                      |
| --------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| macOS 27.2 on Apple silicon, macFUSE 5.5.0, mounts made by test programs          | Tested by `test-on-mac.sh`, 2026-11-16                                     |
| A test program signed ad hoc with the hardened runtime and the app's entitlements | Mounts, 2026-11-16. With no entitlements, macOS refuses macFUSE's library. |
| A build signed with the Developer ID certificate and notarized                    | Not tested (#543).                                                         |
| Finder, Quick Look, drag and drop, file dialogs, sleep and wake                   | Not tested (#543).                                                         |
| A mount started from the app, and a real Google account                           | Not tested (#543).                                                         |
| The switches off, and no macFUSE                                                  | Not tested (#543).                                                         |
| An Intel Mac, and macOS 15.4 to 26                                                | Not tested.                                                                |
| Read speed, and the calls Finder makes in a large folder                          | Not measured (#543).                                                       |

Known limits:

- A file or folder whose name is longer than 255 bytes is hidden.
- A file opens only by the exact bytes of its name (#542).
- The free space can be out of date, and it stops at 16 TiB.
- macFUSE and libfuse write their own plain lines to the helper's stderr, so those lines land in the helper's log between its JSON lines.
- After many mounts, macOS can stop finding macFUSE's extension until the user turns it on again (macFUSE issue 1232).
- The library path is fixed: `/usr/local/lib/libfuse3.4.dylib`, where macFUSE 5 installs it.
- The adapter does not handle libfuse's interrupt request. A call that an app gave up on still runs until the core's time limit ends it.
- Read-only: every write fails with "Read-only file system" until #599.
