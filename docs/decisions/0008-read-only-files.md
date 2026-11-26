# 0008: A mount shows a file it cannot change as read-only, and a refused upload waits

**Status:** accepted; choice 1 extended to folders by [0011](0011-read-only-folders.md)
**Date:** 2026-11-23
**Issue:** #631 (epic #368)

## Decision

A provider says when it knows that the account may not give a file new bytes. The mount shows that file without write permission. The core refuses a change to it that could only fail later, in an upload. An upload that the remote refuses for good stops, and waits for the user. This fills in choice 13 of [decision 0005](0005-mount-writes.md) for files, and replaces its choice 8 for a refused upload.

1. **One mark per file: read-only.** `Attributes::read_only` is true when the provider knows that the account may not give the file new bytes. A provider that does not know says false. A folder is never marked here: what a folder that may not take new items does is #639.
2. **The mode follows the mark.** On a mount that takes writes, a read-only file shows as mode 0400, on Linux and on macOS. Every other file keeps 0600.
3. **The core refuses what would go to the spool.** These fail with "access denied" (`EACCES`), and nothing is spooled:
   - opening a read-only file for writing, or cutting it;
   - the first write to a file that an app opened before the core learned the mark, when the answer that write fetches marks the file;
   - renaming a new file over a read-only file. By choice 9 of 0005, the new file's bytes would become that file's next bytes.

   The core decides from the answer the node holds, however old, as it already does for the kind. For a rename it uses the newest answer it holds about the file that would be replaced, which is also the answer that gives the upload its revision. A file whose bytes are already in the spool is not read-only: the spool is the file.

4. **Name changes still go to the provider.** Making a folder, removing an item, and renaming or moving an item on the remote each get the remote's answer at once (choice 1 of 0005). The core does not check the mark for them.
5. **An upload that the remote refuses stops.** When an upload fails with "access denied" or "not supported", the core keeps the bytes and does not try again by itself. The file shows as refused. These start a new try: the user's retry, a new edit, a new modified time, and a rename of the file. A close with no edit does not. The user can also discard the bytes. The refusal is not stored, so the next mount of the spool tries the file once again. Every other failure still follows choice 8: the core tries again after a wait that grows.
6. **Google Drive** marks a file read-only when `capabilities.canModifyContent` is false. A link file ([decision 0003](0003-gdrive-link-files.md)) is always read-only: it has no bytes on Drive to replace.

## Why

**A retry cannot change a refusal.** Under choice 8 alone, a file that the account may only view would fail every try in the same way, every 5 minutes, while the helper's status would say that the next try comes by itself.

**An app should hear "no" when it asks.** A save goes into the spool and returns before any upload (choice 5 of 0005). If the core takes the save, the app believes it worked, and the edit is stuck on this computer. When the mount knows ahead, the app gets "permission denied" at once, as it would for a read-only local file, and can offer to save a copy.

**The core checks, not only the kernel.** Linux mounts with `default_permissions`, so the kernel refuses to open a 0400 file for writing for a normal user. It does not refuse root on a mount that root made, and it allows a rename over the file, because a rename needs write permission on the folder. macOS does not check the mode on this mount at all, and an app that calls `access` hears yes, so on macOS the core's refusal is the only one. The core's own check gives the same answer on every OS and for every user.

**The answer the node holds.** An open for writing asks the provider nothing (choice 3 of 0005), and the kind already comes from the answer the node holds. A mark that changes after an app opened the file is caught at the first write when that write fetches a newer answer, and otherwise by the upload, which then stops.

**`canModifyContent`, not `canEdit`.** Google's reference says `canModifyContent` is whether the user "can modify the content of this file". It says that other things can limit `canEdit`.

**A close is not a reason to try again.** Some apps open a file for writing and close it without writing. Each close would cost one more refused try.

## Rejected

- **One mark per kind of change** (rename, trash, add, remove, edit), as Google Drive's capabilities have. Unix has one write bit per item, and a name change already gets the provider's answer at once.
- **Refusing a rename of a remote item over a read-only file.** Unix lets a user replace a file that the user may not write when the folder allows it. The provider decides.
- **Retrying a refusal, only more slowly.** It never lands, and the status keeps saying that it will.
- **Uploading the refused bytes as a copy beside the file.** The folder may refuse the copy too, and a file that nobody asked for appears in a shared folder. The user can still read the spooled bytes and decide.
- **Answering macOS's `access` call from the mode.** macOS asks `access` for `W_OK` before it renames or removes a file too, with nothing that tells the two apart. A read-only file could then not be renamed or trashed, and every Google Doc link file is read-only. macFUSE's FSKit backend ignores `default_permissions`, which would have had macOS apply the Unix rules itself (macFUSE 5.5.0).
- **Asking the provider at each open for writing.** Every save would wait for a round trip to Google, a quarter to half a second.
- **Storing the refusal in the spool's record.** The account's rights can change between two mounts. One try at the next mount costs one request.

## What follows

- The helper lists a refused file with the state `refused` (protocol 6), and `retryUnsent` and `discardUnsent` work for it. Showing it in the app is #377.
- A new local copy of a read-only file, made with `cp` or Finder, gets mode 0400, as a copy of any read-only file does.
- A provider that sets the mark needs no change in the core or the adapters.
- `rm` in a terminal asks before it removes a read-only file, as it does for any 0400 file. Every Google Doc link file is read-only.
- #639 decides the same for a folder.
