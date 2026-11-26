# 0012: An empty new file waits until nothing in the mount is open for writing

**Status:** accepted
**Date:** 2026-11-26
**Issue:** #629 (epic #368)

## Decision

A new file that holds no bytes does not upload while any file in the mount is open for writing. It uploads once no file in the mount has been open for writing for the settle time. Every other file keeps choice 4 of [decision 0005](0005-mount-writes.md): it waits for its own last writer to close, then for the settle time. This extends choice 4 of 0005.

1. **An empty new file** is a file that is only in the spool (choice 3 of 0005) and is 0 bytes long. A file the remote has, cut to zero bytes, is not one: its upload gives an existing item new bytes.
2. **Any writer holds it back.** A handle open for writing on any file of the mount counts, in any folder, also on a file that never uploads, such as a `._name` file.
3. **The mount's last writer starts the wait.** An empty new file uploads once the settle time has passed since the last handle open for writing in the mount closed. A writer that opens in that time holds it back again. Its own wait still counts too: a file waiting to retry a failed upload keeps that wait (choice 8 of 0005).
4. **It shows as open.** While it waits for other files' writers, the list of unsent files shows it as open, like a file with a writer of its own. An unmount does not wait for an open file, because nothing can close a writer after an unmount. So a writer that is still open at an unmount never holds the unmount up. The empty file stays in the spool, and the next mount uploads it (choice 10 of 0005). A file whose last upload failed or was refused shows that error instead, as before.
5. **Nothing else changes.** A new file with bytes uploads after its own last close, while other files are still being written, so a long copy still reaches the remote as it goes. A `touch` with nothing else open uploads an empty file after the settle time, as before.

## Why

**Finder copies through placeholders.** For each item it copies into a mount, Finder first makes an empty file and closes it at once. When it gets to the item, it removes that file and makes the real one under the same name ([macOS mount adapter](../mount-macos.md#finders-files)). This log is from Finder's `duplicate` of a 900 MB file and a 5-byte file, through a mount that logged every provider call (#599, 2026-11-20, macOS 27.2, macFUSE 5.5.0). The copy took 3.7 seconds:

```
[12.24s] provider create_file "small6.txt" 0 bytes -> item-4
[12.63s] provider remove item-4
[14.72s] provider create_file "big5.bin" 943718400 bytes -> item-5
[14.76s] provider create_file "small6.txt" 5 bytes -> item-6
```

The placeholder's close started its settle time, and Finder was still writing the large file when that time ended. So the placeholder uploaded empty and was then removed on the remote. On Google Drive each removed placeholder goes to the trash, so a slow copy puts one empty file in the trash for every item after the first.

**The calls cannot tell a placeholder from a file made on purpose.** `touch` also makes a file and closes it with no bytes. GNU `cp` of an empty file does the same, and creates with `O_EXCL`, as Finder does. The core's `create` gets no open flags anyway. What differs is what the mount does next: during a copy, another file is being written.

**Holding back an empty file costs the user almost nothing.** The mount lists the new file and serves it from the spool at once. Only another computer sees it later. The spool's record keeps it across a crash.

**A placeholder must never reach the remote.** Removing a file that is only in the spool makes no provider call. So a placeholder that waits until Finder stops writing never shows on the remote and never lands in the trash. It leaves no refused upload and no conflict copy behind, also on SFTP and SMB, whose item IDs are paths.

## Rejected

- **A longer settle time for an empty new file.** A copy can take longer than any fixed time. A copy larger than the spool waits for uploads to free room, and those take as long as the network does. An unmount would also wait that long for every `touch`.
- **Spot Finder by `O_EXCL`.** The core does not see the flag, and `cp` uses it too.
- **Spot Finder by what it writes in the `._name` file.** That means reading the AppleDouble files that macFUSE writes, on macOS only, and nobody has checked what Finder puts there.
- **Only writers in the same folder, or below it.** Finder writes a copied folder's files into new folders below the placeholder's folder, so the check would have to walk up from every writer. One question about the whole mount is simpler, and its only cost is a later upload of an empty file.
- **Only on macOS.** No other system loses anything by the rule, and one rule is easier to test than two.
- **A new state in the list of unsent files.** "Open" already means that the file waits for a writer to close, and an unmount already treats it right. A new state would change the helper's protocol and the app's types for no gain.

## What follows

**Easier after this.** A Finder copy that takes longer than the settle time makes one upload per item and no remove. Google Drive's trash gets no empty files from a copy.

**Harder after this.** An app that keeps a file open for writing holds every empty new file in the mount back until it closes that file, such as a long download saved to the mount. The empty files show on the mount at once, and reach the remote later.

**Not tested.**

- Finder itself. `a_copy_through_placeholders_uploads_each_file_once` replays Finder's calls through a real mount on Linux and macOS: two placeholders made with `O_EXCL`, one removed and written for longer than the settle time, then the other.
- Whether Finder ever pauses longer than the settle time between its placeholders and its first write, with nothing open for writing. A placeholder would then upload, as before this record.

A Finder copy that stops partway, such as one bigger than the free space, leaves its placeholders behind. They upload once nothing is open for writing, as files the user can see and remove.
