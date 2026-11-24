# Location providers

Every path in Marlin is a URI, and its scheme picks a storage backend called a location provider. The provider trait covers listing and whole-item operations only. It has no way to read or write file contents, so every feature that moves bytes between two backends is written by hand for each pair. Epic #368 replaces this with native mounts.

**Covers:** `src-tauri/src/locations/`, `src-tauri/crates/marlin-smb/`, `src-tauri/crates/marlin-smb-client/`, `src-tauri/crates/marlin-sftp/`, `src-tauri/crates/marlin-local-network/`, `src-tauri/Info.plist`, the paste, drop, download, and extract commands in `src-tauri/src/commands.rs`, the `unrar` entry in `src-tauri/Cargo.toml`, `src/utils/sftpHostKey.ts`, `src/windows/SftpConnectWindow.tsx`, and `src/utils/googleSignIn.ts`
**Checked against the code:** 2026-11-24

## The model

`src-tauri/src/locations/mod.rs` defines it:

- **`LocationInput`** is what a command receives: a raw string (`/Users/me`, `smb://nas/share/x`) or an object with `scheme`, `authority`, and `path`. A string with no scheme is `file`.
- **`Location`** is the parsed form.
- **`LocationProvider`** is the trait each backend implements.
- **`REGISTRY`** maps a scheme to its provider. `resolve_location(input)` returns the provider and the parsed location.
- **`LocationCapabilities`** tells the frontend what the backend allows, so the UI can disable actions.

The trait has eight operations, and all of them take whole paths:

```rust
fn scheme(&self) -> &'static str;
fn capabilities(&self, location: &Location) -> LocationCapabilities;
async fn read_directory(&self, location: &Location) -> Result<ProviderDirectoryEntries, String>;
async fn get_file_metadata(&self, location: &Location) -> Result<FileItem, String>;
async fn create_directory(&self, location: &Location) -> Result<(), String>;
async fn delete(&self, location: &Location) -> Result<(), String>;
async fn rename(&self, from: &Location, to: &Location) -> Result<(), String>;
async fn copy(&self, from: &Location, to: &Location) -> Result<(), String>;
async fn move_item(&self, from: &Location, to: &Location) -> Result<(), String>; // defaults to rename
```

`rename`, `copy`, and `move_item` work inside one provider. There is no open, no read of a byte range, and no write. Errors are plain strings.

## The five providers

| Scheme    | Code                 | URI shape                                 | Writes | Watches for changes | How it reaches the storage                                                     |
| --------- | -------------------- | ----------------------------------------- | ------ | ------------------- | ------------------------------------------------------------------------------ |
| `file`    | `locations/file.rs`  | a plain path, or `file:///path`           | yes    | yes                 | The local filesystem, through `fs_utils.rs`.                                   |
| `archive` | `locations/archive/` | `archive:///?src=<archive>&path=<inside>` | no     | no                  | Reads ZIP, RAR, and TAR files. `src` can be another URI, nested up to 10 deep. |
| `gdrive`  | `locations/gdrive/`  | `gdrive://<account email>/<path>`         | yes    | no                  | The Google Drive REST API with OAuth.                                          |
| `sftp`    | `locations/sftp/`    | `sftp://<user>@<host>:<port>/<path>`      | yes    | no                  | `russh` and `russh-sftp`, with a connection pool per server.                   |
| `smb`     | `locations/smb/`     | `smb://<server>/<share>/<path>`           | yes    | no                  | The `marlin-smb` helper process.                                               |

Notes per provider:

