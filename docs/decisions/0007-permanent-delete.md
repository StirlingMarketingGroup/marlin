# 0007: A permanent delete walks the folder on four threads and never follows a link

**Status:** accepted
**Date:** 2026-11-19
**Issue:** #48

## Decision

1. **A permanent delete of a local folder runs on four threads**, in one pool that every delete in the app shares (`THREADS` in `src-tauri/src/delete_entry.rs`).
2. **On macOS and Linux, Marlin walks the folder itself, with `rustix`.** Every step is relative to an open folder handle, starting at the parent of the selected entry. A folder opens with `O_NOFOLLOW`, so a link is unlinked and never entered, even one given as `link/` or swapped in during the delete.
3. **On Windows, Marlin uses the `remove_dir_all` crate**, run in the same pool. Marlin first decides what the selected entry is: a folder goes to the crate, a link to a folder or a junction is removed as a folder, anything else as a file.
4. **A folder on another device than its parent is not entered** (macOS and Linux). The delete stops there with the message "it is on another drive or volume, so it was left in place". A disk image, a network share, or a Marlin mount inside a deleted folder keeps its files.
5. **On macOS and Linux, the app raises a lower soft open-file limit at start** to the smaller of 4096 and the hard limit.

## Why

**Threads are what make a delete fast, not a native tool.** Measured 2026-11-19 on a tree of 100,000 files in 400 folders, on a Mac that was busy with other work. The prototype was a path-based walk; the shipped column is the code in `delete_entry.rs`:

| File system              | std `remove_dir_all` | `rm -rf` | rsync empty-folder trick | robocopy `/MIR` | 4 threads, prototype | 4 threads, shipped |
| ------------------------ | -------------------- | -------- | ------------------------ | --------------- | -------------------- | ------------------ |
| macOS APFS (M4 Max)      | 4.7 s                | 4.9 s    | 4.9 s                    | –               | 2.5 s                | 2.8 s              |
| Linux ext4 (Docker)      | 0.65 s               | 0.63 s   | 0.71 s                   | –               | 0.25 s               | 0.23 s             |
| Windows NTFS (VM, 4 CPU) | 13.0 s               | –        | –                        | 56.8 s          | 4.3 s                | 3.0 s              |

**Four threads, not one per core.** On APFS, 4 to 6 threads were fastest. 8 threads took 3.2 s, and 16 took 7.1 s, slower than one thread. On ext4, 8 threads were a little faster than 4 (0.19 s), and 16 were slower again (0.58 s).

**A delete must never follow a link.** A path-based parallel walk would follow a folder that another program swaps for a link. Opening each child relative to its parent's handle with `O_NOFOLLOW` prevents that.

**The crate is unsafe on Unix and slow on macOS.** Its `remove_dir_all` first calls `canonicalize` on the path. That resolves a selected link to its target, so deleting the link would empty the target. It also turns its threads off on macOS. On Windows it opens each folder with `FILE_FLAG_OPEN_REPARSE_POINT`, refuses a link, and works from open handles. Its path step there (`GetFullPathNameW`) does not resolve links.

**A parallel walk holds more folders open.** Each thread holds one open folder per level it is in. A synthetic tree 30 folders deep peaked at about 118 extra open files. macOS starts an app with room for 256. Marlin caps the increase at 4096 because the apps it opens inherit the soft limit.

**A mounted drive is not part of the folder.** `rm -rf` and std empty a drive mounted inside the tree, then fail at its mount point. The device check stops before the drive.

## Rejected

- **Running `rsync`, `rm`, or `robocopy`.** No faster than std (table above), and robocopy was four times slower.
- **The `remove_dir_all` crate on every OS.** See above.
- **A path-based parallel walk.** About as fast as the handle-based walk (2.5 s on APFS), but it follows a link swapped in during the delete.
- **One thread per core.** Slower than one thread on APFS at 16.
- **Falling back to std when the pool cannot start.** std gets the path as given, and `link/` makes it empty the link's target. Rayon's global pool runs the same walk instead.

## What follows

- **On Windows, an error does not name the child that failed.** The crate does not return it. On macOS and Linux the error names it, from inside the selected folder: `locked/file.txt: Permission denied (os error 13)`.
- **A btrfs subvolume inside a deleted folder is left in place**, because it has its own device number.
- **A bind mount on the same device is not detected.** That would need mount IDs.
- **On Windows, a child that another program removes during the delete fails the delete.** The crate does not skip it, as std did. Deleting again finishes the job.
- **Not measured:** spinning disks, network drives, FUSE mounts as the deleted folder, and FAT or exFAT drives on Windows.
- The progress a delete shows belongs to #169.
