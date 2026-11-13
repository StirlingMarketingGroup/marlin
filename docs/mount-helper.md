# Mount helper (`marlin-mount`)

`marlin-mount` is the process that will own every native mount. It runs once per user, apart from the app, so a mount outlives a hot reload, an app crash, and every window. The app tells it what to mount over a local socket.

It cannot mount anything yet. Linux has an adapter, the [Linux mount adapter](mount-linux.md). macOS (#373) and Windows (#375) have none. The helper links no provider yet: the [Google Drive mount provider](mount-gdrive.md) is built, and #529 wires it in. So no mount request can succeed: one that passes the name checks fails with -1004, unknown provider. The app starts the helper on the first mount command: see [The app side](#the-app-side). No part of the UI sends one yet (#377). The app bundle holds the helper: see [Building and bundling](#building-and-bundling).

**Covers:** `src-tauri/crates/marlin-mount/`, `src-tauri/crates/marlin-mount-client/`, `src-tauri/src/mount_helper.rs`, `src-tauri/build.rs`, `scripts/run-tauri.mjs`, `scripts/check-bundled-mount-helper.mjs`
**Checked against the code:** 2026-11-13

The design is in [decision 0001](decisions/0001-mount-architecture.md), choices 3 to 6. The helper builds each mount on the [mount core](mount-core.md).

## The parts

| File        | Holds                                                                                                |
| ----------- | ---------------------------------------------------------------------------------------------------- |
| `main.rs`   | The binary: two flags, the default paths, and the signals.                                           |
| `server.rs` | The socket, the lock that allows one helper per user, the connections, and the request dispatch.     |
| `mounts.rs` | The mount table and its rules, the three traits that adapters and providers plug into, `MountError`. |
| `log.rs`    | The log writer, and `redact`.                                                                        |

The wire types and the client are in a second crate, `marlin-mount-client`. The helper and the app both link it, so the app never links the helper's provider and adapter code.

| File          | Holds                                                                                      |
| ------------- | ------------------------------------------------------------------------------------------ |
| `protocol.rs` | The wire types. The helper exports them again as `marlin_mount::protocol`.                 |
| `client.rs`   | `Client`, which connects to the helper or starts one, and `shutdown`. Unix only.           |
| `lib.rs`      | `default_socket`, the one definition of the socket path that the helper and the app share. |

`marlin-mount` and `Client` are Unix only. A Windows named pipe comes with the Windows adapter (#375). On Windows the five commands fail with the message "Marlin cannot mount on this OS yet."

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
- **macOS signing.** Tauri signs the helper inside the bundle with the app's identity, the hardened runtime, and the app's `src-tauri/Entitlements.plist`. So the helper already has `com.apple.security.cs.disable-library-validation`, which it needs to load macFUSE's library. An entitlements file of its own waits for the macOS adapter (#373). Nothing loads a library until then, so nothing could test the file.
- **CI checks the bundle.** `scripts/check-bundled-mount-helper.mjs` starts the helper from the built macOS app or from the built `deb`, and fails unless it answers `hello` and exits with status 0 after `shutdown`. On macOS it first runs `codesign --verify --deep --strict` on the app and checks that the helper has the hardened runtime. The release dry run runs it on every target, and so does a release: see [Release signing](release-signing.md).

Not checked: the `rpm` and the AppImage (#486), and a helper started by an installed release.

## Running it

`marlin-mount [--socket <path>] [--root <path>]`

| Path             | Default                                                                                                                                     |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| The socket       | `marlin/mount.sock` under the runtime folder (`$XDG_RUNTIME_DIR` on Linux), or under the local data folder when there is none, as on macOS. |
| The mount folder | `~/Marlin`                                                                                                                                  |

- **One helper per socket.** The default socket path gives each user one helper. The helper takes a lock on `<socket>.lock` and holds it while it lives. A second helper that finds the lock taken logs that and exits with status 0. The OS drops the lock when a process dies, so a crashed helper never blocks the next one. The next one removes the socket file that was left. It removes nothing else: if the socket path holds any other kind of file, the helper fails to start and leaves the file alone.
- **Starting.** Before it takes a request, the helper clears what a killed helper left in the mount folder: see [Dead mounts](#dead-mounts). A client can connect during that sweep, and its `hello` is answered when the sweep is done.
- **Open to that user only.** The socket file and the lock file have mode 0600, and the helper drops a connection from any other user ID.
- **Stopping.** A `shutdown` request unmounts everything, removes the socket file, and exits with status 0. If an unmount fails, it returns -1007 and the helper keeps running. `SIGTERM` and `SIGINT` try every unmount, remove the socket file, and exit with status 0 even if an unmount failed.
- **Replacing a helper.** After a `shutdown` response, wait for the helper to close the connection before you start another one. The lock is free by then. A helper started earlier finds the lock taken and exits with status 0.
- **A socket file can outlive a killed helper.** If a connect is refused, start the helper and retry the connect until it succeeds.

## The protocol

JSON-RPC 2.0, one JSON object per line, the same framing as `marlin-smb`. Field names are camelCase. A line may be at most 64 KiB. A longer line, or one that is not UTF-8, closes the connection.

A request is `{jsonrpc, id, method, params?}`. The `id` must be an unsigned integer. A line that does not parse is answered with id 0, so start at 1.

| Method       | Params                      | Result                                                                                                   |
| ------------ | --------------------------- | -------------------------------------------------------------------------------------------------------- |
| `hello`      | `{protocol}`                | `{protocol, version}`: the helper's protocol number and version.                                         |
| `mount`      | `{provider, account, name}` | The mount's status, once the mount is up. A failed mount is an error.                                    |
| `unmount`    | `{provider, account}`       | `null`. An account with no mount is a success.                                                           |
| `list`       | none                        | Every mount's status, sorted by name.                                                                    |
| `status`     | `{provider, account}`       | One mount's status, or `null`.                                                                           |
| `shutdown`   | none                        | `null`, then the helper exits.                                                                           |
| `deadMounts` | none                        | Every [dead mount](#dead-mounts) in the mount folder right now, as `{name, mountPoint}`, sorted by name. |

A status is `{provider, account, name, mountPoint, state, error?}`. `error` is `{code, message}` and is there only when the state is `failed`.

The helper also sends notifications: `{jsonrpc, method: "mountChanged", params: <status>}`, one for every state change of every mount.

What a client must know:

- **Requests on one connection run at the same time.** Responses come back in any order, mixed with notifications. Wait for a response before you send a request that depends on it: a `mount` and an `unmount` of one account sent back to back can run in either order.
- **Notifications start after the `hello` response and are not replayed.** Call `list` after `hello`. A client that falls 64 notifications behind loses the oldest ones, and `list` is how it catches up. The notifications a client does get arrive in the order the changes happened.
- **A failed `mount` answers with an error, not a status.** The codes -1001, -1002, and -1009 are refusals: they make no entry, keep an entry that is already there, and send no notification. Any other failure leaves a `failed` entry, which keeps its name until the next attempt or an `unmount`.

**Version rules.** The protocol number is `PROTOCOL_VERSION` in `marlin-mount-client/src/protocol.rs`.

- The first request on a connection must be `hello`. The helper answers it before it reads anything else.
- A `hello` with a valid `protocol` succeeds whatever the number is, and returns the helper's own number.
- If the client's number differs, that connection may only send `shutdown`, and it gets no notifications. This is how a newer app replaces an older helper.

**Error codes.**

| Code   | Meaning                                                                |
| ------ | ---------------------------------------------------------------------- |
| -32700 | The line is not a request.                                             |
| -32601 | Unknown method.                                                        |
| -32602 | Wrong params.                                                          |
| -1001  | The mount name is not one plain folder name.                           |
| -1002  | Another account's mount has this name, ignoring letter case.           |
| -1003  | The mount point exists and is not an empty folder.                     |
| -1004  | Unknown provider.                                                      |
| -1005  | The provider could not open the account.                               |
| -1006  | The adapter could not mount.                                           |
| -1007  | The adapter could not unmount.                                         |
| -1008  | A folder could not be made or read.                                    |
| -1009  | The helper is shutting down and takes no new mount.                    |
| -1010  | `hello` must come first.                                               |
| -1011  | The protocol versions differ, and the request was not a `shutdown`.    |
| -1012  | A dead mount is at the mount point, and the helper could not clear it. |

The protocol has no `health` request. A helper that answers `hello` is alive, and a machine that cannot mount says why in the error of a `mount`.

## The app side

The app talks to the helper through `Client` in `marlin-mount-client`. `src-tauri/src/mount_helper.rs` keeps one `Client` for the app process and puts five commands over it.

| Command            | Helper request | Returns                  |
| ------------------ | -------------- | ------------------------ |
| `list_mounts`      | `list`         | `MountStatus[]`          |
| `get_mount_status` | `status`       | `MountStatus`, or `null` |
| `mount_account`    | `mount`        | `MountStatus`            |
| `unmount_account`  | `unmount`      | nothing                  |
| `list_dead_mounts` | `deadMounts`   | `DeadMount[]`            |

The types are in `src/types/index.ts`. A failed command rejects with a `MountCommandError`, `{code?, message}`. `code` is one of the helper's [error codes](#the-protocol), and it is missing when the failure was not an answer from the helper: a missing binary, a helper that would not start, a lost connection.

| Event                 | Payload       | Sent when                           |
| --------------------- | ------------- | ----------------------------------- |
| `mount-changed`       | `MountStatus` | The helper sends `mountChanged`.    |
| `mount-helper-closed` | none          | The connection to the helper ended. |

**What the frontend must do.** Call `list_mounts` once before it trusts any event, and again after `mount-helper-closed`: notifications are not replayed, and after a lost connection every mount's state is unknown. The app connects only when a command runs, so no event arrives before the first command.

**Starting the helper.** The first command connects to the socket. The app starts a helper only when the socket file is missing or refuses the connection. Any other error, such as no permission on the socket's folder, is reported and starts nothing.

- The binary is the file `marlin-mount` in the folder of the app binary, and nowhere else. [Building and bundling](#building-and-bundling) says how it gets there. When the file is missing or empty, the command fails with "Marlin's mount helper is missing", and the rest of the app works.
- The helper is detached: it runs in its own process group, with no pipe to the app. A Ctrl+C in the terminal that runs `npm run tauri dev` does not reach it, and it keeps running when the app is killed or rebuilt.
- The command line holds `--socket` and the socket path, and nothing else.
- The app waits up to 10 seconds for the new helper to take a connection, and up to 30 seconds for the answer to `hello`, which comes after the helper's sweep.
- Two commands at once start one helper: the app holds a lock while it connects. A window or a webview reload reuses the connection. An app that was restarted finds the running helper through the socket.

**A helper with another protocol version.** The app sends it `shutdown`, waits for it to close the connection, starts the helper beside the app, and connects again. It does this once. If the old helper cannot unmount, it keeps running and the command fails with the helper's reason. If the helper beside the app also speaks another version, the command fails and says both numbers. In a dev build that means `target/debug/marlin-mount` is stale: restart `npm run tauri dev`, which builds it again.

**A helper with the same protocol version is reused, whatever its build.** A helper left by a killed dev app keeps running its old code after the helper is built again. Quit Marlin from its menu, or stop the `marlin-mount` process, to load the new build.

**When the user quits.** The app handles Tauri's `RunEvent::Exit`: it connects to the socket and sends `shutdown`. It never starts a helper to do this.

- A window that closes while others stay open is not a quit. A killed app gets no `Exit` event, and neither does a rebuild under `npm run tauri dev`, so the helper outlives both.
- The app waits up to 5 seconds. The request needs that time to reach the helper, which answers `hello` first. Once the helper has the request, it finishes the shutdown even when the app has gone. A dev build logs a failure. A release build has no logger.
- **If an unmount fails at quit, the app quits anyway and shows nothing.** The helper keeps running and keeps serving that mount, because a program still has a file open in it. The next start of Marlin finds that helper and uses it.
- **Limit: two Marlin processes share one helper.** When one of them quits, it stops the helper under the other (#508).

## Mounts

A mount is keyed by provider and account. Its mount point is the mount folder plus `name`.

**The caller sends the name.** Decision 0001 says a mount's name is built once and stored with the account. So the helper checks the name and never invents one. Nothing builds names yet: that is #526.

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
- **A failed unmount keeps the mount.** The mount comes back with its current health, and the request gets the error.
- **An operation finishes even when its client has gone.**
- **The helper never mounts over anything.** A mount point must be absent or an empty real folder. A file, a folder with something in it, or a symbolic link fails with -1003 and is left as it was. The one thing the helper clears first is a [dead mount](#dead-mounts). This guards the user's files against a mistake. It is not a defense against another process of the same user.
- **Folders.** The helper opens the account before it makes any folder, so a mount the provider refuses leaves nothing behind. It makes the mount point with mode 0700. It removes it after an unmount and after a failed mount. It only ever removes an empty folder, and an empty folder at a mount point counts as the helper's own.
- **Shutdown.** From the moment a shutdown starts, a new `mount` gets -1009. If any unmount fails, `shutdown` returns -1007, the helper keeps running, and it takes mounts again: a mount that is still served must not lose its helper. The mounts that did unmount stay unmounted, so call `list`. A signal is different. It is an order to exit, so the helper unmounts what it can and exits anyway. A mount it could not unmount is then dead, and the next helper clears it when it starts: see [Dead mounts](#dead-mounts). On Linux the adapter may detach it as the helper exits instead: see [Linux mount adapter](mount-linux.md).

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
- **A `mount` request.** If a dead mount is at the mount point, the helper unmounts it and then mounts. The provider is opened first, so a provider error (-1004, -1005) comes before this.

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

The helper judges a mount's remote from how each provider call ends. It makes no calls of its own. A mount that nothing reads keeps its last state.

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

A health state carries no `error`. Provider error text can hold a file name, so it reaches no status and no log line.

A mount recovers by itself. In `auth-required`, the next call the remote answers after the user signs in again moves the mount back to `mounted`. No unmount is needed. An `unmount` and a `mount` also work. A `mount` of an unhealthy mount returns its status and mounts nothing.

The state changes only at a provider call. A read that a cache answers makes none. The core reuses a folder listing for 5 seconds, and the OS caches too. After a sign-in, the state can stay `auth-required` until something reads the mount past those caches.

`degraded` covers the core's time limit. That limit includes the wait for a free call slot. A mount whose slots are all held by slow calls becomes `degraded` too.

Results count in the order the calls finish. A call that finishes later is later evidence. Up to 16 calls run at once, so a call that started before a `SignInNeeded` and finishes after it can show `mounted` for a moment. The next call corrects it.

## Adding an adapter or a provider

`mounts.rs` has the three traits:

- `Adapter::mount(vfs, mount_point)` serves a `marlin_vfs::Vfs` at an empty folder and returns a `Session` once the OS shows the mount.
- `Adapter::is_dead_mount(mount_point)` says whether a [dead mount](#dead-mounts) is there. A symbolic link is never one. `Adapter::unmount_dead(mount_point)` unmounts it and leaves the folder.
- `Session::unmount()` unmounts. After an error the mount must still be served.
- `Providers::open(provider, account)` returns the `marlin_vfs::Provider` for an account. `account` names the account and is never a secret. The provider reads its own credentials from the OS keychain.

A provider's errors decide a mount's [health](#health). `VfsError` in `marlin-vfs/src/error.rs` says which error to return: `Other` is for a failure of the remote, never for one item. After `SignInNeeded`, a provider reads the keychain again on its next call, so the mount recovers without an unmount.

`main.rs` passes one adapter and one `Providers` value to `Server::bind`. The adapter is `adapter::OsAdapter`, chosen per OS at the top of `main.rs`: on Linux it is `marlin-fuse`, and on macOS it refuses every mount but can clear a dead macFUSE mount. The `Providers` value refuses everything. An adapter lives in its own crate, so `marlin-mount` implements `Adapter` for it in a few lines and the adapter crate does not depend on the helper.

## Logs and secrets

The helper writes one JSON object per line to stderr: `timeMs`, `level`, `event`, and the fields of the event. It logs its start and stop, a failed start, a dead mount the sweep could not clear, a failed unmount at a signal, a refused connection, a line that was too long, and each mount state change with the mount's status (provider, account, name, mount point, state, error). It logs nothing else from a request, and never file names inside a mount or file contents.

Whoever starts the helper must point stderr at a file or `/dev/null`, never at a pipe. The helper drops a log line it cannot write, but a pipe that is full and never read blocks it.

The app points it at `marlin-mount.log` in the app's log folder: `~/Library/Logs/com.marlin.filebrowser/` on macOS, and `~/.local/share/com.marlin.filebrowser/logs/` on Linux. The file has mode 0600. Lines are added to it across helper starts. When the app starts a helper and the file is over 1 MiB, it first renames the file to `marlin-mount.log.old`, which replaces the one before.

No secret crosses the socket or the command line. The text inside a `MountError` must hold no credential. As a backstop, `MountError` prints through `redact`, which masks bearer tokens, the values of fields such as `access_token` and `password`, and the user and password in a URL. That is the only way the text reaches a log line, a status, or a response. `redact` masks the word that follows `token:`, `password=`, `Bearer`, `Basic`, and the like, so word an error so that its cause does not sit there.

## Testing

`cd src-tauri && cargo test -p marlin-mount` needs no kernel driver, no network, and no account. On a Mac it does not compile the Linux adapter wiring in `main.rs`. `src-tauri/crates/marlin-fuse/test-in-docker.sh` does.

- `tests/control.rs` runs the server on a real socket in a temporary folder, with a fake adapter and the core's `MemoryProvider`. The fake adapter can hold a mount or an unmount open, so the tests for two requests at once do not depend on timing. The fake adapter keeps each mount's `Vfs`, so a test makes provider calls through it and sets failures with `MemoryProvider::fail_with`.
- One test starts the built binary and checks its flags, a refused mount, and a clean exit.
- The fake adapter can call a folder a dead mount, and can refuse to clear it. Two tests cover the sweep, `deadMounts`, and error -1012 that way. A unit test in `main.rs` checks the real macOS rule on a folder, a missing path, and a symbolic link.
- `src-tauri/crates/marlin-mount/test-sweep.sh` makes a real dead mount and a live one with the spike programs in `spikes/mount-hello/`, starts the built helper, and checks that only the dead mount and the empty folder are gone. Run it on a Mac with macFUSE, both of its switches on. `test-in-docker.sh` runs it on Linux.
- `tests/client.rs` tests `Client` against the built helper: the first connect starts it, a second one reuses it, it outlives its client, it runs in its own process group, and `shutdown` stops it. A fake helper, a plain socket listener in the test, covers another protocol version, a refused shutdown, and a notification in front of an answer.
- The Tauri layer in `src-tauri/src/mount_helper.rs` has one test: the request at quit runs outside the async runtime, with no helper. Nothing drives the real app yet (#497), so the `Exit` event, the five commands, and the two events are untested.
- A signal is tested through the future that `Server::run` takes, not with a real `SIGTERM`.
- The check of the connecting user's ID has no test. It needs a second user account.
