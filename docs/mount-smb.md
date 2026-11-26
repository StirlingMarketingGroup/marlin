# SMB mount provider (`marlin-smb-client`)

`SmbProvider` shows one saved SMB server to the [mount core](mount-core.md) as a `Provider`. The server's shares are the top folders. It takes writes: `can_write` and `ids_are_paths` are true, so a mount of it takes writes and an item's id is its path. [Decision 0006](decisions/0006-smb-mounts.md) says what an SMB mount is, how many `marlin-smb` processes it runs for calls, and where it gets the password. [Decision 0010](decisions/0010-smb-uploads-run-on-their-own-processes.md) adds the processes the byte calls run on. The [mount helper](mount-helper.md#providers) opens it as the provider `smb`.

This is not the code behind `smb://` paths. The app handles those through its own location provider and its own `marlin-smb` process: see [Location providers](location-providers.md).

**Covers:** `src-tauri/crates/marlin-smb-client/src/provider.rs`, `src-tauri/crates/marlin-smb-client/tests/provider.rs`, `src-tauri/crates/marlin-smb/tests/provider.rs`
**Checked against the code:** 2026-11-26

## The parts

| File                                  | Holds                                                                                                                                                                                                                 |
| ------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `marlin-smb-client/src/provider.rs`   | `SmbProvider` and its calls, the item IDs, the two process pools, and `SmbSignIn`.                                                                                                                                    |
| `marlin-smb-client/src/sidecar.rs`    | `Sidecar`, the client for one `marlin-smb` process. A call through `call_until` ends when a stop flag is set: before its request is sent it sends nothing, and while it waits for the answer it ends the process too. |
| `marlin-smb-client/src/servers.rs`    | The saved servers and their keychain entries: see [Location providers](location-providers.md).                                                                                                                        |
| `marlin-smb-client/tests/provider.rs` | The provider's tests against fake `marlin-smb` scripts. They need no Samba, and CI runs them.                                                                                                                         |
| `marlin-smb/tests/provider.rs`        | The provider's tests against a real Samba server. CI does not run them.                                                                                                                                               |

A caller gives the provider two things:

- **`SmbSignIn`**, the server's sign-in. Every provider of one server gets the same one: see [The sign-in](#the-sign-in).
- **The `marlin-smb` file** to run, or none when it is missing. Every call that needs the helper then fails with the app's own text, "SMB support not available. The marlin-smb helper is missing.", and reads no keychain.

## What a mount shows

- **The top folder holds one folder per share** that `list_shares` reports. A share that is not a disk share, such as `IPC$` or a printer, is not shown, and neither is a share whose name ends in `$`, such as `C$` or `ADMIN$`. `marlin-smb` leaves them out, as the app's `smb://` view does. So the mount cannot reach an admin share.
- **Names that start with a dot are shown.** `marlin-smb` marks them hidden, and the provider ignores the mark. The OS hides them itself, as it does on a local disk.
- **A share's folder has the modified time 1970-01-01.** `list_shares` gives no time, and a listed item must agree with the same item asked for alone.
- A folder's size is 0.
- Every file's `read_only` mark is false. A file's revision is its size and its modified time in whole seconds since 1970, written `size:seconds`. A folder's revision is unset. The seconds are the same ones `FileStamp` compares. The change time is not used: on Samba it is the POSIX ctime, and the rename that publishes an upload moves it.

## Item IDs

| ID                | What it is                                        |
| ----------------- | ------------------------------------------------- |
| `/`               | The server: the top of the mount.                 |
| `/{share}`        | A share.                                          |
| `/{share}/{path}` | An item in a share, such as `/data/photos/a.jpg`. |

An SMB name never holds `/`, so an ID splits one way only. `marlin-smb` gets the share and the path from `/` inside it: `/data` is share `data` and path `/`, as the app sends it.

The ID is the path. So a rename that another client makes looks like one item removed and another added: see [Mount core](mount-core.md#what-a-provider-implements).

## The five calls

| Call    | Requests to `marlin-smb`                                                                                                                                              |
| ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `root`  | None.                                                                                                                                                                 |
| `item`  | `list_shares` for `/` and for a share. `get_file_metadata` for anything else.                                                                                         |
| `list`  | `list_shares` for `/`. `read_directory` for anything else.                                                                                                            |
| `read`  | `read`, 16 MiB at most per request, until the bytes asked for arrived or the file ended. None for 0 bytes, and none for the server or a share, which are `IsAFolder`. |
| `space` | `list_shares`, then `space` for each share in order, until one share reports a size.                                                                                  |

- **A share is what `list_shares` says**, in `item` as in the top folder's listing. So a share the user may not open is a folder whose listing is refused, and a share the server does not list is `NotFound`.
- **`item` for the top asks the server.** The mount core asks `item` for the top folder's attributes when the OS asks for them and its last answer is more than a few seconds old. The helper reads the result of every call as news about the server's [health](mount-helper.md#health). An answer made up without the server would show an offline mount as up. The Google Drive provider still makes that answer up: #647.
- **A long read is split.** `marlin-smb` takes at most 16 MiB per `read` (`READ_MAX`). The pieces run one after another on one process, inside one call's limit. A piece that comes back short is the end of the file. Below about 0.5 MB/s, one 16 MiB piece takes longer than the 30-second call limit, and the read fails with `TimedOut`.
- **Free space is the first share's that reports a size.** A mount reports one size for all its shares. Adding them up would count a disk that several shares sit on more than once. A share that reports no size or cannot answer, such as one the user may not open, is skipped. A refused sign-in, or a server out of reach, fails the call. When no share reports a size, the provider answers 1 PiB free, as the Google Drive provider does, because a mount that reports 0 free makes file managers refuse to copy.

## Changes and uploads

A name that is empty, `.`, `..`, or that holds `/`, `\`, `:`, `*`, `?`, `"`, `<`, `>`, `|`, a control character (below U+0020), or U+FFFD is `NotSupported` before any request. SMB cannot hold these, and Samba answers some of them with a generic error, which would count against the whole mount. Every call that takes a new name checks it first.

The three change calls run on the call processes. The two byte calls run on the upload processes. A call that removes or overwrites uses the id the core gave, never a path rebuilt from a name: the core's bookkeeping removes exactly those ids. SMB has no trash.

Bytes waiting to upload go up under the account saved for the server at the time of the upload. Every SMB request reads the saved account ([`SmbSignIn`](#the-sign-in)).

- **`create_folder`** on the share list (`/`) is `NotSupported`: a mount cannot make a share. Anywhere else it sends `create_directory` for the new path, then `get_file_metadata`, and returns that item. A taken name answers `SMB_ALREADY_EXISTS`, which is `Conflict`.
- **`remove`** of the server or of a share is `NotSupported`. An id whose parent is not the folder the core names is `NotFound`. Otherwise it sends `delete`. A missing item is `NotFound`. A folder that holds something is `NotEmpty`.
- **`rename`** needs an entry whose parent is the folder the core names. Anything else is `NotFound` for an entry in another folder, and `NotSupported` for a share or the server. The destination must be the same share, its top or a folder in it. The server, or another share, is `NotSupported`, because `marlin-smb` renames inside one share. A folder moved into itself is `NotSupported`. With nothing to replace, `rename` uses `replace: false`, so a taken name is `Conflict`. With an item to replace, that item's parent must be the destination folder, or the call is `NotFound`. `get_file_metadata` of it runs first, so a target that is gone is `NotFound` and nothing is renamed. The rename then lands on that item's own path with `replace: true`, and the result's id is that path: on a share that ignores case, the replaced item can be spelled differently from the requested name. With nothing to replace, the new path is the new id. Either way the call reads the item back with `get_file_metadata`.
- **`create_file`** on the share list is `NotSupported`. It looks the target up first, so the sign-in is settled by a short call: an untried sign-in holds `SmbSignIn`'s lock for the whole call that sends it, and that call must not be the upload. A file that is already there is `Conflict`, and nothing is sent. The bytes then go to `.marlin-{pid}-{nanos}-{n}` in the target's own folder, a new name for each call, and `save_file` publishes them with a rename. A name taken by then answers `SMB_ALREADY_EXISTS`, which is `Conflict`.
- **`replace_bytes`** of the server or of a share, or of a folder, is `IsAFolder`. It looks the file up first, for the same sign-in reason. A revision other than the one the core sent is `Conflict`, and nothing is sent. `SMB_CHANGED`, the file changed during the upload, is `Conflict`. `SMB_PERMISSION_DENIED` stays `AccessDenied`, which [decision 0008](decisions/0008-read-only-files.md) treats as refused. `marlin-smb` answers that when it cannot give the new file the old access, and when libsmbclient before 4.23 is asked to replace a file on a server that is not on port 445 ([Location providers](location-providers.md)).

The modified time sent with the bytes is RFC 3339. A server may refuse to set it, so the item's revision comes from the answer.

From the `staged` notice on, this upload owns the temporary file. On any error after that notice, including a call the core stopped, the same worker removes it before it lets its process go. The removal uses a fresh stop flag, because the call's own flag is already set and would send nothing, and a 30-second limit, so a dead network cannot hold an upload process forever. A removal that fails is remembered, and the next upload tries it again before its own `save_file`. The memory lasts only as long as the provider: a file still there when the provider ends stays on the server. A process stopped after it made the temporary file but before the `staged` notice arrived leaves a file the provider never heard of.

## Processes and the call limit

- **One mount runs up to 4 `marlin-smb` processes for its calls** (`PROCESSES`) **and up to 4 more for the two byte calls** (`UPLOAD_PROCESSES`) ([decision 0010](decisions/0010-smb-uploads-run-on-their-own-processes.md)). Each process runs one call at a time. `PROCESSES` is the mount's `max_calls`, and `UPLOAD_PROCESSES` is its `max_uploads`. The core never starts an upload that would wait for a process: an upload that waits sends no progress, and the core ends it as stalled, so the upload pool is not smaller than `max_uploads`. A call holds one process from start to end, and only that call uses it. A process starts when a call first needs it and stays up for the next call. A slow upload never holds a process a list or a read needs. An upload has no time limit. The core drops it when progress stops, and that ends its process.
- **The provider keeps no clock.** The core limits each call to `call_timeout`, counting the wait for a free slot, and drops the call when the limit passes ([Mount core](mount-core.md#rules-the-core-keeps)). Dropping the call sets its stop flag. While `Sidecar::call_until` waits for an answer, it checks the flag at least every 50 ms and ends the process group. A request not yet sent is never sent, and its process stays. So the process ends when the core gives up, as choice 5 of decision 0006 asks, and the next call starts a new one.
- **A call the core gave up on while it waited for a free process** never starts.
- **A call starts its process before it reads the keychain**, so a missing `marlin-smb`, or a `marlin-smb` that cannot load Samba, says so first. A missing `smbclient` program shows only when `list_shares`, `rename`, or `save_file` runs it. A keychain read cannot be stopped: a call that waits for one, for example behind a macOS prompt, holds its process until the read ends.
- **A process that dies during a call** fails that call. The next call starts a new process.

## The sign-in

`SmbSignIn` holds one server's sign-in in one process. It reads the saved server from `smb-servers.json` and the password from the keychain ([Location providers](location-providers.md)). It is in one of four states:

| State    | Means                                                          |
| -------- | -------------------------------------------------------------- |
| Unread   | Nothing read yet.                                              |
| Untried  | Read, and the server has not yet answered a call that sent it. |
| Accepted | The server took it.                                            |
| Refused  | The server refused it. It is never sent again.                 |

- **It reads the keychain itself** (`read_password`), not the copy of the password that `servers.rs` keeps for the app. That copy could be one the server refused. Its lock is also held across a read for any server, so a macOS prompt for one server would hold up the others. One lock per server is held across the read, so calls that arrive together share one read. macOS can ask the user at each read.
- **An untried sign-in goes out on one call at a time.** The other calls wait for its answer. So four calls with a wrong new password cost one refusal, not four.
- **An accepted sign-in goes out on every call at once.** When the password changes on the server, up to 8 calls in flight, the call processes and the upload processes, can each be refused once.
- **A refused sign-in is never sent again.** Each later call reads the saved server and the keychain again, the calls that waited for the refusal included. When they still hold the refused user name, password, and domain, the call fails with `SignInNeeded` and sends nothing. A refused share call costs at least two sign-ins: one that gets `EACCES` runs `smbclient -L` to learn who refused, and a folder listing stats the folder first. A domain with a lockout threshold counts every try. A new password, user name, or domain is tried, so the mount recovers with no unmount.
- **Give every provider of one server the same `SmbSignIn`, for the whole run of the process**, as the helper keeps one `SavedSignIn` per Google account. The first call of a mount is `space`. When it is refused, the mount fails and its provider goes away, but the refusal stays with the server's `SmbSignIn`. The helper keeps one per server.
- **Every request to `marlin-smb` reads `smb-servers.json` first.** When the user name or domain saved for the server is not the one the sign-in holds, the request reads the sign-in again: the user may have switched accounts while the old one still works on the server. An accepted or untried sign-in of the same account keeps its password, so the request reads no keychain.
- A missing saved server or keychain entry is `SignInNeeded`. A saved servers file or a keychain that cannot be read is `Other("Marlin could not read the saved SMB sign-in")`.

## Errors

The provider goes by the error code `marlin-smb` answers, never by the text: `marlin-smb` keeps libsmbclient's text when it changes a code.

| `marlin-smb` answers                          | The call returns                                                                                                                                           |
| --------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `SMB_AUTH_FAILED`                             | `SignInNeeded`                                                                                                                                             |
| `SMB_PATH_NOT_FOUND`                          | `NotFound`                                                                                                                                                 |
| `SMB_PERMISSION_DENIED`                       | `AccessDenied`                                                                                                                                             |
| `SMB_CONNECTION_LOST`                         | `Offline`                                                                                                                                                  |
| `SMB_IS_A_FOLDER`                             | `IsAFolder`                                                                                                                                                |
| `SMB_NOT_A_FOLDER`                            | `NotAFolder`                                                                                                                                               |
| `SMB_ALREADY_EXISTS`                          | `Conflict`                                                                                                                                                 |
| `SMB_NOT_EMPTY`                               | `NotEmpty`                                                                                                                                                 |
| `SMB_CHANGED`                                 | `Conflict`                                                                                                                                                 |
| `SMB_NO_SPACE`                                | `NoSpace`                                                                                                                                                  |
| `INVALID_PARAMS`                              | `NotSupported`. A name `marlin-smb` cannot send fails this item, never the mount.                                                                          |
| `SMB_LIBRARY_MISSING`: no `smbclient` program | `Other` with `marlin-smb`'s own text (`SMBCLIENT_MISSING`), which says how to install it                                                                   |
| `marlin-smb` cannot start                     | `Other` with the text the app shows: the helper is missing, Samba is not installed, or the process failed to start, with the OS error, which names no path |
| Any other code                                | `Other("marlin-smb answered error code <code>")`                                                                                                           |
| An answer that is not the expected JSON       | `Other("marlin-smb sent an answer Marlin could not read")`                                                                                                 |
| The process dies during the call              | `Other("marlin-smb stopped during a call")`                                                                                                                |
| The core drops the call at its limit          | `TimedOut`                                                                                                                                                 |
| The call's thread panics                      | `Other("an SMB call stopped unexpectedly")`                                                                                                                |

- **A process that dies is `Other`, not `Offline`.** A lost network answers `SMB_CONNECTION_LOST`. A process that dies in the middle of a call has crashed, and the helper counts that as `degraded`.
- **On macOS, a call to a server its `marlin-smb` process has not reached yet waits up to 20 seconds for a Local Network answer** ([macOS Local Network access](location-providers.md#macos-local-network-access)). With no answer it is `Offline`.
- **No text of an `Other` holds a file name, a path, a share, a host, a user name, or a password**, so the helper logs it ([Logs and secrets](mount-helper.md#logs-and-secrets)).
- What each code means, and how `marlin-smb` tells a refused sign-in from a refused share, is in [Location providers](location-providers.md).

## Known gaps

- **A sign-in refused for a reason that passes stays refused.** A disabled account turned back on, or a domain controller that was down, gives the same answer as a wrong password. The mount sends the same sign-in again only after the user saves the server with a change, or after the helper restarts.
- **A password changed on the server can cost 16 or more failed sign-ins at once.** Up to 8 calls in flight are each refused, and a refused share call signs in at least twice. A domain that locks an account after fewer tries locks it.
- **A new password for the same account is read only after the server refuses the old one.** The sign-in keeps the password of an account it holds, so the first call after the password changed on the server fails with `SignInNeeded` and costs a sign-in. When that is a mount's first call, the mount fails, and the next one reads the new password. Reading the keychain at every call or mount would close this, against [decision 0006](decisions/0006-smb-mounts.md)'s one read per server.
- **One free space for all shares.** A mount reports one size, the first share's. A copy into another share can still run out of room.
- **Free space asks each share in turn until one answers.** A server that lists many shares the user cannot open before one it can makes `space`, the mount's first call, slow: each refused share is a connection and an `smbclient -L` run. Nobody has measured it (#543).
- IPv6 servers do not work yet (#642).

## Testing

- `cargo test -p marlin-smb-client` runs the provider's tests against fake `marlin-smb` scripts. They need no Samba, and `npm run test:rust` and CI run them. They cover the shares, a folder listing, the item IDs, a file's revision, a split read, a call the core drops, all 4 call processes busy, a process that dies, each error code, the sign-in states, and free space. They also cover an upload that does not hold up a listing, progress reaching the caller, a stopped upload removing its temporary file, a failed save removing it, and a removal that failed being tried again before the next upload. One test runs the provider under a real `Vfs`.
- `cargo test -p marlin-smb` runs the provider against a real Samba server (`marlin-test-samba`), with the in-memory keychain. It covers the shares, a listing with a dot name, a read across a 16 MiB piece, the errors, a changed password after `SignInNeeded`, free space, a server that stops, and names with `%XX` in a file and a share. It also covers making, renaming, and removing folders and files, a replace at the base revision, and a stopped upload leaving no temporary file. On libsmbclient before 4.23 the replace test expects `AccessDenied`, because the test server is not on port 445. CI does not run it. See [Testing](testing.md).
