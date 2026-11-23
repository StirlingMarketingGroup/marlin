# 0006: An SMB mount is one saved server, served by its own `marlin-smb` processes

**Status:** accepted
**Date:** 2026-11-19
**Issue:** #612 (epic #368)

## Decision

1. **One mount per saved SMB server.** The server's shares are the mount's top folders.
2. **The mount is named `smb-<host>`**, from the host as the user saved it, built with `mount_name::build` like every other mount name. The host is the account the app sends in a `mount` request.
3. **The helper runs `marlin-smb` itself.** It starts its own `marlin-smb` processes as child processes, from the file beside the `marlin-mount` binary. It never uses the app's process. The app uses its own `marlin-smb` process for `smb://` browsing.
4. **One mount runs up to four `marlin-smb` processes**, started as calls need them. Each process runs one call at a time, so an SMB mount's `max_calls` is four. The number lives in one constant, and changing it does not need a new record.
5. **Every mount call has a time limit, and a call that passes it ends its process.** The process gets what is left of the mount's call limit (`Options::call_timeout`) when the call reaches it, because the mount core counts the wait for a free process too. So the process stops when the mount core gives up on the call. The call fails with `TimedOut`. The next call starts a new process. A process that dies is replaced the same way, and the call that was running fails.
6. **The helper reads the server's password from the keychain itself**, once per server, and keeps it in memory. After a failed sign-in, the next call reads the keychain again. The password reaches `marlin-smb` inside each request, over the pipe, as it does in the app today.
7. **When `marlin-smb` cannot start, the `mount` request fails and says why**, with the text the app shows today: the helper is missing, or Samba is not installed. Intel Macs ship no `marlin-smb` (see [licenses](../licenses.md)), so the app does not offer an SMB mount there.

## Why

**The app saves servers, not shares.** `smb-servers.json` holds a host, a user, and a domain, and the keychain entry is keyed by host. The app lists a server's shares live, with `smbclient -L`. A mount per share would need a saved list of shares that does not exist, and it would make several mounts out of one sign-in. The app's `smb://<server>` already shows the shares as folders, so the mount looks the same as what the user knows.

**The host alone names the account.** The app keeps one saved server per host, ignoring letter case. A user name in the mount name would rename the mount when the user edits the server's user.

**Samba stays out of the helper's binary.** Samba is GPL. Rule 2 of [`licenses.md`](../licenses.md) keeps GPL code out of the app and the helper, which is why `marlin-smb` is its own program. So the helper cannot link libsmbclient, and it talks to `marlin-smb` the way the app does.

**One process runs one call at a time.** `marlin-smb` answers one request at a time, and pavao holds one lock around every libsmbclient call, because libsmbclient keeps global state that is not safe across threads, even across connections. Calls that run at the same time need more than one process.

**Only a process can be stopped in the middle of a call.** A libsmbclient call that waits on a hung server cannot be cancelled from another thread. The mount core already gives up on a call after its limit, but the call goes on holding the process, and every later call waits behind it. Ending the process frees the mount.

**Four processes.** A slow read holds one process while Finder keeps listing folders on the others. `marlin-smb` opens a connection to the server for each call and closes it after, so a mount runs at most four SMB operations at once. Four is a guess: nobody has measured it. #543 measures mounts in Finder.

**The mount does not share the app's process.** The app runs downloads, uploads, and copies with no time limit, so a mount call could wait minutes behind one. The helper also outlives the app.

**The keychain rule is decision 0001's.** Secrets never cross the helper's socket, and each side reads the keychain. Reading once and keeping the password follows the Google Drive provider and #187, because macOS can ask the user at each read. macOS may also ask once for the helper, because it is a second binary with its own signature (first comment on #478, and #544, which signs the helper). That is untested.

## Rejected

- **One mount per share.** See above: there is no saved share, and one sign-in would make several mounts.
- **Linking libsmbclient into the helper.** GPL, see above. A missing Samba would also stop the helper from starting at all, and with it every Google Drive and SFTP mount.
- **Threads in one `marlin-smb` process.** Not safe in libsmbclient, and a hung call could not be stopped.
- **Sharing the app's `marlin-smb` process.** A mount call would wait behind the app's transfers, and the process would end with the app.
- **The app sends the password in the `mount` request.** That breaks decision 0001: secrets never cross the socket.
- **A pure-Rust SMB client in the helper.** It would remove the extra processes and the license limit. But the app's SMB code is built on libsmbclient, and two SMB stacks in one app could disagree on sign-in, protocol versions, and names. Not evaluated.

## What follows

- #612 moves the `marlin-smb` client and the saved servers into `marlin-smb-client`. The app links it, and the helper will. Its client runs one `marlin-smb` process, and the caller sets each call's time limit.
- #613 adds a byte-range read and a space call to `marlin-smb`.
- #614 builds the read-only `marlin_vfs::Provider` with up to four processes and the time limit.
- #478 opens the provider from the helper and finds `marlin-smb` beside `marlin-mount`.
- An SMB mount runs up to four SMB operations at once, plus one in the app's own process while the app runs.
- #377 moves the app's `smb://` browsing onto the mount, and the app's own process goes with it.
