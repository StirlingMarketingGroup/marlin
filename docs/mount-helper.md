# Mount helper (`marlin-mount`)

`marlin-mount` is the process that will own every native mount. It runs once per user, apart from the app, so a mount outlives a hot reload, an app crash, and every window. The app tells it what to mount over a local socket.

It cannot mount anything yet. No OS adapter (#374, #373, #375) and no real provider (#372) exists, so no mount request can succeed: one that passes the name checks fails with -1004, unknown provider. Nothing starts the helper yet either: the app side is #483.

**Covers:** `src-tauri/crates/marlin-mount/`
**Checked against the code:** 2026-11-12

The design is in [decision 0001](decisions/0001-mount-architecture.md), choices 3 to 6. The helper builds each mount on the [mount core](mount-core.md).

## The parts

| File          | Holds                                                                                                |
| ------------- | ---------------------------------------------------------------------------------------------------- |
| `main.rs`     | The binary: two flags, the default paths, and the signals.                                           |
| `server.rs`   | The socket, the lock that allows one helper per user, the connections, and the request dispatch.     |
| `mounts.rs`   | The mount table and its rules, the three traits that adapters and providers plug into, `MountError`. |
| `protocol.rs` | The wire types. The app will share them (#483).                                                      |
| `log.rs`      | The log writer, and `redact`.                                                                        |

The crate is Unix only. A Windows named pipe comes with the Windows adapter (#375).

## Running it

`marlin-mount [--socket <path>] [--root <path>]`

| Path             | Default                                                                                                                                     |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| The socket       | `marlin/mount.sock` under the runtime folder (`$XDG_RUNTIME_DIR` on Linux), or under the local data folder when there is none, as on macOS. |
| The mount folder | `~/Marlin`                                                                                                                                  |

- **One helper per socket.** The default socket path gives each user one helper. The helper takes a lock on `<socket>.lock` and holds it while it lives. A second helper that finds the lock taken logs that and exits with status 0. The OS drops the lock when a process dies, so a crashed helper never blocks the next one. The next one removes the socket file that was left. It removes nothing else: if the socket path holds any other kind of file, the helper fails to start and leaves the file alone.
- **Open to that user only.** The socket file and the lock file have mode 0600, and the helper drops a connection from any other user ID.
- **Stopping.** A `shutdown` request unmounts everything, removes the socket file, and exits with status 0. If an unmount fails, it returns -1007 and the helper keeps running. `SIGTERM` and `SIGINT` try every unmount, remove the socket file, and exit with status 0 even if an unmount failed.
- **Replacing a helper.** After a `shutdown` response, wait for the helper to close the connection before you start another one. The lock is free by then. A helper started earlier finds the lock taken and exits with status 0.
- **A socket file can outlive a killed helper.** If a connect is refused, start the helper and retry the connect until it succeeds.

## The protocol

JSON-RPC 2.0, one JSON object per line, the same framing as `marlin-smb`. Field names are camelCase. A line may be at most 64 KiB. A longer line, or one that is not UTF-8, closes the connection.

A request is `{jsonrpc, id, method, params?}`. The `id` must be an unsigned integer. A line that does not parse is answered with id 0, so start at 1.

| Method     | Params                      | Result                                                                |
| ---------- | --------------------------- | --------------------------------------------------------------------- |
| `hello`    | `{protocol}`                | `{protocol, version}`: the helper's protocol number and version.      |
| `mount`    | `{provider, account, name}` | The mount's status, once the mount is up. A failed mount is an error. |
| `unmount`  | `{provider, account}`       | `null`. An account with no mount is a success.                        |
| `list`     | none                        | Every mount's status, sorted by name.                                 |
| `status`   | `{provider, account}`       | One mount's status, or `null`.                                        |
| `shutdown` | none                        | `null`, then the helper exits.                                        |

A status is `{provider, account, name, mountPoint, state, error?}`. `error` is `{code, message}` and is there only when the state is `failed`.

The helper also sends notifications: `{jsonrpc, method: "mountChanged", params: <status>}`, one for every state change of every mount.

What a client must know:

- **Requests on one connection run at the same time.** Responses come back in any order, mixed with notifications. Wait for a response before you send a request that depends on it: a `mount` and an `unmount` of one account sent back to back can run in either order.
- **Notifications start after the `hello` response and are not replayed.** Call `list` after `hello`. A client that falls 64 notifications behind loses the oldest ones, and `list` is how it catches up.
- **A failed `mount` answers with an error, not a status.** The codes -1001, -1002, and -1009 are refusals: they make no entry, keep an entry that is already there, and send no notification. Any other failure leaves a `failed` entry, which keeps its name until the next attempt or an `unmount`.

**Version rules.** The protocol number is `PROTOCOL_VERSION` in `protocol.rs`.

- The first request on a connection must be `hello`. The helper answers it before it reads anything else.
- A `hello` with a valid `protocol` succeeds whatever the number is, and returns the helper's own number.
- If the client's number differs, that connection may only send `shutdown`, and it gets no notifications. This is how a newer app replaces an older helper.

**Error codes.**

| Code   | Meaning                                                             |
| ------ | ------------------------------------------------------------------- |
| -32700 | The line is not a request.                                          |
| -32601 | Unknown method.                                                     |
| -32602 | Wrong params.                                                       |
| -1001  | The mount name is not one plain folder name.                        |
| -1002  | Another account's mount has this name, ignoring letter case.        |
| -1003  | The mount point exists and is not an empty folder.                  |
| -1004  | Unknown provider.                                                   |
| -1005  | The provider could not open the account.                            |
| -1006  | The adapter could not mount.                                        |
| -1007  | The adapter could not unmount.                                      |
| -1008  | A folder could not be made or read.                                 |
| -1009  | The helper is shutting down and takes no new mount.                 |
| -1010  | `hello` must come first.                                            |
| -1011  | The protocol versions differ, and the request was not a `shutdown`. |

The protocol has no `health` request. A helper that answers `hello` is alive, and a machine that cannot mount says why in the error of a `mount`.

## Mounts

A mount is keyed by provider and account. Its mount point is the mount folder plus `name`.

**The caller sends the name.** Decision 0001 says a mount's name is built once and stored with the account. So the helper checks the name and never invents one. Nothing builds names yet: that comes with the first real account store (#372).

| State        | Meaning                                                                        |
| ------------ | ------------------------------------------------------------------------------ |
| `starting`   | The helper is opening the account and mounting.                                |
| `mounted`    | The OS shows the mount.                                                        |
| `unmounting` | The adapter is unmounting.                                                     |
| `failed`     | The mount attempt failed. The status keeps the error until the next attempt.   |
| `unmounted`  | Only in the last `mountChanged` for a mount. The mount is no longer in `list`. |

Health states for a mount that is up but unusable (`degraded`, `auth-required`, `offline`) are #481.

Rules the table keeps:

- **One operation per mount at a time.** A mount or unmount first takes a lock that belongs to the provider and account. Operations on different mounts run together.
- **Mounting twice is safe.** A mount request for an account that is already mounted returns its status, and the adapter is not called again. A live mount keeps its name, whatever name the new request asks for.
- **A failed mount can be retried.** The next `mount` tries again. An `unmount` removes the failed entry.
- **Unmounting twice is safe.**
- **A failed unmount keeps the mount.** The state goes back to `mounted` and the request gets the error.
- **An operation finishes even when its client has gone.**
- **The helper never mounts over anything.** A mount point must be absent or an empty real folder. A file, a folder with something in it, or a symbolic link fails with -1003 and is left as it was. This guards the user's files against a mistake. It is not a defense against another process of the same user.
- **Folders.** The helper opens the account before it makes any folder, so a mount the provider refuses leaves nothing behind. It makes the mount point with mode 0700. It removes it after an unmount and after a failed mount. It only ever removes an empty folder, and an empty folder at a mount point counts as the helper's own.
- **Shutdown.** From the moment a shutdown starts, a new `mount` gets -1009. If any unmount fails, `shutdown` returns -1007, the helper keeps running, and it takes mounts again: a mount that is still served must not lose its helper. The mounts that did unmount stay unmounted, so call `list`. A signal is different. It is an order to exit, so the helper unmounts what it can and exits anyway. A mount it could not unmount is then dead, and the start-up sweep (#482) will clear it.

## Adding an adapter or a provider

`mounts.rs` has the three traits:

- `Adapter::mount(vfs, mount_point)` serves a `marlin_vfs::Vfs` at an empty folder and returns a `Session` once the OS shows the mount.
- `Session::unmount()` unmounts. After an error the mount must still be served.
- `Providers::open(provider, account)` returns the `marlin_vfs::Provider` for an account. `account` names the account and is never a secret. The provider reads its own credentials from the OS keychain.

`main.rs` passes one adapter and one `Providers` value to `Server::bind`. Today both refuse everything. An adapter lives in its own crate, so `marlin-mount` implements `Adapter` for it in a few lines and the adapter crate does not depend on the helper.

## Logs and secrets

The helper writes one JSON object per line to stderr: `timeMs`, `level`, `event`, and the fields of the event. It logs its start and stop, a failed start, a failed unmount at a signal, a refused connection, a line that was too long, and each mount state change with the mount's status (provider, account, name, mount point, state, error). It logs nothing else from a request, and never file names inside a mount or file contents.

Whoever starts the helper must point stderr at a file or `/dev/null`, never at a pipe. The helper drops a log line it cannot write, but a pipe that is full and never read blocks it.

No secret crosses the socket or the command line. The text inside a `MountError` must hold no credential. As a backstop, `MountError` prints through `redact`, which masks bearer tokens, the values of fields such as `access_token` and `password`, and the user and password in a URL. That is the only way the text reaches a log line, a status, or a response. `redact` masks the word that follows `token:`, `password=`, `Bearer`, `Basic`, and the like, so word an error so that its cause does not sit there.

## Testing

`cd src-tauri && cargo test -p marlin-mount` needs no kernel driver, no network, and no account.

- `tests/control.rs` runs the server on a real socket in a temporary folder, with a fake adapter and the core's `MemoryProvider`. The fake adapter can hold a mount or an unmount open, so the tests for two requests at once do not depend on timing.
- One test starts the built binary and checks its flags, a refused mount, and a clean exit.
- A signal is tested through the future that `Server::run` takes, not with a real `SIGTERM`.
- The check of the connecting user's ID has no test. It needs a second user account.
