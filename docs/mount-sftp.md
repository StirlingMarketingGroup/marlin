# SFTP mount provider (`marlin-sftp`)

`SftpProvider` shows one saved SFTP server to the [mount core](mount-core.md) as a `Provider`. It only reads: `can_write` is false, so a mount of it is read-only. Writes are #603. The [mount helper](mount-helper.md) does not link it yet: #477 does that.

The same crate holds the saved servers, the host key check, and the connect that the app's `sftp://` view uses: see [Location providers](location-providers.md).

**Covers:** `src-tauri/crates/marlin-sftp/src/provider.rs`, `open_sftp_channel` in `src-tauri/crates/marlin-sftp/src/connect.rs`, `src-tauri/crates/marlin-sftp/tests/provider.rs`
**Checked against the code:** 2026-11-23

## What a mount shows

- **The top is `/`**, not the user's home folder. A server's useful folders are often outside home, and a mount's top cannot move. The app's `sftp://` view also starts at `/`.
- **An item's id is its absolute path.** So a rename that another client makes looks like one item removed and another added ([Mount core](mount-core.md#what-a-provider-implements)).
- **Names are the server's.** A listing holds files and folders only. It leaves out `.` and `..`, FIFOs, sockets, and devices, because a read of a FIFO would hang the server. It also leaves out every name that holds U+FFFD: russh-sftp 2.1.2 decodes names lossily, so a name that is not UTF-8 and a real name with U+FFFD could get one id (#637). It decodes the server's file handles the same way, so on a server whose handles are not UTF-8 every listing and read fails until #637.
- **Attributes:** a file's size, a folder's size of 0, and the modified time to the second. There is no revision, and no file is marked read-only.

## Links

The mount core knows files and folders only. A link shows as what it points to, under its own name and path:

| The link points to                                                              | The listing shows                                                                                                         |
| ------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| A file                                                                          | A file with the target's size and time.                                                                                   |
| A folder                                                                        | A folder, only when the listed folder was reached with no link on the way, and the target is not that folder or above it. |
| A missing target, a loop, or a target the server will not describe to this user | Nothing: the link is left out.                                                                                            |

"Reached with no link on the way" means the server's `realpath` of the listed folder is its id. So a walk crosses at most one folder link, and links that point into each other's folders cannot make an endless tree. On a Mac, for example, `/tmp` is a link to `/private/tmp`: it shows in `/`, but a link to a folder inside it does not show under `/tmp`. It shows under `/private/tmp`. The rule needs a server whose `realpath` follows links, as OpenSSH's does. On a server whose `realpath` only tidies the path, a link to a folder above it shows at every depth, until the path grows too long for the server.

`item` follows links whatever the listing showed.

## The calls

- **`item`** asks for the path's attributes, following links.
- **`list`** reads the folder, then looks up its links, at most 8 at once. OpenSSH answers "no such file" for a file it is asked to list, and other servers answer a generic "failure" for a file or a missing folder. So after either answer the provider asks what the path is. A file is `NotAFolder`, and the ask's own "no such file" or "permission denied" gives the error.
- **`read`** opens the file and asks for the range in pieces, at most 8 at once, in order. A piece is the largest read the server names in `limits@openssh.com`, up to 256 KiB, or 32 KiB from a server that names none. A server may answer a piece short. The piece then asks for only the bytes it still lacks, while the pieces after it go on. The end of the file, or an empty answer, ends a piece. Memory holds the asked range plus the pieces in flight. The file is never read whole. Each `read` opens and closes the file, which costs one more round trip per read: #477 measures what that does to a copy.
- **`space`** asks `statvfs@openssh.com` about `/`. A server without that extension, or one that answers it with an error status, gets 1 PiB as both total and free: zero free would make file managers refuse to copy. The space is that of the disk that holds `/`, which can differ from the disk of a folder the user browses.
- **`MAX_CALLS` is 4.** The helper is to set a mount's `max_calls` to it (#477). The app runs one transfer at a time per server, because some servers fail on parallel reads, and one read here already keeps 8 requests going. Nobody has measured how fast either number is.

## The connection