- **`file`** renames in two steps when only letter case changes, so the rename works on case-insensitive disks.
- **`archive`** reads RAR with the `unrar` crate, which compiles the UnRAR C++ source. UnRAR is not open source, so `src-tauri/Cargo.toml` leaves `unrar` out of a Windows build: see rule 3 in [Licenses](licenses.md#rules-for-a-dependency). There, listing a `.rar`, reading one entry, and extracting the whole archive each return "RAR archives are not supported on Windows". Two files use the crate: `locations/archive/reader.rs` and `commands.rs`. Code that uses it goes behind `#[cfg(not(target_os = "windows"))]`, with a Windows twin that returns that error. Listing or extracting a multi-volume RAR from its first part aborts a debug build: the crate reads past the next volume's name (#586). An entry that is opened, copied, dragged out, or read as a nested archive is extracted to `marlin-archives/<hash>/<entry name>` in the temp folder. The cache drops an entry after 24 hours (1 hour at launch). Past 1 GB it drops the oldest first (`locations/archive/cache.rs`). The prune removes only what an extraction made: an entry folder whose one file is stale, or that has no file. A folder with more files holds something the user saved there, and it stays. The prune leaves an entry alone while the entry has a lock file, and takes the lock itself before it removes one, so it never cuts off an extraction. An extracted entry is dated when it is extracted, because RAR extraction keeps the archived time.
- **`gdrive`** sets `requires_explicit_refresh`, because nothing tells Marlin when a remote folder changes. `gdrive-accounts.json` holds the account list and each account's mount name. The service account has no mount name. [Mounts](mount-helper.md#mounts) says how a name is built. Each account's refresh token is in the OS keychain under the service name `marlin-gdrive`, with the email as the user. The access token is never stored: each process asks Google for its own and keeps it in memory with the refresh token, so the keychain is read once per account per run, and again on each call after Google refuses the sign-in. That code is `SavedSignIn` in the `marlin-gdrive` crate, shared with the mount helper: see [The saved sign-in](mount-gdrive.md#the-saved-sign-in). The same page says where the OAuth client ID and secret come from. `gdrive/auth.rs` keeps the account list, the browser sign-in, and the error text. The first read of an accounts file from an older version moves its tokens into the keychain, builds the missing mount names, and rewrites the file. If a keychain write fails, the file stays as it was and the next read tries again. When Google rejects an account's refresh token (`invalid_grant`) or the keychain has no entry for it, the error starts with `[GDRIVE_SIGN_IN_NEEDED]`. The frontend acts on that tag only when the failure comes from opening a folder (`refreshCurrentDirectory`): it asks the user to sign in again, opens Google's sign-in page for that account, and reads the folder again (`src/utils/googleSignIn.ts`). Every other Google Drive call shows the error text and does not ask. Every sign-in asks Google for consent again (`prompt=consent`), because Google sends a refresh token only with a consent. One sign-in runs at a time: a second one fails at once, and a sign-in that is not finished in 5 minutes fails.
- **`sftp`** downloads one file or thumbnail at a time per server (`MAX_CONCURRENT_OPS` in `sftp/pool.rs`), because some servers fail on parallel reads. Uploads and copies within a server do not wait for that limit. It accepts a server key only when `~/.ssh/known_hosts` holds it for that host (`host_key.rs` in the `marlin-sftp` crate). A host with no saved key fails the connection. The frontend asks whether to save the key when the failure comes from opening a folder (`refreshCurrentDirectory`) or from the connect window. Every other SFTP call shows the error text and does not ask (the refresh shortcut is issue #500). A host with a saved key that does not match is refused, and the app has no override. Marlin appends to the file and never rewrites a line. The app keeps each unknown key in memory (`sftp/host_key.rs`) and saves only that key, so the frontend cannot choose what gets saved. Limits: Marlin reads only that one file, not `ssh_config`, `UserKnownHostsFile`, or `/etc/ssh/ssh_known_hosts`. `russh` matches plain and hashed host names, so a host saved under a wildcard pattern, an `@cert-authority` line, or a line that separates its fields with tabs is asked about as new. An `@revoked` line is ignored. Marlin looks the host up in lower case and saves a new host under its plain name, even when `HashKnownHosts` is on. A remote folder is read twice per navigation (issue #499), so the frontend shares one answer between the two failed reads (`src/utils/sftpHostKey.ts`). Passwords are in the OS keychain.
  - The `marlin-sftp` crate holds the saved servers in `sftp-servers.json`, their keychain entries, the host key check, and the code that connects. It also holds the provider for mounts: see [SFTP mount provider](mount-sftp.md). On Linux and macOS the [mount helper](mount-helper.md#providers) opens a saved server as a read-only `sftp` mount; the app's `sftp://` views keep their own connection pool. A server that refuses SFTP fails the connection at once, and one that does not answer the request for it fails after 10 seconds. The pool and the limit of one download at a time are in the app. Every read of `sftp-servers.json` reads the file, so a change one process makes reaches every other. A save writes a new file and renames it over the old one, so a reader never sees a half-written file. On macOS and Linux a save leaves the file readable by its user only. A keychain entry is under the service name `marlin-sftp`, with the user, the host as saved, and the port as its user name (`user@host:port`). Saving a server again drops the app's open sessions to it. When the host spelling or the user changed, the old keychain entry is deleted, and the secret from that save goes under the new name. `parse_authority` reads the `<user>@<host>:<port>` part of an `sftp://` URI (the port is 22 when none is given). `server_credentials` turns the saved record into one `SignIn`: a password, a key file with its passphrase, or the SSH agent. A key's passphrase is optional, so a keychain with no entry means none. When a key needs a passphrase that is not saved, or that the keychain cannot give, the connection fails with the `[SFTP_NO_CREDENTIALS]` tag. So does a key in OpenSSH's format whose given passphrase does not open it. Opening a folder that fails with the tag opens the connect window. The window starts on the saved server's sign-in method and key path, and it clears the secret field when the method changes, so a key's passphrase is never sent as a password. Other SFTP calls show the error text. Other key errors, such as an encrypted PKCS#8 or PuTTY key with no passphrase, or a wrong passphrase for an older PEM key, show "Failed to load SSH key" and open no window. An RSA key signs with the strongest `rsa-sha2` hash the server offers, because OpenSSH 8.8 and later refuse the SHA-1 `ssh-rsa` signature. A connection that closes during sign-in is reported as unreachable, not as a refused sign-in, unless the server refused agent keys first (OpenSSH closes after `MaxAuthTries`). When the user declines the agent's prompt to sign, the sign-in stops there: russh cannot try another key on that connection. On macOS the TCP connect is tried again for up to 20 seconds while the Local Network prompt is unanswered: see [macOS Local Network access](#macos-local-network-access).
  - A paste into an SFTP folder writes only new files. `write_new_file` in `sftp/mod.rs` opens the file with `CREATE` and `EXCLUDE`, so it never overwrites a file. When a write or the close fails, it asks the server to remove the file. When the server does not remove it, the error says the incomplete file may still be there. Do not use russh-sftp's `SftpSession::write`: it opens without `CREATE`, so it fails for a new name, and it returns before the server answers the last writes (#671). An upload, which is a paste from another provider or a pasted image, takes the first free name of `name.ext`, `name (2).ext`, `name (3).ext`, and so on. Only "no such file" frees a name, and a link whose target is gone takes its name. Any other error stops the upload.
- **`smb`** is not compiled on Windows. On macOS and Linux the main app starts `marlin-smb` and sends it JSON-RPC 2.0 requests, one JSON object per line on stdin, with replies on stdout. A call has a 30-second limit, and a partial download has 5 minutes. A download, an upload, and a copy have none. The helper runs one call at a time. A call waits for the one before it with no limit, and its own limit starts when its request is sent. A call that passes its limit ends the helper process, and the next call starts a new one. Any call after the helper dies starts a new one too (`marlin-smb-client`). A caller can also pass a stop flag (`call_until`), which another thread sets. Stopped before the request is sent, the call sends nothing and the process stays. Stopped while it waits for the answer, the call ends within 50 ms, and so does its process. The [SMB mount provider](mount-smb.md) in the same crate uses it, and the mount helper runs its own `marlin-smb` processes for a mount. Passwords are in the OS keychain under the service name `marlin-smb`. The code for the saved servers and their keychain entries is in `marlin-smb-client` (`servers.rs`). Each use reads `smb-servers.json` again. Only the app writes it: a save writes a new file, readable by its user only, and renames it over the old one. At launch, and before it adds or removes a server, the app moves any password it finds in the file into the keychain. To list a server's shares, `marlin-smb` gives `smbclient` the sign-in on its stdin, never in a file. Every server lists `IPC$`, so a listing with no share line at all is an error, even when `smbclient` exits 0. The app reads a server's password from the keychain on first use and keeps it in memory until the app quits, because macOS can ask the user to allow each read. macOS ties "Always Allow" to the app's code signature. A dev build's signature is ad hoc and changes with each rebuild, so macOS asks once per server after each rebuild (issue #491). A release build keeps one signature identity across updates (see [`release-signing.md`](release-signing.md)), so macOS sees the same app after an update. `read` returns up to 16 MiB of a file from an offset (`READ_MAX`). The bytes are base64 in the JSON reply. A caller that needs more asks again from where the last read ended. `space` answers a share's total and free bytes. It answers `null` when the server reports none. For a call through libsmbclient, the error code comes from the errno it gives. `list_shares` takes its code from what `smbclient` prints: a `do_connect` failure or a transport status means the server was not reached, and `session setup failed` means it refused the sign-in, also for a locked, disabled, or expired account. A transfer that fails partway in `download_file`, `download_partial`, or `upload_file` answers the generic code. libsmbclient gives `EACCES` for a refused sign-in, a refused share or file, and, on a Mac, a LAN host that is turned off (the socket's `EHOSTDOWN`). So an `EACCES` runs `smbclient -L`, which signs in without opening a share, and takes its refused sign-in or failed connection; anything else is a refused share or file. Without `smbclient` installed, a refused sign-in reads as a refused share or file. A server that signs an unknown user name in as a guest (Samba's `map to guest = bad user`) lists its shares, so there a mistyped user name reads as a refused share or file too. A failed folder listing stats the folder to learn why: a folder that is there but cannot be listed was refused, and a path that is there and is not a folder answers `SMB_NOT_A_FOLDER`. `read_directory` marks a name that starts with a dot as hidden, and still lists it. A host may carry a port, which `list_shares` passes to `smbclient` as `-p`. An IPv6 address does not work yet (#642). `marlin-smb` hands libsmbclient each share and path inside an `smb://` URL. libsmbclient ends the path at the last `?`, drops `"` from a share name, and decodes each `%XX`. So `marlin-smb` writes a name's own `%`, `?`, and `"` as `%25`, `%3F`, and `%22`, and the server gets the name as given. A listing gives each name back as the server holds it. Samba refuses a path with `?` in it: opening, statting, or making such a name answers `EINVAL`, and so the generic code. On macOS `marlin-smb` waits for a Local Network answer before it connects to a server it has not reached yet: see [macOS Local Network access](#macos-local-network-access).

## macOS Local Network access

On macOS 15 and later, an outgoing connection to a device on the local network needs the Local Network privilege. Until the user answers the prompt, and after Don't Allow, the system can refuse the connection at once with "No route to host". Apple describes this in [TN3179](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy). A local network address is an address on the subnet of a broadcast-capable interface, such as Wi-Fi or Ethernet, plus every multicast address and `255.255.255.255`. A VPN, a cellular interface, and loopback do not count. Resolving a `.local` name needs the privilege too.

`marlin-local-network` retries a TCP connect every 250 ms for up to 20 seconds when it failed the way macOS fails a blocked one: "No route to host" to a local network address, or a `.local` name that did not resolve, in under a second. A host that is off takes seconds to fail, so it does not wait. When a name has several addresses, one blocked address is enough to wait. One attempt has one time limit for all the addresses. After the 20 seconds the error keeps the real failure and adds the Local Network setting as a possible fix. A user who clicks Allow after the 20 seconds opens the address again.

SFTP opens every TCP connection through it in `open_sftp_channel`: the app's view, the connect window, and the SFTP mount provider. `marlin-smb` waits before it connects to a server it has not reached yet, in `Share::open` and `list_shares`, and remembers the servers it reached for the life of the process. A server it could not reach is tried again on the next call. So no SMB call runs twice, and the app and the SMB mount provider are both covered. The wait counts toward the call's 30-second limit.

The prompt's text is in `src-tauri/Info.plist`. `marlin-smb` and `marlin-mount` are started by the app, so macOS uses the app's answer and text for them.

Programs started from Terminal always have access, so `npm run tauri dev`, and a built app's executable run from Terminal, never see the prompt. For a hand test, open the built `Marlin.app` from Finder. On macOS 27.2 and later, the − button in System Settings > Privacy & Security > Local Network resets an app's answer.

A NetBIOS name, which `getaddrinfo` cannot resolve, does not wait. A mount shows `Offline` and not the text.

## Google's settings for Marlin's sign-in

Marlin signs in to Google as an OAuth app in a Google Cloud project. Its settings are in Google Cloud Console, under Google Auth Platform, and no file in this repo holds them. Only the project's owner can change them.

| Setting                     | Value                    | Why                                                                                                      |
| --------------------------- | ------------------------ | -------------------------------------------------------------------------------------------------------- |
| Audience, publishing status | In production            | In "Testing", Google ends a refresh token after 7 days, so users had to sign in again every week (#167). |
| Audience, user type         | External                 | Any Google account may sign in.                                                                          |
| Branding, home page         | The repo's GitHub page   | Google will not publish an app without it.                                                               |
| Branding, privacy policy    | `PRIVACY.md` on `master` | The same. If the file moves, change the link in the Console.                                             |
| Branding, terms of service  | `TERMS.md` on `master`   | Optional for Google.                                                                                     |
| Branding, authorized domain | `github.com`             | Each link's domain must be listed.                                                                       |
| Branding, logo              | None                     | With a logo, Google asks for verification before the app may be in production.                           |

Google has not verified the app. Two things follow:

- Sign-in shows a "Google hasn't verified this app" screen. The user clicks Advanced, then continues.
- **At most 100 Google accounts can ever connect** (#533). The count covers the life of the project and cannot be reset.

## Where the model breaks

These are the reasons for epic #368. Each one is a place where code outside `locations/` has to know which backend it is talking to.

**1. Copy and move stop at the provider edge.** The `copy_file` and `move_file` commands return "not yet supported" when the two schemes differ.

**2. Paste is a hand-written table of provider pairs.** `paste_items_to_location` in `commands.rs` matches on `(source scheme, destination scheme)` and has a separate block of code for each pair. It also runs every drop, because `useDragDetector.ts` calls it.

| From ↓ / To → | `file` | `gdrive` | `smb` | `sftp` |
| ------------- | ------ | -------- | ----- | ------ |
| `file`        | yes    | yes      | yes   | yes    |
| `gdrive`      | yes    | yes      | yes   | yes    |
| `smb`         | yes    | **no**   | yes   | yes    |
| `sftp`        | yes    | yes      | yes   | yes    |

Same-scheme pairs go through the provider's own `copy`. Anything from an `archive` to another backend is not in the table. A pair that is missing returns "Pasting across providers is not supported for these locations yet". A new provider means a new row and a new column of hand-written code.

**3. Reading a remote file means downloading all of it to a temp file first.** To open a file or drag it out, the frontend picks a command by scheme: `download_gdrive_file`, `download_smb_file`, `download_sftp_file`, or `extract_archive_entry_to_temp`. A copy to the OS clipboard calls `download_and_copy_to_clipboard`, which picks by scheme in `download_single_to_temp` (`commands.rs`). Nothing streams.

Each temp copy keeps the file's exact name, in a folder of its own under the OS temp folder, made by `temp_copy_path` in `locations/mod.rs`: `marlin-sftp-downloads/<hash of the address>/`, `marlin-gdrive-cache/<file ID>/` (reused for an hour), `marlin-smb-thumbnails/<random ID>/` (a new folder for each download), and `marlin-archives/<hash of the address>/` for an archive entry. On macOS and Linux only the user can open these folders. Only a path separator and NUL are replaced, and on Windows the characters Windows forbids, control characters, and a final dot or space. A Windows device name such as `CON` gets a `_` in front. An empty name, `.`, or `..` becomes `file`. A copy from inside an archive extracts the entry the same way. A cut there is refused, because an archive cannot change.

**4. Dragging a remote file out to another app is written per provider.** `FileGrid.tsx` and `FileList.tsx` each have a branch for `gdrive`, `smb`, and `archive`. There is no branch for `sftp`.

**5. A drop is validated as a local path.** `resolve_drop_operation` expands the destination with `fs_utils::expand_path` and fails if that path does not exist on disk. It has no remote case.

**6. Thumbnails know about each provider.** `thumbnails/generators/mod.rs` has a download branch for `archive`, `gdrive`, `smb`, and `sftp`, and `thumbnails/cache.rs` builds cache identity differently for `smb` and `sftp`.

**7. Only local folders refresh on their own.** `supports_watching` is true for `file` alone.

**8. Scheme checks are spread across the frontend.** `useAppStore.ts`, `Sidebar.tsx`, `App.tsx`, `FileGrid.tsx`, `FileList.tsx`, `ContextMenu.tsx`, `StatusBar.tsx`, `MainPanel.tsx`, `useThumbnail.ts`, and `utils/pathUtils.ts` all test for scheme prefixes.

## Where this is going

Epic #368 turns each connected account or server into a native mount, so the OS sees an ordinary folder. Then local-to-remote and remote-to-remote copies, drag and drop, the clipboard, and other apps all work through normal filesystem calls, and the pair table goes away. A few provider actions stay outside the filesystem, such as a Drive copy that keeps a Google Doc a Doc. The work is split into issues #369 to #378. [Decision 0001](decisions/0001-mount-architecture.md) sets the architecture and the order of that work. Its evidence is the spike in `spikes/mount-hello/`. The helper's sweep test uses the spike's programs. The [mount core](mount-core.md) and the [mount helper](mount-helper.md) are in the repo: the helper mounts a Google account, with writes, and a saved SMB server, read-only, on Linux and macOS. Marlin's window does not route an account to its mount yet: a saved account or a provider address opens through the providers on this page. A mount opened by its own path goes through the window's local code, which lists it as a network or FUSE folder (see [Architecture](architecture.md)).

[Decision 0009](decisions/0009-window-on-mount-paths.md) sets how the window moves onto mounts, one provider at a time, and which code on this page stays. #377 lists the pieces.

## Adding a provider today

1. Add a module in `src-tauri/src/locations/` that implements `LocationProvider`.
2. Register it in `REGISTRY` in `locations/mod.rs`.
3. Add its pairs to `paste_items_to_location`.
4. Add a download command and call it from the open, clipboard, and drag-out code in the frontend.
5. Add a download branch in `thumbnails/generators/mod.rs`.
6. Add the sign-in or connect UI, and the sidebar entry.

Steps 3 to 5 are the cost that the mount work removes.
