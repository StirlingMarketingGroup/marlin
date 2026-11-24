# Mount helper (`marlin-mount`)

`marlin-mount` is the process that owns every native mount. It runs once per user, apart from the app, so a mount outlives a hot reload, an app crash, and every window. The app tells it what to mount over a local socket.

On Linux and macOS it can mount a Google account: see [Providers](#providers), the [Linux mount adapter](mount-linux.md), and the [macOS mount adapter](mount-macos.md). The helper does not run on Windows (#375). The app starts the helper on the first mount command: see [The app side](#the-app-side). No part of the UI sends one yet (#377). The app bundle holds the helper: see [Building and bundling](#building-and-bundling).

**Covers:** `src-tauri/crates/marlin-mount/`, `src-tauri/crates/marlin-mount-client/`, `src-tauri/src/mount_helper.rs`, `src-tauri/build.rs`, `scripts/run-tauri.mjs`, `scripts/check-bundled-mount-helper.mjs`
**Checked against the code:** 2026-11-23

The design is in [decision 0001](decisions/0001-mount-architecture.md), choices 3 to 6. The helper builds each mount on the [mount core](mount-core.md).

## The parts

| File           | Holds                                                                                                |
| -------------- | ---------------------------------------------------------------------------------------------------- |
| `main.rs`      | The binary: the flags, the default paths, and the signals.                                           |
| `server.rs`    | The socket, the lock that allows one helper per user, the connections, and the request dispatch.     |
| `mounts.rs`    | The mount table and its rules, the three traits that adapters and providers plug into, `MountError`. |
| `adapter.rs`   | `OsAdapter`, the mount adapter this OS has.                                                          |
| `providers.rs` | `LinkedProviders`, the storage providers the helper links.                                           |
| `log.rs`       | The log writer, and `redact`.                                                                        |

The wire types and the client are in a second crate, `marlin-mount-client`. The helper and the app both link it, so the app never links the helper's provider and adapter code.

| File            | Holds                                                                                           |
| --------------- | ----------------------------------------------------------------------------------------------- |
| `protocol.rs`   | The wire types. The helper exports them again as `marlin_mount::protocol`.                      |
| `client.rs`     | `Client`, which connects to the helper or starts one, `shutdown`, and `ask_to_stop`. Unix only. |
| `lib.rs`        | `default_socket`, the one definition of the socket path that the helper and the app share.      |
| `mount_name.rs` | `build`, the folder name an account mounts at.                                                  |

`marlin-mount` and `Client` are Unix only. A Windows named pipe comes with the Windows adapter (#375). On Windows the mount commands fail with the message "Marlin cannot mount on this OS yet."

## Building and bundling

The helper ships inside the app as a Tauri `externalBin` (`bundle.externalBin` in `src-tauri/tauri.conf.json`). Tauri takes the file `src-tauri/binaries/marlin-mount-<target triple>` and puts it beside the app binary under the name `marlin-mount`.

| Build         | Where the helper lands                            |
| ------------- | ------------------------------------------------- |
| A dev build   | `src-tauri/target/debug/marlin-mount`             |
| A macOS app   | `Marlin.app/Contents/MacOS/marlin-mount`          |
| A Linux `deb` | `/usr/bin/marlin-mount`, beside `/usr/bin/marlin` |

- **`npm run tauri dev` and `npm run tauri build` build the helper.** `scripts/run-tauri.mjs` runs `cargo build -p marlin-mount` with the app's profile and target, then copies the result into `src-tauri/binaries/`. A release and the release dry run go through the same script, so the workflows have no build step for the helper.
- **The helper is built when the command starts, not while it runs.** After you change the helper's code, restart `npm run tauri dev`. A helper that is still running keeps its old code: see [The app side](#the-app-side).
- **Any other build gets an empty placeholder.** `tauri-build` fails when a listed helper file is missing. So `src-tauri/build.rs` writes an empty file for each helper that nothing built, `marlin-smb` included. `cargo check`, `cargo test`, and an editor then compile on a fresh clone. An app built that way has no helper: the app treats an empty file as a missing one, and a mount command fails with "Marlin's mount helper is missing".
- **`target/<profile>/marlin-mount` has two writers.** Cargo puts its own build of the helper there, and the app's build script copies the file from `src-tauri/binaries/` there. Cargo puts its copy back on every `cargo build -p marlin-mount` and `cargo test -p marlin-mount`, so the helper's tests run the current code.
- **macOS signing.** Tauri signs the helper inside the bundle with the app's identity, the hardened runtime, and the app's `src-tauri/Entitlements.plist`. So the helper already has `com.apple.security.cs.disable-library-validation`, which it needs to load macFUSE's library. Checked on 2026-11-16 with a test program signed ad hoc with the hardened runtime: with these entitlements it mounts, and with none macOS refuses to load the library. A build signed with the Developer ID certificate is not checked yet (#543). The helper needs none of the app's other entitlements. A file of its own is #544.
- **CI checks the bundle.** `scripts/check-bundled-mount-helper.mjs` starts the helper from the built macOS app or from the built `deb`, and fails unless it answers `hello` and exits with status 0 after `shutdown`. On macOS it first runs `codesign --verify --deep --strict` on the app and checks that the helper has the hardened runtime. The release dry run runs it on every target, and so does a release: see [Release signing](release-signing.md).

Not checked: the `rpm` and the AppImage (#486), and a helper started by an installed release.

## Running it

`marlin-mount [--socket <path>] [--root <path>] [--spool <path>]`

| Path             | Default                                                                                                                                                           |
| ---------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| The socket       | `marlin/mount.sock` under the runtime folder (`$XDG_RUNTIME_DIR` on Linux), or under the local data folder when there is none, as on macOS.                       |
| The mount folder | `~/Marlin`                                                                                                                                                        |
| The spool        | `marlin/spool` under the data folder: `~/Library/Application Support` on macOS, `$XDG_DATA_HOME` or `~/.local/share` on Linux. See [Unsent files](#unsent-files). |

- **One helper per socket.** The default socket path gives each user one helper. The helper takes a lock on `<socket>.lock` and holds it while it lives. A second helper that finds the lock taken logs that and exits with status 0. The OS drops the lock when a process dies, so a crashed helper never blocks the next one. The next one removes the socket file that was left. It removes nothing else: if the socket path holds any other kind of file, the helper fails to start and leaves the file alone.
- **Starting.** Before it takes a request, the helper clears what a killed helper left in the mount folder: see [Dead mounts](#dead-mounts). A client can connect during that sweep, and its `hello` is answered when the sweep is done.
- **Open to that user only.** The socket file and the lock file have mode 0600, and the helper drops a connection from any other user ID.
- **Stopping.** A `shutdown` request unmounts everything, and each unmount waits for its uploads. The unmounts run together. The helper then removes the socket file and exits with status 0. If an unmount fails, it returns -1007 and the helper keeps running. `SIGTERM` and `SIGINT` stop every upload wait at once, unmount what they can, remove the socket file, and exit with status 0 even if an unmount failed. The spool keeps what was still uploading, for the next start.
- **Replacing a helper.** After a `shutdown` response, wait for the helper to close the connection before you start another one. The lock is free by then. A helper started earlier finds the lock taken and exits with status 0.
- **A socket file can outlive a killed helper.** If a connect is refused, start the helper and retry the connect until it succeeds.

## The protocol

JSON-RPC 2.0, one JSON object per line, the same framing as `marlin-smb`. Field names are camelCase. A request line may be at most 64 KiB. A longer one, or one that is not UTF-8, closes the connection. Responses and notifications can be longer: an `unsent` list of many files is.

A request is `{jsonrpc, id, method, params?}`. The `id` must be an unsigned integer. A line that does not parse is answered with id 0, so start at 1.

| Method          | Params                               | Result                                                                                                   |
| --------------- | ------------------------------------ | -------------------------------------------------------------------------------------------------------- |
| `hello`         | `{protocol}`                         | `{protocol, version}`: the helper's protocol number and version.                                         |
| `mount`         | `{provider, account, name}`          | The mount's status, once the mount is up. A failed mount is an error.                                    |
| `unmount`       | `{provider, account}`                | `null`. An account with no mount is a success.                                                           |
| `list`          | none                                 | Every mount's status, sorted by name.                                                                    |
| `status`        | `{provider, account}`                | One mount's status, or `null`.                                                                           |
| `shutdown`      | none                                 | `null`, then the helper exits.                                                                           |
| `deadMounts`    | none                                 | Every [dead mount](#dead-mounts) in the mount folder right now, as `{name, mountPoint}`, sorted by name. |
| `unsent`        | `{provider, account}`                | The mount's [unsent files](#unsent-files), or `null` when the mount takes no writes.                     |
| `retryUnsent`   | `{provider, account, mountId, node}` | `null`. Sends one failed or refused file now.                                                            |
| `discardUnsent` | `{provider, account, mountId, node}` | `null`. Drops one file's unsent bytes.                                                                   |

A status is `{provider, account, name, mountPoint, state, error?, readOnly?}`. `error` is `{code, message}` and is there only when the state is `failed`. `readOnly` is there only when the mount takes no writes: see [Unsent files](#unsent-files).

The helper also sends notifications. `{jsonrpc, method: "mountChanged", params: <status>}` is one for every state change of every mount. `{jsonrpc, method: "unsentChanged", params: <unsent files>}` is one when a mount's unsent files change.

What a client must know:

- **Requests on one connection run at the same time.** Responses come back in any order, mixed with notifications. Wait for a response before you send a request that depends on it: a `mount` and an `unmount` of one account sent back to back can run in either order.
- **Notifications start after the `hello` response and are not replayed.** Call `list` after `hello`, then `unsent` for each mount. A client that falls 64 notifications behind loses the oldest ones, and those two calls are how it catches up. Drop every unsent list and its `mountId` when the connection closes. The notifications a client does get arrive in the order the changes happened.
- **A failed `mount` answers with an error, not a status.** The codes -1001, -1002, and -1009 are refusals: they make no entry, keep an entry that is already there, and send no notification. Any other failure leaves a `failed` entry, which keeps its name until the next attempt or an `unmount`.

**Version rules.** The protocol number is 6 (`PROTOCOL_VERSION` in `marlin-mount-client/src/protocol.rs`).

- The first request on a connection must be `hello`. The helper answers it before it reads anything else.
- A `hello` with a valid `protocol` succeeds whatever the number is, and returns the helper's own number.
- If the client's number differs, that connection may only send `shutdown`, and it gets no notifications. This is how a newer app replaces an older helper.

**Error codes.**

| Code   | Meaning                                                                                    |
| ------ | ------------------------------------------------------------------------------------------ |
| -32700 | The line is not a request.                                                                 |
| -32601 | Unknown method.                                                                            |
| -32602 | Wrong params.                                                                              |
| -1001  | The mount name is not one plain folder name.                                               |
| -1002  | Another account's mount has this name, ignoring letter case.                               |
| -1003  | The mount point exists and is not an empty folder.                                         |
| -1004  | Unknown provider.                                                                          |
| -1005  | The provider could not open the account, or the account's [first call](#providers) failed. |
| -1006  | The adapter could not mount.                                                               |
| -1007  | The adapter could not unmount.                                                             |
| -1008  | A folder could not be made or read.                                                        |
| -1009  | The helper is shutting down and takes no new mount.                                        |
| -1010  | `hello` must come first.                                                                   |
| -1011  | The protocol versions differ, and the request was not a `shutdown`.                        |
| -1012  | A dead mount is at the mount point, and the helper could not clear it.                     |
| -1013  | The account's first call said the sign-in is gone. The user must sign in again.            |
| -1014  | The account's first call could not reach the remote. Try again later.                      |
| -1015  | That file has no unsent bytes in this mount, or the `mountId` is not this mount's.         |
| -1016  | A discard waited too long for the mount's other changes. Try again.                        |

The protocol has no `health` request. A helper that answers `hello` is alive, and a machine that cannot mount says why in the error of a `mount`.

## The app side

The app talks to the helper through `Client` in `marlin-mount-client`. `src-tauri/src/mount_helper.rs` keeps one `Client` for the app process and puts eight commands over it.

| Command               | Helper request  | Returns                  |
| --------------------- | --------------- | ------------------------ |
| `list_mounts`         | `list`          | `MountStatus[]`          |
| `get_mount_status`    | `status`        | `MountStatus`, or `null` |
| `mount_account`       | `mount`         | `MountStatus`            |
| `unmount_account`     | `unmount`       | nothing                  |
| `list_dead_mounts`    | `deadMounts`    | `DeadMount[]`            |
| `list_unsent_files`   | `unsent`        | `UnsentFiles`, or `null` |
| `retry_unsent_file`   | `retryUnsent`   | nothing                  |
| `discard_unsent_file` | `discardUnsent` | nothing                  |

The types are in `src/types/index.ts`. A failed command rejects with a `MountCommandError`, `{code?, message}`. `code` is one of the helper's [error codes](#the-protocol), and it is missing when the failure was not an answer from the helper: a missing binary, a helper that would not start, a lost connection.

| Event                 | Payload       | Sent when                           |
| --------------------- | ------------- | ----------------------------------- |
| `mount-changed`       | `MountStatus` | The helper sends `mountChanged`.    |
| `unsent-changed`      | `UnsentFiles` | The helper sends `unsentChanged`.   |
| `mount-helper-closed` | none          | The connection to the helper ended. |

**What the frontend must do.** Call `list_mounts` once before it trusts any event, and again after `mount-helper-closed`: notifications are not replayed, and after a lost connection every mount's state is unknown. Then call `list_unsent_files` for each mount: `unsent-changed` comes only when a list changes, so a list that stays the same is never sent again. After `mount-helper-closed`, drop every unsent list and its `mountId`. The app connects only when a command runs, so no event arrives before the first command.

**A helper that is finishing its uploads.** After a quit, the helper stays until its uploads end. An app opened in that time reaches the same helper. `list_mounts` shows its mounts as `unmounting`, `list_unsent_files` shows what is left, and every `mount_account` fails with -1009. -1009 can be retried: send the mount again every few seconds while the connection lasts. A shutdown whose unmount failed takes mounts again, and the helper stays. After `mount-helper-closed`, refresh as above and mount again: that starts a new helper.

**Starting the helper.** The first command connects to the socket. The app starts a helper only when the socket file is missing or refuses the connection. Any other error, such as no permission on the socket's folder, is reported and starts nothing.

- The binary is the file `marlin-mount` in the folder of the app binary, and nowhere else. [Building and bundling](#building-and-bundling) says how it gets there. When the file is missing or empty, the command fails with "Marlin's mount helper is missing", and the rest of the app works.
- The helper is detached: it runs in its own process group, with no pipe to the app. A Ctrl+C in the terminal that runs `npm run tauri dev` does not reach it, and it keeps running when the app is killed or rebuilt.
- The command line holds `--socket` and the socket path, and nothing else.
- The app waits up to 10 seconds for the new helper to take a connection, and up to 30 seconds for the answer to `hello`, which comes after the helper's sweep.
- Two commands at once start one helper: the app holds a lock while it connects. A window or a webview reload reuses the connection. An app that was restarted finds the running helper through the socket.

**A helper with another protocol version.** The app sends it `shutdown`, waits for it to close the connection (a helper with uploads to finish closes it only after they end, so the first command waits that long), starts the helper beside the app, and connects again. It does this once. If the old helper cannot unmount, it keeps running and the command fails with the helper's reason. If the helper beside the app also speaks another version, the command fails and says both numbers. In a dev build that means `target/debug/marlin-mount` is stale: restart `npm run tauri dev`, which builds it again.

**A helper with the same protocol version is reused, whatever its build.** A helper left by a killed dev app keeps running its old code after the helper is built again. Quit Marlin from its menu, or stop the `marlin-mount` process, to load the new build.

**When the user quits.** The app handles Tauri's `RunEvent::Exit`: it connects to the socket and sends `shutdown`. It never starts a helper to do this.

- A window that closes while others stay open is not a quit. A killed app gets no `Exit` event, and neither does a rebuild under `npm run tauri dev`, so the helper outlives both.
- The app waits for the answer to `hello`, sends `shutdown` with `ask_to_stop`, and quits. It does not wait for the answer, which comes only after the uploads end. The helper reads a request sent before the connection closed, and finishes its uploads and unmounts, then exits, even when the app has gone. The app gives the helper 5 seconds to answer `hello`. A dev build logs a failure. A release build has no logger.
- The app tells the user nothing at quit. It has no window left. #377 shows unsent files before a quit.
- **If an unmount fails at quit, the app quits anyway and shows nothing.** The helper keeps running and keeps serving that mount, because a program still has a file open in it. The next start of Marlin finds that helper and uses it.
- **Limit: two Marlin processes share one helper.** When one of them quits, it stops the helper under the other (#508).

## Mounts

A mount is keyed by provider and account. Its mount point is the mount folder plus `name`.

**The caller sends the name.** Decision 0001 says a mount's name is built once and stored with the account. So the helper checks the name and never invents one. The app builds it with `mount_name::build` in `marlin-mount-client` when an account is added, and for each stored account that has none.

The name follows these rules:

- It starts with the provider, a hyphen, and the account, for example `gdrive-ana@example.com`.
- A character a folder name cannot hold becomes `_`. That is a control character, and `<` `>` `:` `"` `/` `\` `|` `?` `*`.
- It is at most 255 bytes, cut on a character boundary.
- A `.` or a space at the end becomes `_`.
- A name that matches another account's stored name, ignoring letter case, gets `-2`, then `-3`, and so on. The helper compares names the same way. Neither sees two names that differ only by Unicode form as one name (#542).
- The result is a valid folder name on Linux, macOS, and Windows, because the accounts file can move between machines.
- The app stores the name and never builds it again.

| State           | Meaning                                                                                              |
| --------------- | ---------------------------------------------------------------------------------------------------- |
| `starting`      | The helper is opening the account and mounting.                                                      |
| `mounted`       | The OS shows the mount, and the remote answers its calls.                                            |
| `degraded`      | The mount is up, but calls to the remote time out or fail with an error the provider could not name. |
| `auth-required` | The mount is up, but the remote needs the user to sign in again.                                     |
| `offline`       | The mount is up, but the remote cannot be reached.                                                   |
| `unmounting`    | The adapter is unmounting.                                                                           |
| `failed`        | The mount attempt failed. The status keeps the error until the next attempt.                         |
| `unmounted`     | Only in the last `mountChanged` for a mount. The mount is no longer in `list`.                       |

Rules the table keeps:

- **One operation per mount at a time.** A mount or unmount first takes a lock that belongs to the provider and account. Operations on different mounts run together.
- **Mounting twice is safe.** A mount request for an account that is already mounted returns its status, and the adapter is not called again. A live mount keeps its name, whatever name the new request asks for.
- **A failed mount can be retried.** The next `mount` tries again. An `unmount` removes the failed entry.
- **Unmounting twice is safe.**
- **An unmount waits until no file is waiting or uploading.** A failed file does not hold it: it waits for its retry. A refused file does not hold it: no try comes by itself. An open file does not hold it: nothing can close one after the unmount. There is no fixed time limit. An upload that stops moving fails after the core's stall time.
- **The last `unsentChanged` before `unmounted` lists what stays in the spool.** An unmount never deletes spooled bytes. The next mount of the account that takes writes uploads them, except unplaced files, which stay on the disk for the user.
- **A failed unmount keeps the mount.** The mount comes back with its current health, and the request gets the error. Uploads keep running.
- **An operation finishes even when its client has gone.**
- **The helper never mounts over anything.** A mount point must be absent or an empty real folder. A file, a folder with something in it, or a symbolic link fails with -1003 and is left as it was. The one thing the helper clears first is a [dead mount](#dead-mounts). This guards the user's files against a mistake. It is not a defense against another process of the same user.
- **Folders.** The helper opens the account and makes [the first call](#providers) before it makes any folder, so a mount the provider refuses leaves nothing behind. It makes the mount point with mode 0700. It removes it after an unmount and after a failed mount. It only ever removes an empty folder, and an empty folder at a mount point counts as the helper's own.
- **Shutdown.** From the moment a shutdown starts, a new `mount` gets -1009. The unmounts run together, and each one waits for its uploads. The helper stays until they end, even after the app quit. If any unmount fails, `shutdown` returns -1007, the helper keeps running, and it takes mounts again: a mount that is still served must not lose its helper. The mounts that did unmount stay unmounted, so call `list`. A signal is different. It is an order to exit, so every unmount stops waiting for uploads at once. The spool keeps the rest for the next start. The helper then unmounts what it can and exits anyway. A mount it could not unmount is then dead, and the next helper clears it when it starts: see [Dead mounts](#dead-mounts). On Linux the adapter may detach it as the helper exits instead: see [Linux mount adapter](mount-linux.md).

## Providers

`LinkedProviders` in `providers.rs` opens an account for a mount. It knows one provider. Any other name fails with -1004.

| Provider | Account                    | Storage                                        |
| -------- | -------------------------- | ---------------------------------------------- |
| `gdrive` | The Google account's email | [Google Drive mount provider](mount-gdrive.md) |

**Opening a Google account** makes no request. The helper finds Marlin's OAuth client and builds the provider on the account's [saved sign-in](mount-gdrive.md#the-saved-sign-in). A build with no OAuth client, such as a dev build with no `src-tauri/.env`, fails here with -1005, "this build of Marlin has no Google OAuth client". No text from the OAuth client loader reaches a log or a response: it can hold a file path.

**One saved sign-in per account, for the life of the helper.** It keeps the refresh token in memory, so an unmount and a new mount of the account read the keychain once in all.

**The first call.** Before the helper makes any folder, it asks the provider for the account's free space, with the limits of any call of a mount. For Google that is one request to the Drive API, and a token request before it when the helper holds no fresh token. A failure leaves no folder:

| The call ends with                   | Code  | Text                                                |
| ------------------------------------ | ----- | --------------------------------------------------- |
| `SignInNeeded`                       | -1013 | "the account needs a new sign-in"                   |
| `Offline` or `TimedOut`              | -1014 | "the account's storage could not be reached"        |
| `AccessDenied`                       | -1005 | "the account's storage does not let Marlin read it" |
| `Other`, or any other provider error | -1005 | The error's own text                                |

-1013 tells the app to offer a new sign-in without reading the text. -1014 means no answer came. An `Other` stays -1005 because the remote did answer, and its text says how: a 5xx, or a rate limit. A later try can work for both.

- **Why the helper asks.** A mount starts as `mounted`, and its [health](#health) only changes at a provider call. A mount that came up while the remote was out of reach would show `mounted`. A token is no proof: the helper can hold one from an earlier mount.
- **A mount needs its remote at the start.** Send `mount` again when the network is back. A mount that is already up survives a lost network: it shows `offline` and recovers by itself.
- **The rule is the mount table's, not Google's.** It is in `Mounts::mount`, so every provider gets it. `Providers::open` need not reach the remote.

## Dead mounts

A dead mount is a mount whose server was killed. The OS still lists it, and nothing answers it. A helper that crashes or is killed with `kill -9` leaves one for each mount it served. A restart of the machine clears them all.

| OS      | A listing of a dead mount fails with                 | What clears it                |
| ------- | ---------------------------------------------------- | ----------------------------- |
| Linux   | "Transport endpoint is not connected" (`ENOTCONN`)   | `fusermount3 -u -z <folder>`  |
| macOS   | "Device not configured" (`ENXIO`)                    | `umount -f <folder>`          |
| Windows | Nothing: the mount folder goes away with the helper. | Nothing. The adapter is #375. |

`stat` proves nothing: on both systems it can still succeed on a dead mount. A plain unmount (`fusermount3 -u`, `umount`) fails with "busy" while any process has its working folder inside, for example a terminal the user left there. So the helper uses the lazy form on Linux and the forced form on macOS. Nothing serves a dead mount, so no open file is lost.

The helper clears dead mounts at two moments:

- **The sweep, when the helper starts.** It looks at each real folder in the mount folder. It unmounts a dead mount, then removes the folder if it is empty. A folder that holds anything, a file, a symbolic link, and a live mount are left as they were. It never deletes anything but an empty folder.
- **A `mount` request.** If a dead mount is at the mount point, the helper unmounts it and then mounts. The provider is opened and asked first, so a provider error (-1004, -1005, -1013, -1014) comes before this.

**A dead mount the helper cannot clear** does not stop the helper. It shows in three places:

- `deadMounts` lists it. The request looks at the mount folder each time, so a mount leaves the list once it is cleared.
- A `mount` at that folder fails with -1012 and leaves a `failed` entry. The next `mount` tries to clear it again.
- The sweep logs `helper.dead_mount` with the mount point and what the unmount command reported.

The user can then run the command from the table by hand. If that fails too, a restart of the machine clears every mount.

**Limits.**

- The mount folder is the helper's. The helper treats any folder there whose listing fails with the error in the table as a dead mount, whoever mounted it. Neither OS offers a surer sign that a server is gone.
- The checks have no time limit. A mount in the mount folder that another program serves, and that never answers a listing, holds up the start of the helper, a `deadMounts` request, and a `mount` at that folder. A mount of Marlin's own cannot do this: the core puts a time limit on every call.

**Tested (risk 6 in [decision 0001](decisions/0001-mount-architecture.md)).** On 2026-11-13 the sweep cleared a real dead mount with a process sitting inside it, on Linux (a Debian 12 container, `fuse3` 3.14) and on macOS 27.2 (macFUSE 5.5.0, FSKit backend). No restart was needed on either. Windows waits for its adapter (#375).

## Health

The helper judges a mount's remote from how each provider call ends. After [the first call](#providers) of a mount, it makes no calls of its own. A mount that nothing reads keeps its last state.

| The call ended with                                                             | Kind          |
| ------------------------------------------------------------------------------- | ------------- |
| A value, or any error not listed below (not found, access denied, and the rest) | answered      |
| `SignInNeeded`                                                                  | auth-required |
| `Offline`                                                                       | offline       |
| `TimedOut`, or `Other`                                                          | degraded      |

An answered call sets the failure count back to 0 and the mount back to `mounted`. Any other call adds 1 to that count.

- `SignInNeeded` moves the mount to `auth-required` at once. The remote itself said the sign-in is gone, so there is nothing to wait for.
- After 3 failures in a row of any kind (`FAILURES_IN_A_ROW` in `mounts.rs`), each `Offline` moves the mount to `offline`, and each `TimedOut` or `Other` moves it to `degraded`. One slow call or one dropped connection does not change the state.
- A run of failures only makes the state worse, never better. The order is `degraded`, then `offline`, then `auth-required`. The first call the remote answers moves the mount back to `mounted`.

The helper sends one `mountChanged` per change of state, not one per call. Failed calls that leave the state as it is send nothing.

Each mount has its own count. One mount never changes another.

Only a mount that is up shows its health. `starting`, `unmounting`, and `failed` win. A failed unmount puts the mount back with its current health: calls that end during the unmount still count.

A health state carries no `error`. The helper logs the text of a failure the provider could not name: see [Logs and secrets](#logs-and-secrets).

A mount recovers by itself. In `auth-required`, the next call the remote answers after the user signs in again moves the mount back to `mounted`. No unmount is needed. An `unmount` and a `mount` also work. A `mount` of an unhealthy mount returns its status and mounts nothing.

The state changes only at a provider call. A read that a cache answers makes none. The core reuses a folder listing and a node's attributes for 5 seconds and the free space for 30 seconds, and the OS caches too. After a sign-in, the state can stay `auth-required` until something reads the mount past those caches.

`degraded` covers the core's time limit. That limit includes the wait for a free call slot. A mount whose slots are all held by slow calls becomes `degraded` too.

Results count in the order the calls finish. A call that finishes later is later evidence. Up to 16 calls run at once, so a call that started before a `SignInNeeded` and finishes after it can show `mounted` for a moment. The next call corrects it.

## Unsent files

A file's new bytes go into a spool folder on this computer. The upload runs after the last writer closes. [Decision 0005](decisions/0005-mount-writes.md), choices 5, 8, and 10, says what that means. This section is the helper's part.

The spool root is `marlin/spool` under the data folder: see [Running it](#running-it). On macOS that folder also holds the app's own files and the socket. It is not a cache folder, which the OS and cleanup tools may empty. `--spool` replaces the root.

Each account has one folder: `<spool>/<provider>/<account folder>`, mode 0700.

The account folder name keeps `a-z`, `0-9`, `@`, `.`, `_`, `+`, and `-`. Every other byte is written `%XX` in uppercase hex, and so is a `.` at the start. Upper case letters are escaped too. Two accounts that differ only by letter case then get two folders on a disk that ignores letter case. The mount name is not the folder name. A later account can be given the name of one that was removed.

Before a mount takes the folder, the helper checks it:

- It makes the folder when the folder is missing.
- The path must be a real folder. A file or a symbolic link leaves the mount read-only.
- When the mode is not exactly 0700, the helper sets it to 0700. A folder the user does not own fails that step, and the mount comes up read-only.

The helper locks `<spool>/<provider>/<account folder>.lock` and holds it until the mount's uploads have stopped. The helper's own lock is per socket. Two helpers on two sockets could both recover one spool and upload it. The second mount comes up read-only. It takes writes only when it is mounted again after the first has let the lock go.

A mount is read-only in two cases. The status then carries `readOnly`.

| `readOnly`                            | When                                                                                                |
| ------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `{"reason": "provider"}`              | The provider has no write calls. No spool folder is made.                                           |
| `{"reason": "spool", "message": "…"}` | The spool folder could not be made or read, or another helper holds its lock. `message` says which. |

A read-only mount uploads nothing and deletes no unsent bytes. The user can still read. A recovery that failed partway may already have tidied stale records.

`unsent` answers `{provider, account, mountId, files, unplaced}` for a mount that takes writes. It answers `null` for an account with no mount, and for a mount that takes no writes now: one that is starting or failed, a read-only one, and one whose unmount has stopped its uploads. `readOnly` in the status says whether a mount that is up is read-only. `retryUnsent` and `discardUnsent` answer -1015 in the same cases.

`mountId` is a UUID made when the mount attempt starts. `retryUnsent` and `discardUnsent` send it back. A list kept across a remount or a helper restart must never name another mount's files, because a discard drops bytes for good. A `mountId` from an earlier attempt, a string that is not that UUID, or a node with no unsent bytes is -1015.

Each file is `{node, name, size, state}`. `name` is the file's name as the remote gets it, with no folder. `node` is the core's node number. It only means something together with `mountId`. The list has no folder path: two files of one name in two folders show apart only by node and size. #377 has to live with that or add the path.

| `state`     | Meaning                                                                                                                                                                                                                                                                                                                    |
| ----------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `open`      | A writer has the file open.                                                                                                                                                                                                                                                                                                |
| `waiting`   | No writer has it open, and no upload of it runs. It waits for the settle time, or for a free upload slot.                                                                                                                                                                                                                  |
| `uploading` | An upload of its current bytes runs. `sent` is how far the provider has reported.                                                                                                                                                                                                                                          |
| `failed`    | The last try failed for a reason that can pass. `error` is the text. The next try comes by itself.                                                                                                                                                                                                                         |
| `refused`   | The remote refused the last try: the account may not change the file, or the remote cannot take it. `error` is the text. No try comes by itself: `retryUnsent`, a new edit, a new modified time, or a rename starts one. The next mount of the account tries it once ([decision 0008](decisions/0008-read-only-files.md)). |

While a retry runs, the file shows `uploading` and its last error is gone. A failed retry shows the new error. The app does not need the old one.

A file whose record says an earlier run may have sent it checks the remote first. It shows `uploading` with `sent` 0 meanwhile.

The helper looks at each mount's unsent files every 500 ms (`UNSENT_EVERY` in `mounts.rs`) and sends `unsentChanged` when they changed. An unmount also sends one last `unsentChanged` before `unmounted`, which can repeat the one before it. The body is the same shape as an `unsent` answer. A client that falls behind calls `unsent`.

`retryUnsent` sends a failed or refused file now. `discardUnsent` drops the unsent bytes. An item shows the remote's bytes again. A new file is never uploaded. A discard that waits too long for the mount's other changes is -1016. Both calls work while the mount is up, and while an unmount waits for uploads.

`unplaced` lists spool files from an earlier run that have no readable record. They are kept and never uploaded. The user can open them at those paths.

## Adding an adapter or a provider

`mounts.rs` has the three traits:

- `Adapter::mount(vfs, mount_point)` serves a `marlin_vfs::Vfs` at an empty folder and returns a `Session` once the OS shows the mount.
- `Adapter::is_dead_mount(mount_point)` says whether a [dead mount](#dead-mounts) is there. A symbolic link is never one. `Adapter::unmount_dead(mount_point)` unmounts it and leaves the folder.
- `Session::unmount()` unmounts. After an error the mount must still be served.
- `Providers::open(provider, account)` returns the `marlin_vfs::Provider` for an account. `account` names the account and is never a secret. The provider reads its own credentials from the OS keychain. It need not reach the remote: the mount table makes [the first call](#providers).

A provider's errors decide a mount's [health](#health). `VfsError` in `marlin-vfs/src/error.rs` says which error to return: `Other` is for a failure of the remote, never for one item, and its text holds no file name and no secret, because the helper logs it. After `SignInNeeded`, a provider reads the keychain again on its next call, so the mount recovers without an unmount.

`main.rs` passes one adapter and one `Providers` value to `Server::bind`. The adapter is `OsAdapter` in `adapter.rs`. It mounts through `marlin-fuse`, which has a Linux half and a macOS half. The `Providers` value is `LinkedProviders`, which opens `gdrive` and refuses every other name. An adapter lives in its own crate, so `marlin-mount` implements `Adapter` for it in a few lines and the adapter crate does not depend on the helper. Both values are in the library, not in `main.rs`, so a test can run the real server with them inside the test process.

## Logs and secrets

The helper writes one JSON object per line to stderr: `timeMs`, `level`, `event`, and the fields of the event. It logs its start and stop, a failed start, a dead mount the sweep could not clear, a failed unmount at a signal, a refused connection, a line that was too long, and each mount state change with the mount's status (provider, account, name, mount point, state, error, readOnly). It logs `mount.spool_failed` when a mount comes up read-only because the spool folder failed, with the provider, the account, and the message. It logs `mount.left_unsent` when an unmount leaves files in the spool, with the provider, the account, how many files still have to upload, and how many are unplaced. It logs nothing else from a request, and never file names inside a mount or file contents.

**A provider failure with a text.** When a provider call ends with `VfsError::Other(text)`, the helper logs `mount.call_failed` with the provider, the account, and the text, passed through `redact`. A mount gets a call for every file the OS touches, so a mount logs the same text once a minute at most. Another text is logged at once. A first call that fails with a text logs both this line and the `failed` state. `Offline`, `TimedOut`, and `SignInNeeded` carry no text. The state change they cause is the log line.

Whoever starts the helper must point stderr at a file or `/dev/null`, never at a pipe. The helper drops a log line it cannot write, but a pipe that is full and never read blocks it.

The app points it at `marlin-mount.log` in the app's log folder: `~/Library/Logs/com.marlin.filebrowser/` on macOS, and `~/.local/share/com.marlin.filebrowser/logs/` on Linux. The file has mode 0600. Lines are added to it across helper starts. When the app starts a helper and the file is over 1 MiB, it first renames the file to `marlin-mount.log.old`, which replaces the one before.

No secret crosses the socket or the command line. The text inside a `MountError` must hold no credential. As a backstop, `MountError` prints through `redact`, which masks bearer tokens, the values of fields such as `access_token` and `password`, and the user and password in a URL. That is the only way the text reaches a log line, a status, or a response. `redact` masks the word that follows `token:`, `password=`, `Bearer`, `Basic`, and the like, so word an error so that its cause does not sit there.

## Testing

`cd src-tauri && cargo test -p marlin-mount` needs no kernel driver, no network, and no account. It compiles the helper against the half of `marlin-fuse` for the machine it runs on. From a Mac, `src-tauri/crates/marlin-fuse/test-in-docker.sh` covers the Linux half.

- `tests/control.rs` runs the server on a real socket in a temporary folder, with a fake adapter and the core's `MemoryProvider`. The fake adapter can hold a mount or an unmount open, so the tests for two requests at once do not depend on timing. The fake adapter keeps each mount's `Vfs`, so a test writes a file through it and sets failures with `MemoryProvider::fail_with`. The harness gives each mount a spool in the temp folder and a settle time of 50 ms. The spool tests cover a private folder, a failed upload, a refused upload, retry, discard, a wrong `mountId`, an unmount that leaves a failed file, an unmount that waits for an upload, a discard that ends that wait, a shutdown that waits, a mount sent during that wait (-1009 at once), a signal that ends an unmount's wait, a spool path that is a file, a folder with mode 0755 or 0500, a symbolic link, two helpers on one spool, an unmount that waits for a discard held up by a folder being made, and a spool file with no record. No test covers a provider that cannot write.
- One test starts the built binary and checks its flags, a refused mount, and a clean exit. **A test never sends provider `gdrive` to the built binary.** On a developer's machine that would read the real keychain. Those tests use the provider name `memory`, which the binary does not know.
- One test reads the log. The server runs inside the test process, so the test points the process's stderr at a file while it makes the failed calls. It moves tokio's clock a minute ahead to see the same text logged again.
- `tests/gdrive.rs` runs the real server with `OsAdapter` and `LinkedProviders` against a fake Google: one `wiremock` server that is the Drive API and the token endpoint, and the in-memory keychain. It talks to the server through `Client`. Four tests run on any Unix: a sign-in that is gone (-1013), Google out of reach (-1014), a Drive API that answers 503 or 403 (-1005), and a missing OAuth client (-1005). Each fails and leaves no folder. Two tests mount through FUSE and run only in `test-in-docker.sh` and `test-on-mac.sh`. The first lists a folder that Google sends in two pages, with a duplicate name, a `/` in a name, and a Google Doc, reads a file whole and at an offset, reads the Doc's link file and checks its size, checks that a write, a delete, and a rename fail with `EROFS` because the spool path is a file, and checks that the fake Google got nothing but GETs and one token request, and mounts the account a second time, which asks Google again though the helper still holds a token. The second rejects the sign-in, waits for `auth-required`, signs in again, and waits for `mounted`. Its first status takes writes.
- The fake adapter can call a folder a dead mount, and can refuse to clear it. Two tests cover the sweep, `deadMounts`, and error -1012 that way. A unit test in `marlin-fuse` (`src/lib.rs`) checks the real rule on a folder, a missing path, and a symbolic link.
- `src-tauri/crates/marlin-mount/test-sweep.sh` makes a real dead mount and a live one with the spike programs in `spikes/mount-hello/`, starts the built helper, and checks that only the dead mount and the empty folder are gone. Run it on a Mac with macFUSE, both of its switches on: `test-on-mac.sh` runs it there. `test-in-docker.sh` runs it on Linux.
- `tests/client.rs` tests `Client` against the built helper: the first connect starts it, a second one reuses it, it outlives its client, it runs in its own process group, `shutdown` stops it, and `ask_to_stop` returns at once and the helper still exits. A fake helper, a plain socket listener in the test, covers another protocol version, a refused shutdown, and a notification in front of an answer.
- The Tauri layer in `src-tauri/src/mount_helper.rs` has one test: the request at quit runs outside the async runtime, with no helper. No real-app test covers mounts yet (#580), so the `Exit` event, the eight commands, and the three events are untested.
- A signal is tested through the future that `Server::run` takes, not with a real `SIGTERM`.
- The check of the connecting user's ID has no test. It needs a second user account.
