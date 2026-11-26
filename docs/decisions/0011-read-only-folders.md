# 0011: A mount shows a folder it cannot add to as read-only

**Status:** accepted
**Date:** 2026-11-26
**Issue:** #639 (epic #368)

## Decision

A provider marks a folder read-only when it knows that the account may not add an item to it. The mount shows that folder without write permission, and the core refuses a new file in it before anything is spooled. This extends choice 1 of [decision 0008](0008-read-only-files.md) to folders.

1. **The same mark, read for a folder.** `Attributes::read_only` on a folder is true when the provider knows that the account may not add an item to it. A provider that does not know says false.
2. **The mode follows the mark.** On a mount that takes writes, a read-only folder shows as mode 0500, on Linux and on macOS. Every other folder keeps 0700.
3. **The core refuses a new file there.** Making a file in a read-only folder, and renaming or moving a new file into one, fail with "access denied" (`EACCES`). Nothing is spooled, and the new file stays where it was. The core decides from the answer the folder's node holds, however old, as it does for a file's mark. The same check first asks the provider's own rules (`check_new_item`), so one place decides whether a new file may go in a folder.
4. **A new file renamed over an item still takes its place.** It adds nothing to the folder: its bytes become the item's next bytes (choice 9 of [decision 0005](0005-mount-writes.md)). The item's own mark decides, as choice 3 of 0008 says.
5. **Name changes still go to the provider.** Making a folder, moving a remote item in, removing, and renaming get the remote's answer at once (choice 4 of 0008). The core does not check the folder's mark for them.
6. **Finder's own files are still taken.** `.DS_Store` and `._name` go into a read-only folder's spool and never upload, as on an SMB server's list of shares (#769). Renaming one in that folder to a free name that uploads fails like a new file.
7. **A folder with no answer is asked once.** Every folder's node but the root's holds an answer from a listing, a lookup, or a change. When the root holds none, the core asks the provider for it before the check, and keeps it.
8. **Google Drive** marks a folder read-only when its `capabilities.canAddChildren` is false, and the top folder of a shared drive when the drive's `capabilities.canAddChildren` is false. The top of the mount and `Shared drives` are always read-only: they are not Drive folders, and nothing new can go in them.

## Why

**Without the mark, a save into a view-only folder looks done.** The new file goes into the spool, the app hears that the save worked, and the upload is refused seconds later. The bytes stay on this computer. A Finder copy into a view-only shared drive looks done the same way. When the mount knows ahead, the app hears "permission denied" at once and can offer to save elsewhere.

**Only the remote knows a permission, and the core already holds its answer.** `check_new_item` answers from rules the provider knows without asking the remote, such as a name SMB cannot hold. A folder the account may not add to is a fact about the account on the remote. The folder's node already holds the provider's answer from the listing that showed it, so the mark costs no request.

**The answer the node holds, however old.** Asking the provider at each new file would make every save wait for a round trip, a quarter to half a second on Google Drive. A folder that loses the right after the core's last answer is caught by the upload, which then stops and waits for the user (choice 5 of 0008). A folder that gains the right takes new files once the core holds a newer answer: the next listing of the folder above it, or the OS asking for its attributes, brings one.

**The kernel's refusal fits Drive's roles.** Linux mounts with `default_permissions`, so the kernel refuses making, removing, and renaming an item in a 0500 folder for a normal user, before the core sees the call. Google Drive keeps adding (`canAddChildren`) apart from removing (`canRemoveChildren`, `canTrashChildren`, `canDeleteChildren`) and renaming (`canRename` on the item). By Google's role tables, no role may remove from a folder that it may not add to: a viewer and a commenter may do neither. A contributor of a shared drive may add but not trash, gets 0700, and Drive refuses the trash at once. One case is lost on Linux: a file shared with the account for editing, inside a folder the account may only view. Drive lets the account rename that file, and the kernel refuses for a normal user. So an app that saves that file by writing a new file and renaming it over the old one is refused there, while an app that writes the file in place succeeds. Unix has one write bit per folder, and 0008 rejects one mark per kind of change. macOS checks no modes on this mount, so there a rename of that file goes to Drive. A save that makes its new file in that folder fails on both systems, because the folder takes no new item. A save that makes its new file in a folder the account may add to, and renames it over the file, works on macOS.

**Finder's own files never reach the remote.** The remote's rules do not apply to them, as on an SMB server's list of shares (#769). Refusing them would only break Finder's view settings in every view-only folder for the session.

**The root.** The core holds no answer for the root until something asks. The OS usually asks for the root's attributes before it makes anything there, so the extra request seldom runs. Without it, a mount whose top is a folder the account may not write in, such as an SFTP account's top folder (#784), could take a new file that fails only at upload.

## Rejected

- **Answering it in `check_new_item`.** That call makes no remote request. The Google Drive provider keeps nothing between calls, so it would have to ask Google at each new file.
- **A provider call that asks the remote at each new file.** A round trip per save, as above.
- **Refusing Finder's own files in a read-only folder.** They never upload, and Finder would lose its view settings there.
- **Refusing a new folder from the mark in the core.** The provider answers it at once, with the remote's own reason (choice 4 of 0008).
- **Showing the folder as writable and refusing in the core only.** A file manager would offer to paste into a folder that takes nothing, and on Linux a normal user's create would reach the core only to fail there.
- **Treating a folder with no answer as writable.** It leaves the gap open for the root.

## What follows

- A provider that sets the mark on a folder needs no change in the core or the adapters. #784 does this for SMB and SFTP.
- A folder that only the mount shows, such as `Shared drives` or the planned "Shared with me" folder (#701), is marked read-only where the provider makes it up (`made_up_folder` in the Google Drive provider), beside the refusal in `destination`.
- Not checked against a real Google account: `canAddChildren` on a view-only folder and on a shared drive, and the role tables above. The fake server's tests cover the requests and the marks.
- Not checked: whether Finder draws a "no write" badge on a 0500 folder of this mount.