- **Each `SftpProvider` holds one connection**, opened by the first call that needs one and kept for the calls after it.
- **An attempt to connect runs in a task of its own**, and every call that needs a connection meanwhile waits on it. The core cancels a call by dropping it. An attempt inside that call would end with it: a cancel during a keychain prompt would let the next call start a second prompt, and a cancel while the server judges a password would lose the refusal.
- **Each new connection reads `sftp-servers.json` and the keychain again**, so a changed password or key works with no unmount.
- **Every refused sign-in is remembered** with its user, for as long as the provider lives. A refused password is never sent again, because servers lock an account after a few wrong ones. A refused key or agent sign-in is tried again after `retry_refused_after`, which the helper is to set to 60 seconds (#477), because the fix is often on the server. Saving another user, password, key path, or passphrase makes a new sign-in, which is tried at once. A new key file under the same path waits out the refusal. A server that closes the connection during the sign-in has not refused it, unless it already refused an earlier agent key. A password that gets no answer within the attempt's 60 seconds is not remembered either, and the next call sends it again. An SSH agent that could not sign, as when the user declines its prompt, waits the same way as a refused agent key: otherwise the prompt would come back with the next call.
- **A connection that ends is seen at once.** The provider watches the read half of the SFTP channel. When it ends, every request still waiting ends too: russh-sftp itself would wait out its time limit.
- **A connection that ended is never used again.** A call that lost a connection that was there before the call runs once more on a new one. Every call only reads, so a repeat is safe. A second loss is `Offline`.
- **A connection that died silently**, as after a laptop sleeps, ends within 20 seconds. A task beside each connection asks the server every 5 seconds whether it is there, and ends the connection when the server has been quiet for 15 seconds: no answer to the ask, and no SFTP bytes since the ask or since the last bytes before it. Bytes count as an answer, because on a slow link the answer can queue behind file data already on its way. That is inside the core's 30-second call limit, so a call waiting on the dead connection loses it and runs again on a new one.
- **An attempt has 60 seconds** from the TCP connect to the SFTP channel, then fails with `TimedOut`, so a connection that dies during the sign-in cannot hold every call. The keychain read before it has no limit, since its prompt waits for the user.
- **russh-sftp's own limit per request is 60 seconds**, above the core's 30-second call limit, so the core's limit decides.
- **An open the core cancels before its answer** may leave a handle on the server that nothing can close. The provider then stops using that connection: it closes when the calls on it end, and a closed session frees all its handles. Every other handle is closed by a task of its own after the call.

## Errors

| What happens                                                                                                                                                     | The call returns                                                                                 |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| The saved server cannot be read, a password sign-in cannot read its password from the keychain, or a key needs a passphrase the keychain cannot give             | `SignInNeeded`                                                                                   |
| The host key is not in `known_hosts`, or does not match the saved one                                                                                            | `SignInNeeded`. The app can trust an unknown key. A changed key has no override in the app.      |
| The server refuses the sign-in, the key needs its passphrase, the key does not load, or the agent cannot sign                                                    | `SignInNeeded`                                                                                   |
| The server cannot be reached, or closes the connection during the sign-in                                                                                        | `Offline`                                                                                        |
| The connection ends during a call                                                                                                                                | `Offline`. A call whose connection was there before it first runs once more on a new connection. |
| `known_hosts` cannot be read, the server refuses SFTP or does not answer the request for it within 10 seconds, or the SFTP channel does not start in another way | `Other("the SFTP connection could not start")`                                                   |
| The server answers "no such file"                                                                                                                                | `NotFound`                                                                                       |
| The server answers "permission denied"                                                                                                                           | `AccessDenied`                                                                                   |
| The server answers any other error status                                                                                                                        | `NotSupported`: a status is about one item                                                       |
| The server sends an answer the provider cannot read                                                                                                              | `Other("the SFTP server sent an answer Marlin could not read")`                                  |
| A connection attempt, or a request, passes its 60 seconds                                                                                                        | `TimedOut`                                                                                       |

No `Other` text holds a path or a secret.

## For the helper (#477)

- Build the provider with `Config { servers: servers_path(), known_hosts: known_hosts_path(), retry_refused_after: 60 seconds }`.
- Keep one `SftpProvider` per saved server for the life of the helper, as `LinkedProviders` keeps one saved sign-in per Google account. The refused sign-ins live in the provider, so a new provider for each mount would send a refused password again.
- An agent sign-in reads `SSH_AUTH_SOCK` from the process that connects. A helper started without it answers `SignInNeeded` for an agent server.

## Testing

`cargo test -p marlin-sftp` runs the tests, and `npm run test:rust` includes them. `tests/provider.rs` needs `/usr/sbin/sshd` and `ssh-keygen`: a Mac has both, and CI installs `openssh-server`.

- **Against OpenSSH:** each test starts `sshd` as the user on 127.0.0.1, with key sign-in only, as `app-tests/sftp.test.ts` does. A TCP relay in the test sits between the provider and `sshd`, so a test can count connections and cut them all at once. The keys are an Ed25519 key, an RSA key, and a key the server does not accept. The tests cover listings, a FIFO, reads at an offset, the link rules with a walk that would never end without them, the errors, an RSA sign-in, unknown and changed host keys, a refused key and its wait, a cut connection, and a connection that goes silent, which the relay keeps open and quiet.
- **Against a server inside the test:** a russh server with password sign-in, whose SFTP server has no extensions and answers one request at a time. It covers a refused password, also when the call that sent it was cancelled, an agent that cannot sign, free space from a server without `statvfs` or one that answers it with an error, reads that come back short, a path the user may not read, a generic failure to list, a connection lost during a call, and an open cancelled before its answer.

Not tested: a sign-in with a real SSH agent, a slow link where file data holds up the answer to the provider's ask, SFTP servers other than OpenSSH and the one in the test, and a real remote server.
