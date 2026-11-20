# 0005: A mount takes writes into a local spool and uploads after the last close

**Status:** accepted
**Date:** 2026-11-18
**Issue:** #376 (epic #368)

## Decision

A mount accepts writes in two ways. A change to a name goes to the remote inside the OS call. A change to a file's bytes goes to a local file first, and reaches the remote in the background.

[Decision 0001](0001-mount-architecture.md) left three questions to this record: what `fsync` waits for, what happens when the remote file changed, and how unsent bytes survive a crash. Choices 5, 7, and 10 answer them.

1. **Folder and name changes go to the provider at once.** Make a folder, remove, rename, and move each send one change call to the provider inside the OS call. Finding the names first can take a listing of each folder the change names. The app gets the provider's answer. A provider that has a trash moves a removed or replaced item there. Removing a folder that holds anything fails with "not empty".
2. **The core shows its own changes at once.** After a change succeeds, the core edits the folder listing it holds. It does not wait for the remote to list the change. A listing the core asked for before the change ended, and that answers after it, is not kept. When a change fails, the core drops the listing, because the remote may or may not have changed.
3. **A file's new bytes go to the write spool.** The spool is one local file per changed remote file, on the user's disk. Every write, at any offset, goes to that local file. A write callback never uploads, and no file is held in memory.
   - **A new file lives only in the spool until its first upload.** Creating a file makes no provider call. The mount lists the file, and renames and removes it, from the spool.
   - **An existing file is fetched on its first write, not when it is opened.** Many apps open a file for writing and never write. A write to a file that was just cut to zero bytes fetches nothing. Any other first write copies the whole remote file into the spool first, in pieces.
   - **While a file is in the spool, the spool is the file.** Every read, the size, and the modified time come from the spool, for every open handle.
4. **The upload starts after the last writer closes.** When no handle that can write is left, the file waits a short settle time (2 seconds by default), then uploads whole. A new open for writing, a rename, or a remove in that time changes what is sent, or stops it. A write to a file that is uploading stops that upload: the file is open again, and it uploads from the start after its next last close. A remove of a file that is uploading stops the upload too, and when the remote already has the file, the core removes it there before the remove returns. Uploads have their own limit of calls at once, apart from the limit on other provider calls, and no fixed time limit: a provider reports progress, and an upload that stops moving is what fails.
5. **`close` and `fsync` mean "safe on this computer", not "on the remote".**
   - `close` returns when the bytes are in the spool. It reports a local write error, such as a full disk.
   - `fsync` returns when the spool file and its record are on the disk.
   - Neither waits for the upload. The mount helper reports which files wait, which are uploading, and which failed, and the app shows it.
6. **A provider gets whole files.** The provider calls for bytes are "make this new file from this local file" and "replace this item's bytes with this local file". There is no call for a part of a file.
7. **A changed remote file makes a conflict copy, never a lost edit.** Each item carries a revision: a value the provider changes whenever the item's bytes change. The spool notes the revision its bytes were based on. The replace call sends it, and the provider refuses when the remote's revision differs. The core then uploads the spool as a new file beside the original, named `{name} (conflict {date} {time}){extension}`, and leaves the remote's version alone. If the remote item is gone, the core uploads the spool as a new file under the same name.
8. **A failed upload keeps its bytes and is retried.** One rule covers every failure: the spool file stays, the mount keeps serving it, and the core tries again after 5 seconds, then at growing waits up to 5 minutes, for as long as the mount is up. The helper reports the file and its last error. The user can ask for a retry now, or discard the edit. Nothing else removes unsent bytes.
9. **A rename over an existing file keeps that file's identity when it can.** Many apps save by writing a new file and renaming it over the old one. When the renamed file is still only in the spool, the core does not replace the old item. It makes the spooled bytes the new bytes of the old item, based on the revision the core holds for the old item at the rename, and the new file never reaches the remote. On Google Drive this keeps the file's ID, its share links, and its history. When the renamed file is already on the remote, the provider replaces the old item.
10. **The spool survives a crash.** It lives in the app's data folder, not in a cache folder, with one folder per account, open to the user only. Each spooled file has a small record beside it: the item or, for a new file, the folder and name; the revision it was based on; and whether its writers had closed.
    - At the next mount of the account, a file whose writers had closed goes back in the upload queue.
    - A file that was still open when the helper died may hold half an edit. It is uploaded as a new file named `{name} (recovered {date} {time}){extension}`, and never over the original.
    - A folder or name change is one provider call with no record. After a crash the remote has it or does not.
