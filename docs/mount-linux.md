# Linux mount adapter (`marlin-fuse`)

`marlin-fuse` serves one [mount core](mount-core.md) `Vfs` to Linux as a FUSE mount. The [mount helper](mount-helper.md) uses it on Linux. It is read-only, like the core, until #376.

No real provider exists yet (#372), so today only the tests mount anything.

**Covers:** `src-tauri/crates/marlin-fuse/`, and the adapter wiring in `src-tauri/crates/marlin-mount/src/main.rs`
**Checked against the code:** 2026-11-13

The design is in [decision 0001](decisions/0001-mount-architecture.md), choices 2 and 6.

## The parts

| File                | Holds                                                                                                       |
| ------------------- | ----------------------------------------------------------------------------------------------------------- |
| `src/lib.rs`        | `mount`, `Mount::unmount`, `is_dead_mount`, `unmount_dead`, `Error`, and the check of what the machine has. |
| `src/fs.rs`         | The `fuser::Filesystem` callbacks, the attribute rules, and the errno table.                                |
| `tests/mount.rs`    | Tests that mount for real. They are marked `#[ignore]`, and `test-in-docker.sh` runs them.                  |
| `test-in-docker.sh` | Runs every test in a container, as a user who is not root.                                                  |

- The crate is the only one that imports `fuser`. The version is pinned exactly (`=0.18.0`), because `fuser` changes its `Filesystem` trait between releases (risk 8 in decision 0001).
- Every dependency is behind `cfg(target_os = "linux")`. On macOS the crate compiles to nothing, so a Mac never checks this code. The CI check runs on Linux and does. To check it from a Mac, run `rustup target add x86_64-unknown-linux-gnu` once, then `cargo check -p marlin-fuse -p marlin-mount --all-targets --target x86_64-unknown-linux-gnu` in `src-tauri/`.
- The build links no libfuse. `fuser` speaks the kernel protocol itself and runs `fusermount3` to mount.

## What the machine needs

| Need                        | When it is missing, the mount fails with                                           |
| --------------------------- | ---------------------------------------------------------------------------------- |
| `/dev/fuse`                 | Advice to run `sudo modprobe fuse`, or to pass the device into a container.        |
| `fusermount3` on the `PATH` | The command that installs the `fuse3` package on Debian, Ubuntu, Fedora, and Arch. |

The helper sends this text to the app as error -1006. Any other failure carries what `fuser` or `fusermount3` reported.

The `.deb` and `.rpm` packages depend on `fuse3` (`src-tauri/tauri.conf.json`). An AppImage cannot declare a dependency, so it relies on the message above.

## How a mount behaves

- **Mount options:** `ro`, `default_permissions`, file system name `marlin`, subtype `marlin`. The mount shows in `/proc/self/mountinfo` with the type `fuse.marlin`.
- **Only the mounting user can open it.** The adapter never passes `allow_other`, so the kernel refuses every other user, root included.
- **Made-up permissions.** The mounting user owns everything. A folder has mode 0500 and a file has mode 0400. `default_permissions` makes the kernel enforce them, so the adapter has no access callback.
- **Times.** Access, change, and creation times repeat the modified time.
- **Caching.** The kernel may reuse a name and its attributes for 1 second.
- **Node IDs.** An inode number is the core's node ID, and a file handle is the core's handle ID. The kernel's forget calls go straight to the core ([decision 0002](decisions/0002-node-ids-follow-lookup-counts.md)).
- **Listings.** A folder lists `.` and `..` first. `..` carries the inode number `0xffffffff`, which is libfuse's value for "not known": the core keeps no parent for a node.
- **Free space.** `statfs` reports the provider's total and free bytes in 4,096-byte blocks.
- **Names.** A name longer than 255 bytes is not listed and cannot be opened (`ENAMETOOLONG`). The core sets no length limit, so each adapter sets its own.
- **A slow call blocks nothing else.** Each kernel request that may reach the provider becomes a tokio task, and the `fuser` thread goes back for the next request at once. `forget`, `readdir`, and the two release calls only touch the core's own state, so they answer in place.

## Errors

| Core error     | Linux error   | Linux error text          |
| -------------- | ------------- | ------------------------- |
| `NotFound`     | `ENOENT`      | No such file or directory |
| `AccessDenied` | `EACCES`      | Permission denied         |
| `ReadOnly`     | `EROFS`       | Read-only file system     |
| `Conflict`     | `EEXIST`      | File exists               |
| `NotAFolder`   | `ENOTDIR`     | Not a directory           |
| `IsAFolder`    | `EISDIR`      | Is a directory            |
| `NoSpace`      | `ENOSPC`      | No space left on device   |
| `SignInNeeded` | `EACCES`      | Permission denied         |
| `TimedOut`     | `ETIMEDOUT`   | Connection timed out      |
| `Offline`      | `ENETUNREACH` | Network is unreachable    |
| `NotSupported` | `EOPNOTSUPP`  | Operation not supported   |
| `StaleHandle`  | `ESTALE`      | Stale file handle         |
| `Other`        | `EIO`         | Input/output error        |

**Never map a provider error to `ENOSYS`.** The kernel reads it as "this filesystem has no such call" and stops sending that call for the life of the mount. Only the callbacks the adapter leaves out answer it, through `fuser`'s defaults.

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
- It also runs the helper's tests (`marlin-mount`). The helper's Linux half, the wiring to this adapter, is compiled out on a Mac.
- Last, it runs `crates/marlin-mount/test-sweep.sh`, which kills a mount's server and checks that the helper clears the dead mount. The script mounts with the spike program in `spikes/mount-hello/`, so the container gets the whole repo, read-only.
- The first run downloads and builds everything. Two Docker volumes keep the registry and the build folder for later runs.
- The unit tests cover the attribute rules, the two "missing" messages, and what is not a dead mount: a folder, a missing path, a symbolic link. They need no kernel support.
- The mount tests mount a `MemoryProvider` and use ordinary file calls: list, read at an offset, free space, a 1,000-file folder, a name that is too long, every row of the error table, four slow reads at once, an unmount while a file is open, an unmount after one from outside, an unmount while the provider is slow, two mounts at once (one at a path with a space), a dropped mount, and a live mount that must not count as dead.
- On a Linux machine, `cargo test -p marlin-fuse` runs the unit tests only. Add `-- --include-ignored --test-threads=1` to run the mount tests there.
- The mount tests run one at a time. In parallel they fail now and then: a test that starts a program forks, and the child briefly holds a copy of a file that another test has open on its mount, so that test's unmount is refused as busy.

## Tested, and not

| What                                                                           | State                                                     |
| ------------------------------------------------------------------------------ | --------------------------------------------------------- |
| Debian 12 container (`fuse3` 3.14), a user who is not root                     | Tested by `test-in-docker.sh`, 2026-11-13                 |
| A CI runner                                                                    | Not tested. Risk 9 in decision 0001 is still open (#485). |
| A bare Linux desktop: file managers, drag and drop, file dialogs, thumbnailers | Not tested. It needs a real provider (#486).              |
| Flatpak, Snap, AppImage                                                        | Not tested (#486).                                        |

Known limits:

- A file or folder whose name is longer than 255 bytes is hidden.
- The adapter does not handle the kernel's interrupt request. A call that an app gave up on still runs until the core's time limit ends it.
- Read-only: every write fails with `EROFS` until #376.
- Run as root, `fuser` mounts without `fusermount3`, and the mount's type is `fuse` with no subtype. The helper is a per-user process and is not meant to run as root.
