# 0009: Marlin's window works on mount paths and stores provider addresses

**Status:** accepted
**Date:** 2026-11-24
**Issue:** #377 (epic #368), recorded in #676

This record replaces two parts of [0001](0001-mount-architecture.md), as far as they concern Marlin's own window: the end of the "Rename" row in "What a mount promises on every OS", which left Marlin's own move to #377, and risk 3, which left Macs older than macOS 15.4 to #377. The rest of 0001 stands, with the changes that [0002](0002-node-ids-follow-lookup-counts.md) and [0005](0005-mount-writes.md) made to it. A mount still fails a rename across two mounts with a "cross-device" error. Choice 8 says what Marlin's window does about it.

## Decision

Marlin's window opens a remote at its mount path, the same path every other app sees. Eleven choices make that up. Brian answered four questions on 2026-11-24 and chose option A of each: choices 3, 4, 5, and 6. The options he turned down are under "Rejected".

1. **A remote folder's place in the window is its mount path.** Every file command gets the mount path, so Marlin runs its local code for every remote: listing, open, open with, copy, paste, drag, the OS clipboard, rename, delete, archives, and the folder watcher. Once a provider has moved, its addresses no longer reach a file command: they resolve to the mount path first. A provider that has not moved keeps its own code until it does (choice 11).
2. **Provider addresses stay, as input and as the stored form.**
   - A `gdrive://`, `sftp://`, or `smb://` address that is typed, pasted, pinned, restored as the last folder at launch, or handed to a new window, a pasted Google Drive folder link, and, on macOS and Linux, `\\server\share` open the folder at its mount path. When the account is not mounted, Marlin mounts it first.
   - One resolver does this in both directions (#689).
   - What Marlin writes to disk stores an address, never a mount path: pins, the last folder, and the view settings of each folder. An archive inside a remote stores the remote's address as its source. So a change of the mount folder breaks nothing.
   - A Google Drive item is stored by its Drive file ID (`gdrive://<email>/id/<id>`), because a name in a mount can change. That is Google's own ID, not the mount's item ID ([Google Drive mount provider](../mount-gdrive.md#item-ids)). The account's top (`gdrive://<email>/`), the top folders the mount makes (`My Drive`, `Shared drives`, `Shared with me`), and an item that has not reached Drive yet are stored by name.
   - A name address saved by an older version resolves through the names the mount shows, and view settings saved under it are still found. An address that points into `Starred` or `Recent`, or to a place the mount does not have, opens nothing and says why.
   - History lives in memory only, and holds mount paths.
3. **The path bar shows the mount path** (question 1). My Drive › Projects of `ana@example.com` shows as `<mount folder>/gdrive-ana@example.com/My Drive/Projects`, the path Terminal and every other app uses. Decision 0001 (choice 5) and #541 set the mount folder. Typing either the mount path or the provider address opens the folder.
4. **Every saved account mounts when Marlin opens** (question 2), so Finder and other apps see it while Marlin runs. An SMB server on an Intel Mac is an exception: that build has no `marlin-smb`, so Marlin offers no SMB mount there ([decision 0006](0006-smb-mounts.md), choice 7).
   - Each account's row in the sidebar shows a mark while its mount is starting, is offline or failing (`degraded`), needs a sign-in, or failed, with the reason on hover. #687 maps the helper's states ([Mount helper](../mount-helper.md#mounts)) to these marks.
   - A row whose mount is up (`mounted`, `degraded`, `offline`, or `auth-required`) has an eject button, which unmounts it. Opening an ejected or failed account tries to mount it (choice 2), and so does the next launch.
   - Preferences shows each account's mount too, with the same state, and Mount or Eject as fits (#688).
   - With no saved account, Marlin starts no helper. Every mount command can start one, `list_mounts` too (first comment on #377), so #684 decides when the window first lists mounts.
   - Quitting Marlin asks the helper to unmount every mount (`stop_at_quit` in `src-tauri/src/mount_helper.rs`). The helper waits until no file is waiting or uploading, then unmounts, and keeps serving a mount it could not unmount ([Mount helper](../mount-helper.md#the-app-side), "When the user quits").
5. **A computer that cannot mount opens no saved account, and says why** (question 3). Linux needs both `/dev/fuse` and `fusermount3` (from the `fuse3` package). A Mac needs macOS 15.4 or later, and macFUSE 5 or later with both of its File System Extensions switches on.
   - The account's row shows the text the helper sends with the failed mount. For a missing need above, that text names the fix, and the code is -1006 ([Linux mount adapter](../mount-linux.md#what-the-machine-needs), [macOS mount adapter](../mount-macos.md#what-the-machine-needs)). On a Mac the text about the switches is a best guess: the adapter gives it for every mount that ends before its volume appears. Opening the account tries the mount again, and shows the same text when it fails again.
   - On Windows every mount command fails with "Marlin cannot mount on this OS yet.", which the app sends with no code, until the Windows adapter (#375). A Windows share still opens there as a native `\\server\share` path.
   - Marlin keeps macOS 10.13 as its minimum, so the rest of the app still runs there.
   - No fallback is kept: choice 11 says when each provider's old code goes.
6. **The Drive mount gets a `Shared with me` folder, and `Starred` and `Recent` go away** (question 4). `Shared with me` is added before Drive moves (#701). A Drive link to an item that has no place in the mount says so.
7. **Provider actions run in the helper.** A filesystem has no call for some things a remote can do: a copy inside one account that keeps a Google Doc a Doc and moves no bytes, a thumbnail the remote already made, an export. The mount core gets optional provider calls for them.
   - The core decides, in one place and before anything changes, whether an item may use a provider call. An item may only when it is on the remote with no unsent changes in the spool. A link file can qualify. A call that writes also needs a mount and a destination folder that take writes. A folder may use one only when every item in it may. The provider can still refuse, for example a file the account may not copy (#681).
   - When the core says no, the helper answers "not eligible". When the call has nothing to give, such as a file Drive made no thumbnail for, it answers none. Either way the app does the plain filesystem work. A provider call that fails after it started is an error, not a "not eligible".
   - These calls are requests on the helper's socket, so they keep choice 4 of [0001](0001-mount-architecture.md): no file contents cross it. #683 sets how a thumbnail reaches the app inside that rule.
   - The app never calls a remote's API for an item in a mount, so the mount's caches and spool stay right.
8. **One rule decides how a transfer runs** (#678).
   - A move inside one filesystem is a rename. What a move does when a mount refuses that rename as not supported is under "Not settled here".
   - A copy inside one mount is a provider copy when the helper allows it (#682).
   - Every other transfer is a filesystem copy. A move by copy deletes the source only after the whole copy is in place under its final names. A Replace removes the old item only after the new one is in place (#679).
   - For a move into a mount, the copy is in place once every file has been written and closed, and the whole copy has its final names. A name the mount never uploads is the exception (see "Not settled here"). The move does not wait for the upload: `close` returns when the bytes are in the spool, the spool keeps them through a helper crash, and an upload the remote refuses waits in the unsent list ([decision 0005](0005-mount-writes.md), choices 5 and 10; [decision 0008](0008-read-only-files.md)).
   - A move out of a Drive account never trashes what a link file stands for: a Google Doc, Sheet, or other editor file, or a shortcut ([decision 0003](0003-gdrive-link-files.md)). At any depth, a link file is copied and stays in the source, with the folders that hold it, and Marlin says so (#682). This holds for every destination, an SFTP or SMB server that still uses its own paste code included: those paste branches delete a cut file's source after the upload (`paste_items_to_location` in `commands.rs`).
9. **A network or FUSE folder is remote to Marlin.** Marlin lists it without opening each subfolder or reading each image (#677). Move to Trash there never makes a trash folder on the remote. What it does instead follows Finder (#686).
10. **The window shows unsent files** (#684). A failed or refused file has a retry, and a discard that asks first, because it drops the user's change for good. An unplaced file shows where it is on the disk. Quitting asks first when files would stay on this computer: failed, refused, or unplaced ones. Files that wait or upload need no question. The helper waits for them after Marlin quits, and a file whose upload then fails stays in the spool for the next mount (#685).
11. **The old browsing code goes provider by provider**, once that provider's mount reads and writes: Google Drive first (#691), then SFTP (#694, after #603), then SMB (#698, after #604). These stay: the sign-in and connect windows, the saved accounts and servers, SFTP host-key trust, and the Google Drive link parser.

## Why

**One code path for every app.** Decision 0001 mounts remotes so that every app sees one folder. If Marlin's window kept its own provider code, every file operation would have two code paths, and the eight breaks in [Location providers](../location-providers.md#where-the-model-breaks) would stay. One of them shows today: an editor opened from an `sftp://` folder edits a temporary copy, and its save never reaches the server (Brian, v0.1.28, in a comment on #377). On a mount path the editor saves into the mount, and the mount uploads.

**A mount path is a poor stored form.** The mount folder can change: the fix for #541 (PR #703, open) gives a dev build a folder of its own. A name in a mount can change too. When items in one folder share a name, the core takes them in item-ID order: each keeps the name if it is free, or gets the first free ` (2)`, ` (3)` before the extension. It works this out again at each listing, so adding or removing a duplicate can move a suffix to another item (`unique_names` in `src-tauri/crates/marlin-vfs/src/names.rs`). A Drive file keeps its ID when it is renamed ([Google Drive mount provider](../mount-gdrive.md#item-ids)), and the old view already opens a Drive folder by ID (`/id/<id>` in `src-tauri/src/locations/gdrive/provider.rs`).

**Saved addresses must keep working.** Marlin saves pins in `pinned_directories.json`, and the last folder and each folder's view settings in `preferences.json` (`lastDir` and `directoryPreferences`), all as address strings (`src-tauri/src/commands.rs`). People paste Drive links (`parseGoogleDriveUrl` in `src/utils/googleDriveUrl.ts`, then the `resolve_gdrive_folder_url` command) and type `\\server\share` (`resolveTypedPath` in `src/utils/pathUtils.ts`). History is never saved (`pathHistory` in `src/store/useAppStore.ts`), so it can hold mount paths.

**The path bar (choice 3).** A path copied from the bar then works in Terminal, in a file dialog, and in every other app.

**Mounts at launch (choice 4).** Other apps see an account only while it is mounted, and a mount takes time to come up. The helper opens the account and makes one call to the remote first ([Mount helper](../mount-helper.md#providers), "The first call"). On macOS the adapter then waits for the volume, which takes one to two seconds ([macOS mount adapter](../mount-macos.md#how-a-mount-behaves)).

**No fallback (choice 5).** A fallback would keep both code paths for every feature, with today's limits on the computers that use it. Raising the minimum to macOS 15.4 would take the whole app from older Macs, when only remotes need the mount. Marlin's macOS minimum is Tauri's default, 10.13: `src-tauri/tauri.conf.json` sets none, and `.github/workflows/release.yml` sets `MACOSX_DEPLOYMENT_TARGET` to match. The release builds are for macOS and Linux only (`release.yml`), so no Windows user of a release loses remotes. The `deb` and `rpm` packages already require `fuse3` (`tauri.conf.json`). The AppImage declares no `fuse3` dependency, so an AppImage user can start Marlin on a computer that cannot mount, and gets the text of choice 5.

**Shared with me (choice 6).** Today Marlin's Drive view shows `My Drive`, `Shared drives`, `Shared with me`, `Starred`, and `Recent` (`list_virtual_root` in `provider.rs`), and a pasted link to a shared folder opens it. The mount shows only `My Drive` and `Shared drives` ([Google Drive mount provider](../mount-gdrive.md#what-a-mount-shows)). Without `Shared with me`, a folder shared with the account that is in neither My Drive nor a shared drive the account can see would open only in the browser.

**Provider actions in the helper (choice 7).** Only the core knows whether a file's newest bytes are still in the spool, and what its caches hold. If the app asked Drive to copy a file whose edit had not uploaded, Drive would copy the old bytes. If the app renamed an item through Drive's API, the mount would show the old name until its cache expired.

**One rule for transfers (choice 8).** Marlin ends a local cut and paste in `fs::rename`, also when it moves entries during a folder merge, with no fallback (`rename_file_or_directory` in `src-tauri/src/fs_utils.rs`, and `merge_local_directory` in `commands.rs`). A rename between two filesystems fails with "cross-device", and once remotes are mounts, every cut and paste between a mount and the disk is such a move. Removing a link file puts its Google file in the trash ([Google Drive mount provider](../mount-gdrive.md#link-files)), so a plain copy and delete would trash the Google file.

**Remote folders (choice 9).** After a listing, Marlin looks for `.git` in each subfolder, counts what each subfolder holds, and reads the header of each image of a known type: `build_file_metadata` in `fs_utils.rs` does this for the window's streaming listing, and `build_file_item` does the same for the `file` provider. In a Drive mount, counting a subfolder lists it, so a folder with 200 subfolders asks the mount for 200 listings just to show itself. How many Google requests that makes depends on caches and pages (#677).

**Unsent files (choice 10).** A save into a mount returns before the upload ([decision 0005](0005-mount-writes.md), choice 5). Only the window can tell the user that a file has not arrived yet.

**Provider by provider (choice 11).** Marlin writes to Google Drive, SFTP, and SMB today, so a provider cannot move before its mount takes writes. The Google Drive mount takes writes. SFTP and SMB mounts are not in the helper yet (#477, #478), and take no writes yet (#603, #604).

**Untested.** None of these choices is built yet.

- On macOS, mounts have run only on macOS 27.2 on Apple Silicon, with macFUSE 5.5.0. An Intel Mac and macOS 15.4 to 26 are not tested. A Mac without macFUSE or with its switches off, and a mount started from the app, wait for #543 ([macOS mount adapter](../mount-macos.md)).
- On Linux, mounts have run only in a Debian 12 container. A bare Linux desktop, Flatpak, Snap, and the AppImage are not tested (#486, [Linux mount adapter](../mount-linux.md)).
- Whether the folder watcher sees changes inside a mount is not tested on either OS.

## Rejected

Brian's four questions, answered on 2026-11-24. Option A of each is the choice above.

- **Question 1, path bar, B: show `gdrive://ana@example.com/My Drive/Projects`, as today.** That text works only in Marlin. Typing it still opens the folder (choice 3).
- **Question 2, mounting, B: nothing mounts until the user opens the account.** Other apps do not see the account until then, and the first open waits for the mount to come up.
- **Question 2, mounting, C: choice 4, plus a "Mount when Marlin opens" switch per account in Preferences.** Brian chose A over it. Adding the switch later would change choice 4, so it needs a new record.
- **Question 3, no mount, B: fall back to today's code where a mount is not possible.** Both code paths would stay for every feature, and there an editor would still save to a temporary copy.
- **Question 3, no mount, C: raise Marlin's minimum to macOS 15.4.** The whole app would leave older Macs, when only remotes need the mount. Linux without FUSE would still need choice 5.
- **Question 4, Shared with me, B: drop all three virtual folders.** A shared folder that is in neither My Drive nor a shared drive the account can see would open only in the browser.
- **Question 4, Shared with me, C: keep all three.** A folder in `Starred` or `Recent` also sits in its own parent folder, so the mount would reach it through two parents, which [decision 0003](0003-gdrive-link-files.md) does not allow.

Other routes:

- **Store mount paths.** They change when the mount folder or a ` (2)` changes (see "Why").
- **Store a Drive item by its name.** A ` (2)` can move to another item.
- **The app calls a remote's API for provider actions.** The mount's caches would go stale, and a copy could send old bytes (see "Why").
- **A move into a mount deletes the source only after the upload.** The move would take as long as the upload, though the spool already keeps the bytes through a helper crash.
- **Move a Drive link file by copy and delete.** The delete trashes the Google file or the shortcut.

## What follows

**The work.** Each piece of #377 builds part of this record. #377 lists their order and what blocks each one.

| Choice | Built by                                                                                                                         |
| ------ | -------------------------------------------------------------------------------------------------------------------------------- |
| 1, 3   | #690 (Google Drive), #693 (SFTP), #696 (SMB)                                                                                     |
| 2      | #689, with #680 (the helper maps an item to its path) and #701                                                                   |
| 4, 5   | #687 (Google accounts in the sidebar), #688 (Preferences, for every kind of account), #692 (SFTP), #695 (SMB)                    |
| 6      | #701                                                                                                                             |
| 7      | #681 (the provider copy in the core, the helper, and the Drive provider), #682 (Marlin's paste uses it), #683 (Drive thumbnails) |
| 8      | #678, #679, #682                                                                                                                 |
| 9      | #677, #686                                                                                                                       |
| 10     | #684, #685                                                                                                                       |
| 11     | #691, #694, #698                                                                                                                 |

**Easier after this.** A new provider gets the whole window by mounting: no new pair in `paste_items_to_location`, no download command, no thumbnail branch ([Location providers](../location-providers.md#adding-a-provider-today)). An editor's save reaches the remote. Marlin and every other app show the same files.

**Harder after this.**

- Saved accounts need macFUSE and macOS 15.4 or later on a Mac, and FUSE on Linux. On Windows, Marlin's own mounts wait for the Windows adapter (#375).
- On macOS and Linux, every user with a saved account that can mount runs the helper while Marlin runs (choice 4).
- The Google service account (`GOOGLE_SERVICE_ACCOUNT_KEY_FILE`) is not saved, has no mount name, and the helper cannot sign in as it (`GoogleAccountInfo` in `src-tauri/src/locations/gdrive/auth.rs`), so it cannot mount. #691 removes it, with the manual live tests in `src-tauri/tests/gdrive_integration.rs` that use it.
- Quitting one Marlin process stops the helper, and so the mounts, under another one (#508). With mounts at launch, that happens more often.
- A move into a mount can finish while files still wait to upload. The unsent list is how the user learns that a file has not arrived.
- A mount gets no change notices from the remote ([0001](0001-mount-architecture.md), "Change notices"). A change made on another computer shows when Marlin lists the folder again and no cache holds the old answer. A refresh drops the core's listings (#680). The kernel also keeps a name and its attributes for 1 second, on Linux and macOS (`TTL` in `src-tauri/crates/marlin-fuse/src/lib.rs`). When macOS drops a file's cached bytes after the remote changed is not tested ([macOS mount adapter](../mount-macos.md)).

**Not settled here.**

- Which shared items `Shared with me` lists, so that no folder has two places. #701 writes that in a decision record of its own.
- Whether Marlin also mounts an account again by itself after its mount failed at launch, for example with no network (-1014). Opening the account tries again (choice 2). #687 decides the rest.
- What a move does when a mount refuses a rename inside it as not supported: a Drive folder moved between My Drive and a shared drive, or a duplicate link file moved under its shown name ([Google Drive mount provider](../mount-gdrive.md#the-three-change-calls)). #678 decides. If it copies, the link-file rule of choice 8 holds there too.
- How SFTP and SMB addresses map to mount paths. #693 sets that for an SFTP address, its port included. An SMB mount is one saved host with its shares as the top folders ([decision 0006](0006-smb-mounts.md)), and #696 sets how a share and a path map to the names the mount shows. A saved SFTP server and a saved SMB server store no mount name yet (the note from #477 on #377).
- Whether a move by copy syncs the copy to the disk before it deletes the source (#678). Choice 8 covers a helper crash, not a power loss.
- What a move into a mount does with a file whose name the mount keeps only on this computer and never uploads: `.DS_Store` and any `._name` ([decision 0005](0005-mount-writes.md), choice 14). Such a copy is not one the spool keeps, so it does not let the move delete the source. #678 decides what the move does instead.
- An export, such as "Download as PDF". No piece adds one yet. When one does, it is a provider call under choice 7.
- A setting for the spool's size limit, if 10 GiB proves too small ([decision 0005](0005-mount-writes.md), risk 5).