11. **The spool has a size limit, and unsent bytes are never evicted.** The default limit is 10 GiB or half the free space on the disk, whichever is smaller. A spool file is deleted when the upload that the provider confirmed sent the bytes it holds now. A write that would pass the limit waits for uploads to free room, up to the call time limit, then fails with "no space". So one file larger than the limit cannot be written.
12. **The spool is not encrypted.** It is as safe as any file the user saves in their home folder: it relies on the account's file permissions and on disk encryption.
13. **What cannot be changed says so.** A provider that has no write calls gives a read-only mount: #598 adds the question an adapter asks a provider before it mounts. An item the account may not change shows without write permission when the provider knows that. A change the remote cannot make fails with one of the core's errors: read-only, access denied, or not supported. What a Google Drive link file allows, which [decision 0003](0003-gdrive-link-files.md) left to the write work, is decided with the Drive calls in #600.
14. **Some names stay on the computer.** Finder writes `.DS_Store` and `._name` files into every folder it touches. A mount accepts them into the spool and never uploads them. They are gone after an unmount, and they are left out of choice 10: a mount that finds one in a spool from an earlier run deletes it. This replaces the "Extended attributes" row of decision 0001, which had the adapter refuse them.

A file's bytes are in one of these states:

| State     | The mount serves | What moves it on                                                                                                  |
| --------- | ---------------- | ----------------------------------------------------------------------------------------------------------------- |
| Clean     | The remote       | The first write, or a create                                                                                      |
| Open      | The spool        | The last writer closes                                                                                            |
| Waiting   | The spool        | The settle time ends, or a new writer opens the file                                                              |
| Uploading | The spool        | The provider confirms, refuses the revision, or fails. A new write stops the upload and makes the file open again |
| Failed    | The spool        | The retry time comes, or the user asks for a retry or a discard                                                   |

A mount still has no hard links, no new symbolic links, no locks sent to the remote, and no extended attributes. Changing an owner or a mode succeeds and does nothing. Setting a modified time on a spooled file is sent with its upload. A rename that swaps two names (`RENAME_EXCHANGE`) fails with "not supported".

## Why

