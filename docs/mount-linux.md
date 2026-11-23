# Linux mount adapter (`marlin-fuse`)

`marlin-fuse` serves one [mount core](mount-core.md) `Vfs` to Linux as a FUSE mount. The [mount helper](mount-helper.md) uses it on Linux. The same crate serves macOS through macFUSE: see [macOS mount adapter](mount-macos.md). The adapter serves writes when `Vfs::writable()` is true, and mounts read-only otherwise. [Decision 0005](decisions/0005-mount-writes.md) says how a write works. A mount the helper makes takes writes when the helper gives it a spool: see [Unsent files](mount-helper.md#unsent-files).

The helper mounts a Google account through it: see [Providers](mount-helper.md#providers). No part of the UI asks for a mount yet (#377).

**Covers:** `src-tauri/crates/marlin-fuse/` except its macOS half (`src/macos.rs`, `src/macos/`, `test-on-mac.sh`), and the adapter wiring in `src-tauri/crates/marlin-mount/src/adapter.rs`
**Checked against the code:** 2026-11-20

The design is in [decision 0001](decisions/0001-mount-architecture.md), choices 2 and 6.

## The parts

| File                | Holds                                                                                                                                                                                    |
| ------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/lib.rs`        | What both systems share: the errno table, the limits, the made-up permissions, the name rule, the open access rule, the `setattr` order, the names that stay local, and `is_dead_mount`. |
| `src/linux.rs`      | `mount`, `Mount::unmount`, `unmount_dead`, `Error`, and the check of what the machine has.                                                                                               |
| `src/linux/fs.rs`   | The `fuser::Filesystem` callbacks and the attribute rules.                                                                                                                               |
| `tests/mount.rs`    | Tests that mount for real. They are marked `#[ignore]`, and `test-in-docker.sh` runs them.                                                                                               |
| `test-in-docker.sh` | Runs every test in a container, as a user who is not root.                                                                                                                               |

- The crate is the only one that imports `fuser`. The version is pinned exactly (`=0.18.0`), because `fuser` changes its `Filesystem` trait between releases (risk 8 in decision 0001).
- The Linux code and `fuser` are behind `cfg(target_os = "linux")`. A Mac compiles the macOS half of the crate, so it never checks this code. The CI check runs on Linux and does. To check it from a Mac, run `rustup target add x86_64-unknown-linux-gnu` once, then `cargo check -p marlin-fuse --all-targets --target x86_64-unknown-linux-gnu` in `src-tauri/`. That does not work for `marlin-mount`: it links the Google Drive provider, whose TLS code is C and needs a Linux C compiler. `test-in-docker.sh` compiles and tests the helper's Linux half.
- The build links no libfuse. `fuser` speaks the kernel protocol itself and runs `fusermount3` to mount.

## What the machine needs

| Need                        | When it is missing, the mount fails with                                           |
| --------------------------- | ---------------------------------------------------------------------------------- |
| `/dev/fuse`                 | Advice to run `sudo modprobe fuse`, or to pass the device into a container.        |
| `fusermount3` on the `PATH` | The command that installs the `fuse3` package on Debian, Ubuntu, Fedora, and Arch. |

The helper sends this text to the app as error -1006. Any other failure carries what `fuser` or `fusermount3` reported.

The `.deb` and `.rpm` packages depend on `fuse3` (`src-tauri/tauri.conf.json`). An AppImage cannot declare a dependency, so it relies on the message above.

## How a mount behaves

- **Mount options:** `rw` when the mount takes writes, and `ro` when it does not. Also `default_permissions`, file system name `marlin`, subtype `marlin`. The mount shows in `/proc/self/mountinfo` with the type `fuse.marlin`.
- **Only the mounting user can open it.** The adapter never passes `allow_other`, so the kernel refuses every other user, root included.
- **Made-up permissions.** The mounting user owns everything. A mount that takes writes shows a folder as mode 0700 and a file as mode 0600. A read-only mount shows 0500 and 0400. `default_permissions` makes the kernel enforce them, so the adapter has no access callback.
- **Times.** Access and change times repeat the modified time. A Linux FUSE reply has no creation time.
- **Caching.** The kernel may reuse a name and its attributes for 1 second.
- **File bytes stay cached across opens.** The adapter answers each open with `FOPEN_KEEP_CACHE`, so a preview and a thumbnailer that open the same file fetch it once. It also asks the kernel for `FUSE_AUTO_INVAL_DATA` when the mount starts. With that flag the kernel checks a file's size and modified time before it serves cached bytes, and drops them when either changed. `fuser` 0.18.0 does not ask for that flag by itself. On a kernel without it, the adapter does not keep the bytes. An open within about 6 seconds of a change on the remote can still get the old bytes: the core reuses a listing and a node's attributes for up to 5 seconds, and the kernel keeps its answer for 1 second. The kernel asks for the attributes of a file that is being read at most once a second. The core asks the provider for them at most once in 5 seconds ([Rules the core keeps](mount-core.md#rules-the-core-keeps)).
- **Node IDs.** An inode number is the core's node ID, and a file handle is the core's handle ID. The kernel's forget calls go straight to the core ([decision 0002](decisions/0002-node-ids-follow-lookup-counts.md)).
- **Listings.** A folder lists `.` and `..` first. `..` carries the inode number `0xffffffff`, which is libfuse's value for "not known": the core keeps no parent for a node.
- **Free space.** `statfs` reports the provider's total and free bytes in 4,096-byte blocks. The core reuses the provider's answer for 30 seconds.
- **Names.** A name longer than 255 bytes is not listed and cannot be opened (`ENAMETOOLONG`). A name that is not UTF-8 cannot be opened (`ENOENT`). A new name that is not UTF-8 is refused (`EINVAL`), because the remote cannot hold it. The core sets no length limit, so each adapter sets its own.
- **A slow call blocks nothing else.** Each kernel request that may reach the provider becomes a tokio task, and the `fuser` thread goes back for the next request at once. `forget`, `opendir`, `readdir` past offset 0, and the two release calls answer in place. They touch the core's own state, and the last writer's release also writes the file's small spool record, with no sync. `opendir` takes no listing. A `readdir` from offset 0 calls `Vfs::rewind_folder` in a task, which takes the listing then: at the first read, and again at a `rewinddir`.

## Writes

- `create` calls `Vfs::create` and opens the new file. The mode and the umask are ignored. The open uses the same keep-cache flag as `open`.
- `write` calls `Vfs::write`. An `O_APPEND` write uses the size the kernel holds. The kernel puts the offset at the end of the file before the call.
- `setattr` calls `Vfs::truncate` when the size is set, then `Vfs::set_modified` when the modified time is set. It answers with `Vfs::attributes` after them. An owner or a mode change succeeds and changes nothing. No access time is kept.
- An `open` with `O_TRUNC` arrives as `setattr` with size 0. The adapter does not ask for `FUSE_ATOMIC_O_TRUNC`. The core cuts a clean file to zero without reading it.
- `mkdir` calls `Vfs::create_folder`. `unlink` calls `Vfs::remove` for a file. `rmdir` calls `Vfs::remove` for a folder.
- `rename` calls `Vfs::rename`. `RENAME_NOREPLACE` refuses a name that is taken. Any other rename replaces it. `RENAME_EXCHANGE` and `RENAME_WHITEOUT` fail with `EINVAL`.
- `fsync` calls `Vfs::fsync`. `release` calls `Vfs::release`. The settle time starts at `release`, not at `flush`.

`flush` is left out. Every write is in the spool before it answers, so a close has nothing to flush. After the first `ENOSYS` the kernel stops sending `flush`. `mknod` is left out: the kernel uses `create` for a regular file. `fsyncdir`, `fallocate`, `copy_file_range`, `lseek`, `ioctl`, the xattr calls, and `getlk`/`setlk` are left out. For `copy_file_range` the kernel then copies with reads and writes. For `SEEK_DATA` and `SEEK_HOLE` it treats the whole file as data, up to the size it holds. `link` and `symlink` stay at `fuser`'s `EPERM`.

The adapter does not ask for `FUSE_WRITEBACK_CACHE`. A `write` call goes through the kernel's page cache and reaches the adapter before it returns. A store through a shared writable memory map reaches the adapter later, when the kernel writes the page back. A failed write clears the pages it touched. A size or time change drops pages through `FUSE_AUTO_INVAL_DATA`. The pages kept across opens stay the bytes this mount last wrote.

## Errors

| Core error     | Linux error   | Linux error text          |
| -------------- | ------------- | ------------------------- |
| `NotFound`     | `ENOENT`      | No such file or directory |
| `AccessDenied` | `EACCES`      | Permission denied         |
| `ReadOnly`     | `EROFS`       | Read-only file system     |
| `Conflict`     | `EEXIST`      | File exists               |
| `NotAFolder`   | `ENOTDIR`     | Not a directory           |
| `IsAFolder`    | `EISDIR`      | Is a directory            |
| `NotEmpty`     | `ENOTEMPTY`   | Directory not empty       |
| `NoSpace`      | `ENOSPC`      | No space left on device   |
| `SignInNeeded` | `EACCES`      | Permission denied         |
| `TimedOut`     | `ETIMEDOUT`   | Connection timed out      |
| `Offline`      | `ENETUNREACH` | Network is unreachable    |
| `NotSupported` | `ENOTSUP`     | Operation not supported   |
| `StaleHandle`  | `ESTALE`      | Stale file handle         |
| `Other`        | `EIO`         | Input/output error        |

**Never map a provider error to `ENOSYS`.** The kernel reads it as "this filesystem has no such call" and stops sending that call for the life of the mount. Only the callbacks the adapter leaves out answer it, through `fuser`'s defaults.

These errors reach an app without a core error:

| Case                                   | Linux error    | Linux error text        |
| -------------------------------------- | -------------- | ----------------------- |
| `RENAME_EXCHANGE` or `RENAME_WHITEOUT` | `EINVAL`       | Invalid argument        |
| A new name that is not UTF-8           | `EINVAL`       | Invalid argument        |
| A name over 255 bytes                  | `ENAMETOOLONG` | File name too long      |
| A hard link or a symbolic link         | `EPERM`        | Operation not permitted |
| A write on a read-only mount           | `EROFS`        | Read-only file system   |

`EINVAL` for those rename flags is what Linux file systems give for a rename flag they do not support. [Decision 0005](decisions/0005-mount-writes.md)'s "not supported" for an exchange is this `EINVAL`, not the core's `NotSupported`. `fuser` answers `EPERM` for the links. The kernel answers `EROFS` when the mount is read-only, before the call reaches the adapter.

## Unmounting

- `Mount::unmount` runs `fusermount3 -u`. This is a plain unmount: it fails while any file or folder in the mount is open, and the mount stays served. That is what the helper's `Session` trait asks for.
- `unmount` returns when `fusermount3` does, and does not wait for the session thread. That thread ends when the kernel closes the connection, which waits for every copy of the mount. A container that bound the user's home folder holds a copy in its own mount namespace.
- `Mount::unmount` never calls `fuser`'s own unmount. In `fuser` 0.18.0 that one is lazy: it detaches a busy mount at once, so the mount point looks free while open files are still served.
- **A mount removed from outside.** A user can run `fusermount3 -u` or eject the mount in a file manager. The next `unmount` sees that the mount table no longer lists the mount and succeeds without running anything. Until that `unmount`, the helper still lists the mount (#488).
- Dropping a `Mount` makes `fuser` lazily detach whatever FUSE mount is at the path. `fuser` 0.18.0 cannot tell that the mount is already gone, so this also runs after a successful unmount, where it finds nothing. The helper drops a mount before it frees the mount's name. Untested: whether the helper drops its mounts when it exits after a signal. If it does, a busy mount is detached then. If it does not, the mount is left dead.
- A helper that is killed leaves a dead mount. A listing of it fails with "Transport endpoint is not connected". `is_dead_mount` reports that, and `unmount_dead` runs `fusermount3 -u -z`. The unmount is lazy on purpose: a plain one fails while a process has its working folder inside, and a dead mount has no open file to protect. The helper calls both when it starts and at each mount: see [Dead mounts](mount-helper.md#dead-mounts).

## Testing

Run `src-tauri/crates/marlin-fuse/test-in-docker.sh`. It needs Docker.

- The container gets `/dev/fuse`, `CAP_SYS_ADMIN`, and no AppArmor profile. The tests run as a user who is not root, so every mount goes through the setuid `fusermount3`, as it does for any user who is not root.
- It also runs the helper's tests (`marlin-mount`). The helper's Linux half, the wiring to this adapter, is compiled out on a Mac. Two of those tests mount a Google account against a fake Google: see [Mount helper](mount-helper.md#testing).
- Last, it runs `crates/marlin-mount/test-sweep.sh`, which kills a mount's server and checks that the helper clears the dead mount. The script mounts with the spike program in `spikes/mount-hello/`, so the container gets the whole repo, read-only.
- The first run downloads and builds everything. Two Docker volumes keep the registry and the build folder for later runs.
- The unit tests cover the attribute rules, the two "missing" messages, and what is not a dead mount: a folder, a missing path, a symbolic link. They need no kernel support.
- The mount tests mount a `MemoryProvider` and use ordinary file calls: list, read at an offset, free space, a 1,000-file folder, a listed folder whose files open with no provider call, a name that is too long, every row of the error table, four slow reads at once, an unmount while a file is open, an unmount after one from outside, an unmount while the provider is slow, two mounts at once (one at a path with a space), a dropped mount, an unmount from outside and a drop while a read is waiting, a name with an accent, a live mount that must not count as dead, a listing that shows a new file, a removed file, and a file made before a `rewinddir`, and the cached bytes: a second open makes no provider read, and a file with a new modified time is read again.
- The write tests are `a_provider_that_cannot_write_mounts_read_only`, `copies_reach_the_provider`, `editor_saves_keep_one_file`, `dd_into_the_middle_changes_only_those_bytes`, `a_write_is_read_back_at_once_also_by_another_process`, `fsync_puts_open_bytes_in_the_spool`, `names_change_through_the_mount`, `preserved_times_reach_the_provider`, and `a_copy_bigger_than_the_spool_waits_for_uploads`. They run on macOS too: see [macOS mount adapter](mount-macos.md#testing). The container installs `rsync` and `vim-tiny`. `copies_reach_the_provider` and `preserved_times_reach_the_provider` need `rsync`. `editor_saves_keep_one_file` needs `vim-tiny`.
- On a Linux machine, `cargo test -p marlin-fuse` runs the unit tests only. Add `-- --include-ignored --test-threads=1` to run the mount tests there.
- The mount tests run one at a time. In parallel they fail now and then: a test that starts a program forks, and the child briefly holds a copy of a file that another test has open on its mount, so that test's unmount is refused as busy.

**Measured** in the container on 2026-11-20, with the default 2-second settle time (risk 5 of [decision 0005](decisions/0005-mount-writes.md)):

- Copies of 9 files with `cp` and `rsync`, one of them from the mount into the mount, made 9 uploads.
- A `sed -i` save and a `vim.tiny` save each made one upload and kept the item's ID. Debian 12's `vim-tiny` writes over the file in place, so it does not hit risk 6.
- A `cp -r` of four 16 MiB files through a 32 MiB spool took 2.1 seconds: the copy waited one settle time for room.

These runs use `MemoryProvider`, which uploads at once. They say nothing about the 10 GiB default limit or about a real provider's upload speed. #543 and #486 repeat them on real desktops.

## Tested, and not

| What                                                                           | State                                                     |
| ------------------------------------------------------------------------------ | --------------------------------------------------------- |
| Debian 12 container (`fuse3` 3.14), a user who is not root                     | Tested by `test-in-docker.sh`, 2026-11-20                 |
| A CI runner                                                                    | Not tested. Risk 9 in decision 0001 is still open (#485). |
| A bare Linux desktop: file managers, drag and drop, file dialogs, thumbnailers | Not tested (#486).                                        |
| Flatpak, Snap, AppImage                                                        | Not tested (#486).                                        |

Known limits:

- A file or folder whose name is longer than 255 bytes is hidden.
- A file whose bytes change while its size and its modified time stay the same keeps its old cached bytes until the kernel drops them. A provider must change one of the two: see [Mount core](mount-core.md).
- The adapter does not handle the kernel's interrupt request. A call that an app gave up on still runs until the core's time limit ends it.
- An append uses the size the kernel holds. When another client grows the file, an append through a descriptor that has not seen the new size lands at the old end, over the other client's bytes. An open descriptor that does not stat keeps the old size. A new open can be up to about 6 seconds behind. NFS has the same limit.
- `mknod` answers `ENOSYS`, so no FIFO, socket, or device can be made. An app that makes a file with `open` reaches `create`.
- No item shows as read-only on its own. No provider reports that yet.
- A modified time set on a folder, or on a file that is not in the spool, succeeds and changes nothing. So `touch` on a file that has uploaded keeps its old time: see `set_modified` in [Mount core](mount-core.md#what-an-adapter-calls).
- Run as root, `fuser` mounts without `fusermount3`, and the mount's type is `fuse` with no subtype. The helper is a per-user process and is not meant to run as root.