**A remote is not a disk.** A write callback carries a few kilobytes at an offset. Google Drive takes whole files. Each request to it takes 0.25 to 0.5 seconds ([Google's limits](../mount-gdrive.md#googles-limits)). An editor that saves a 1 MB file makes hundreds of write callbacks. Only a local file can take them at disk speed.

**The OS decides when a filesystem hears about a close.** From the comments in libfuse's `fuse_lowlevel.h`:

- `flush` runs on every `close` of a descriptor, and one open can cause many. Its error is what `close` returns.
- `release` runs once, when the last descriptor is gone. Its error is not returned to `close`.
- An app sees no error from anything that runs after `close` returns.

So an upload after the last close cannot report through the file call. It needs its own status, which choices 5 and 8 give it. Blocking `close` until the upload ends would freeze the app for as long as the upload takes, and decision 0001 already rules that out: a slow provider call never blocks a mount.

**Apps save through temporary files.** A save often writes a new file, then renames it over the old one. Swap files, lock files, and Finder's own files come and go around it. If each new file reached the remote when it was created, one save would cost several provider calls inside the app's save, and the remote would fill with files that live for a second. With new files in the spool, a save is local, and only the result is uploaded.

**Google Drive cannot refuse a stale update by itself.** `files.update` in the Drive v3 API has no "only if unchanged" parameter. A provider has to read the item's revision, compare, and then upload. Another change can land between the two. The time between them is short, and the old bytes stay in Drive's own revision history, so this record accepts it.

**The same shape is proven.** rclone's mount with `--vfs-cache-mode writes` buffers written files on disk, uploads them after close and a wait (`--vfs-write-back`, 5 seconds by default), retries a failed upload at growing waits, and uploads what is left at the next start.

**Untested.** Nothing in this record has run against a real kernel, a real Google account, or a real app. The risks below name the check for each.

## Rejected

- **Upload on every write callback.** No provider Marlin has takes a write at an offset as one cheap call, and Drive takes none at all.
- **`close` waits for the upload.** The app freezes for the length of the upload, and a dropped network turns a save into a hang.
- **`fsync` waits for the upload.** Editors call it on every save. It would have the same freeze, and the promise it would add is one the crash-safe spool already makes locally.
- **Create the remote file when the app creates it.** Every temporary file becomes two or three provider calls and a visible file on the remote. A save through a temporary file could not keep the old file's ID.
- **The last writer wins.** It silently destroys the other person's edit.
- **Fail the upload on a conflict and wait for the user.** The edit sits on one computer until someone opens Marlin and decides. A conflict copy puts both versions where every user of the folder can see them.
- **Fetch only the parts of a file that a write touches.** The spool would mix bytes from two versions of the remote file if it changed in between, unless the provider can read an old revision, and most cannot.
- **Fetch the whole file when it is opened for writing.** Apps open for writing without writing. A large file would be downloaded for nothing.
- **Refuse `.DS_Store` and `._name` files.** Nobody has tested how Finder reacts on macFUSE's FSKit backend when it cannot write them. Keeping them local cannot break a copy.
- **macFUSE's `noappledouble` option.** macFUSE says most mount options are not implemented in the FSKit backend.
- **The kernel's write-back cache.** With it the kernel owns the size and the modified time of a file. The core needs to own both while a file is in the spool.
- **The spool in a cache folder or in memory.** The OS and cleanup tools empty cache folders. The spool holds work the user has not saved anywhere else.
- **An offline mode.** A mount does not queue folder and name changes while the remote is out of reach. Those calls fail at once. Only a file's bytes wait.

## What follows

**The work, in order.** Each line is one branch.

| Issue      | What it adds                                                                                                             |
| ---------- | ------------------------------------------------------------------------------------------------------------------------ |
| #376       | This record. Folder and name changes in the core, with choice 2.                                                         |
| #594       | Names that hold `%XX`. Until then the core refuses to make a name it could not show back.                                |
| #595       | The spool in the core: create, write, truncate, read, rename, and remove of a spooled file (choices 3, 9, 11, 14).       |
| #596       | Uploads: the queue, the settle time, the two provider calls for bytes, revisions, conflict copies, retries (4, 6, 7, 8). |
| #597       | The spool's records, and what a mount does with a spool it finds (choice 10).                                            |
| #598       | The Linux adapter serves writes.                                                                                         |
| #599       | The macOS adapter serves writes, with Finder's files (choice 14).                                                        |
| #600       | Google Drive: make a folder, remove to the trash, rename, and move.                                                      |
| #601       | Google Drive: upload a new file, replace a file's bytes, and report revisions.                                           |
| #602       | The helper: where the spool lives, upload status, retry, discard, and what unmount and quit do with unsent files.        |
| #603, #604 | Writes for SFTP and SMB mounts.                                                                                          |

The Windows adapter (#375) serves writes from its first version. Showing upload status in the app's window belongs to #377.

**Risks, and the check for each.**

| #   | Risk                                                                                                                                                                                         | Check, and where                                                                                                                           | If it fails                                                                                                                                                |
| --- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | macFUSE's FSKit backend has only served reads for Marlin. It may fail or misorder write, create, rename, or `setattr` calls.                                                                 | #599 writes, renames, and truncates through a real mount before it builds on them.                                                         | macOS mounts stay read-only until macFUSE fixes it. The core and Linux do not change.                                                                      |
| 2   | Finder may need more than `.DS_Store` and `._name` to copy a file in: extended attributes, a swap rename, or locks.                                                                          | #599 copies files in with Finder. #543 covers other apps.                                                                                  | Answer the calls Finder needs locally, as with the two names.                                                                                              |
| 3   | A listing asked right after a change may not hold the change yet on a real remote. Choice 2 covers the core's cached listing, not an expired one.                                            | #600 lists a folder right after it adds to it, on a real account.                                                                          | The core keeps its own changes for a time and lays them over each new listing.                                                                             |
| 4   | The check-then-upload gap of choice 7 could lose an edit on a busy shared file.                                                                                                              | #601 measures the gap, and confirms Drive keeps the overwritten bytes as a revision.                                                       | Compare the revision again after the upload, and make the conflict copy from Drive's older revision.                                                       |
| 5   | The 2-second settle time and the 10 GiB limit are guesses.                                                                                                                                   | #598 saves from common editors and copies a folder of large files. #543 and #486 repeat it on real desktops.                               | Change the defaults. They are options of the mount.                                                                                                        |
| 6   | An app that renames the old file away and writes a new one under the old name gives the new file a new ID on Drive.                                                                          | #543 and #486 list the editors that do this.                                                                                               | A known limit, or a rule in the core that treats the pair as one save.                                                                                     |
| 7   | A provider whose item IDs are paths changes an item's ID on every rename, and a folder rename changes every ID below it.                                                                     | #603 decides how the core keeps a node across that.                                                                                        | Until then the renamed item keeps its node, and the nodes of items inside a renamed folder go stale ([Mount core](../mount-core.md#rules-the-core-keeps)). |
| 8   | A create whose answer is lost is sent again (choice 8), and a remote that allows two items with one name then holds the file twice. A crash between the upload and its record does the same. | #601 creates a file under an ID it asked Google for first, inside one call. #632 keeps that ID in the spool's record before the first try. | Before a new try, look in the folder for an item with that name, size, and modified time.                                                                  |

**Easier after this.** Any app can save into a remote. Drag and drop onto a mount works. A new provider adds five small calls and gets every write behavior for free.

**Harder after this.** A save that succeeds can leave bytes waiting for upload, so the app has to show what is still on its way, and quitting has to deal with it. A user who closes the laptop right after a large copy has not uploaded it. One file larger than the spool limit cannot be written.

## Sources

Read on 2026-11-18.

- libfuse: [`fuse_lowlevel.h`](https://github.com/libfuse/libfuse/blob/master/include/fuse_lowlevel.h), the comments on `flush`, `release`, and `fsync`
- Google Drive API: [`files.update`](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/update)
- rclone: [rclone mount, "VFS file caching"](https://rclone.org/commands/rclone_mount/)
- macFUSE: [FUSE Backends](https://github.com/macfuse/macfuse/wiki/FUSE-Backends)
