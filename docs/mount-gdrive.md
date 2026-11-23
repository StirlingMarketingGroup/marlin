# Google Drive mount provider (`marlin-gdrive`)

`marlin-gdrive` shows one Google account to the [mount core](mount-core.md) as a `Provider`. It reads the Drive API, and it can make a folder, rename or move an item, trash one, and write a file's bytes, so `can_write` is true. A mount takes writes when the helper gives it a spool. It also holds the code that turns a saved Google sign-in into an access token, for the mount and for the app. The app links the crate for that sign-in code. The [mount helper](mount-helper.md) links it as the provider `gdrive`: see [Providers](mount-helper.md#providers).

This is not the code behind `gdrive://` paths. The app handles those through its own location provider: see [Location providers](location-providers.md).

**Covers:** `src-tauri/crates/marlin-gdrive/`
**Checked against the code:** 2026-11-20

## The parts

| File                    | Holds                                                                                                                             |
| ----------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `src/provider.rs`       | `DriveProvider`, its five read calls, three change calls, and two byte calls, the item IDs, and the two things a caller supplies. |
| `src/upload.rs`         | The resumable upload behind the two byte calls: the session, the pieces, and the resume.                                          |
| `src/api.rs`            | `request`, the path of every request but an upload's `PUT`s: the token, the retries, and the error table. Google's JSON.          |
| `src/sign_in.rs`        | `OAuthClient`, `SavedSignIn`, and the keychain entry: see [The saved sign-in](#the-saved-sign-in).                                |
| `build.rs`              | Passes the OAuth client ID and secret to the compiler.                                                                            |
| `tests/drive.rs`        | The provider's tests. They run against a fake Drive server in the test process.                                                   |
| `tests/sign_in.rs`      | The sign-in's tests. They run against a fake token endpoint and an in-memory keychain.                                            |
| `tests/live.rs`         | One manual test against a real account: see [Checked against real Google](#checked-against-real-google).                          |
| `tests/live_changes.rs` | One manual test of the three change calls against a real account. It changes the account.                                         |
| `tests/live_uploads.rs` | One manual test of the two byte calls against a real account. It changes the account.                                             |

The provider does not sign in, read a keychain, or know about the helper. A caller gives it two things:

- **`AccessTokens`** hands out an access token for the account. When Google answers 401, the provider calls `rejected(token)`, and the token source must never hand that token out again. A token that is no longer the current one changes nothing, so every call in flight can report one stale token and cause one refresh. `SavedSignIn` implements the trait.
- **`Config`** holds the API's base address, the upload base address, and the wait before the first retry. Neither address has a slash at the end. The tests point both at the fake server.

## What a mount shows

| Folder          | Holds                                            |
| --------------- | ------------------------------------------------ |
| `My Drive`      | The account's own files.                         |
| `Shared drives` | One folder per shared drive the account can see. |

- "Shared with me", "Starred", and "Recent" are not shown. They are searches, not folders.
- Both folders have these names for every account, whatever name Google gives My Drive.
- `Shared drives` is there even when the account has none.
- **A Google Doc, Sheet, Slides file, or other Google editor file is a link file, and so is a shortcut.** See [Link files](#link-files).
- A name is the name Google gives. A link file's name has two more parts. Drive allows two files with one name in a folder, and any character in a name. The core makes the names unique and safe: see [Mount core](mount-core.md).
- A file in the trash is not listed.

## Link files

Some Drive items have no bytes to read: Google Docs, Sheets, Slides, Forms, and the other Google editor files, and shortcuts. Google refuses a plain download of them. The size Google reports for an editor file is not the length of anything a mount can read, and a shortcut has no size. A mount shows each one as a **link file**: a small web page that sends the browser to the item. [Decision 0003](decisions/0003-gdrive-link-files.md) says why, and what was rejected.

| Drive item                    | Name in the mount         | The page opens                                 |
| ----------------------------- | ------------------------- | ---------------------------------------------- |
| A Google Doc `Notes`          | `Notes.document.html`     | The Doc.                                       |
| A Google Sheet `Budget`       | `Budget.spreadsheet.html` | The Sheet.                                     |
| A Slides file `Deck`          | `Deck.presentation.html`  | The Slides file.                               |
| A shortcut `Plan` to a file   | `Plan.shortcut.html`      | The shortcut's target.                         |
| A shortcut `Team` to a folder | `Team.shortcut.html`      | The target folder, in Google Drive on the web. |

- **The name** is the name Google gives, then the type, then `.html`. The type is what follows `application/vnd.google-apps.` in the item's MIME type, up to the next `.`. So a Form is `.form.html`, a Drawing is `.drawing.html`, and a new Google type needs no code.
- **The page** is five lines. `{id}` is the item's own ID, or the target's ID for a shortcut:

  ```text
  <!doctype html>
  <meta charset="utf-8">
  <meta http-equiv="refresh" content="0; url=https://drive.google.com/open?id={id}">
  <title>Open in Google Drive</title>
  <a href="https://drive.google.com/open?id={id}">Open in Google Drive</a>
  ```

- **The size is the length of the page.** The page depends on the ID alone, so the size is known when the folder is listed, and it never changes.
- **A read of a link file sends Google no request.** The ID to open is in the item's ID: see [Item IDs](#item-ids). Opening one, and asking for its attributes, cost one `files.get` when the core holds no fresh answer, as for any file.
- **The modified time** is the item's own, as for any file.
- **A shortcut is never followed.** The provider does not ask Google about the target. A shortcut whose target is gone, or is not shared with the account, lists like any other, and its page leads to Google's error page. A shortcut to a folder is a file, so no folder is reachable through two parents.
- **A link file is a file.** Listing one returns `NotAFolder` and sends no request.
- **Remove trashes the Drive file the link is.** A Google Doc is trashed. A shortcut is trashed. The shortcut's target is a different file, and it is left as it is.
- **Rename keeps the ending.** `Notes.document.html` can become `Plan.document.html`, and Drive stores `Plan`. A name that drops the ending, such as `Notes.txt`, is `NotSupported`, and no change is sent. The provider does not turn a Google Doc into another kind of file. A duplicate link file is a known limit: see [The three change calls](#the-three-change-calls).

Not handled:

- **The page opens in the browser's default Google account.** That may not be the mount's account, and Google then shows its "You need access" page (#546).
- **A target that needs a resource key.** Google gives some older link-shared files a key that the address must carry. The page does not carry it.
- **A name near the OS limit.** The added parts can push a long name past 255 bytes, and the adapters hide such a name ([Linux mount adapter](mount-linux.md), [macOS mount adapter](mount-macos.md)).

## Item IDs

| ID                     | What it is                                                              |
| ---------------------- | ----------------------------------------------------------------------- |
| `/`                    | The top of the mount.                                                   |
| `root`                 | My Drive. `root` is Google's own name for that folder.                  |
| `/shared-drives`       | The `Shared drives` folder.                                             |
| `{file id}`            | An item in My Drive.                                                    |
| `{drive id}/{file id}` | An item in a shared drive. The drive itself is `{drive id}/{drive id}`. |
| `{either form}#{id}`   | A link file. `{id}` is the Drive ID its page opens.                     |

A Drive ID never holds `/` or `#`, so the two made-up IDs cannot be real ones, and a link file's ID cannot be a plain item's.

**Why a link file's ID holds the ID it opens.** `read` gets an item ID and nothing else, and the provider keeps nothing between calls. With the ID to open in the item ID, a read builds the page and asks Google nothing. A Google Doc `abc` in My Drive is `abc#abc`. A shortcut `abc` to `xyz` is `abc#xyz`. A shortcut's target is set when the shortcut is made, and a Google editor file stays one, so the ID stays with the item.

**For a file or a link file, `item` returns the ID it was asked for, or `NotFound`.** The provider builds an item's ID from Google's answer. If that is not the ID asked for, the answer is not the item the core holds: a plain ID whose item is a Google Doc, or a link whose shortcut has another target. The size in such an answer would not be the size of the bytes a read returns.

**Why a shared drive's ID is in the item's ID.** Google lists a folder of a shared drive in full only when the request names that drive (`corpora=drive` and `driveId`). Without them, Google promises only the files the user has opened, and a listing can come back short with no error. The provider keeps nothing between calls, so the ID has to carry the drive.

What follows from that:

- A file keeps its ID when it is renamed, or moved inside one drive, so the core keeps its node and its open handles.
- A listing gives each child the drive of the folder listed. A folder's children are always in the folder's own drive.
- A file moved between My Drive and a shared drive gets a new ID, with the drive of the folder it moved to. A move this mount makes returns that id, and the core keeps the node. A move that only shows up in a later listing still looks like one item removed and one added.

## The five calls

| Call    | Requests                                                                                                              |
| ------- | --------------------------------------------------------------------------------------------------------------------- |
| `root`  | None.                                                                                                                 |
| `item`  | One `files.get`, or one `drives.get` instead for the top folder of a shared drive. None for `/` and `/shared-drives`. |
| `list`  | `files.list`, 1,000 items a page, until Google sends no next page. `drives.list` for the shared drives, 100 a page.   |
| `read`  | One `files.get` with `alt=media` and a `Range` header for exactly the bytes asked for. None for a link file.          |
| `space` | One `about.get`.                                                                                                      |

- **Listing the top asks Google for My Drive.** So a sign-in that is gone shows at the top of the mount, not as an empty folder.
- **An empty listing costs one more request.** Google answers a listing of a file, and of a folder that is gone, with an empty list and no error. When a listing comes back empty, the provider asks for the folder itself and returns `NotFound` or `NotAFolder` when that is the truth.
- **A read never downloads a whole file to serve a part.** If the server ignores the range and sends the whole file, the provider skips to the offset, keeps the bytes asked for, and drops the connection. A read at or past the end returns no bytes.
- **Free space with no limit.** Google gives some Workspace accounts no storage limit. The provider then reports 1 PiB free, because a mount that reports 0 free makes file managers refuse to copy.
- **A shared drive's top folder is asked from the drive, not from `files.get`.** On a real account `files.get` gives that folder another name than the drive's. So `item` asks `drives.get`, and its answer equals the drive's line in the `Shared drives` listing.
- **A shared drive's modified time** is the time the drive was made. Google gives a shared drive no other time.
- The five read calls send only GET. The change calls also send POST and PATCH, and the byte calls PUT. A token refresh sends a POST to Google's token endpoint.

## The three change calls

`create_folder`, `remove`, and `rename`. A mount calls them when it has a spool.

| Call            | Requests                                                                                                                                                                                                                                                                                                                                              |
| --------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `create_folder` | `files.generateIds` for one id, then `files.create` (`POST /files`) with that id, the folder type, and the parent's Drive id.                                                                                                                                                                                                                         |
| `remove`        | `files.get` to check the item is still in the folder. `files.list`, one item a page, when it is a folder. Then `files.update` (`PATCH`) with `trashed: true`.                                                                                                                                                                                         |
| `rename`        | `files.get` of the item. When a folder moves to another folder, `files.get` of each folder above the destination, up to the folder it leaves or the top. Then one `PATCH` with the name, plus `addParents` and `removeParents` when the folder changes. A `replace` adds the same checks as `remove` before the `PATCH`, and its trash `PATCH` after. |

These ids are not ordinary Drive files. The provider answers `AccessDenied` for them before it sends anything. An `Allowed` entry sends the usual requests:

| Call                                | `/`            | `/shared-drives` | `root` (My Drive)       | `{d}/{d}` (a shared drive's top folder) |
| ----------------------------------- | -------------- | ---------------- | ----------------------- | --------------------------------------- |
| `create_folder` with it as `parent` | `AccessDenied` | `AccessDenied`   | Allowed. Parent `root`. | Allowed. Parent `{d}`.                  |
| `remove` of it                      | `AccessDenied` | `AccessDenied`   | `AccessDenied`          | `AccessDenied`                          |
| `rename` of it                      | `AccessDenied` | `AccessDenied`   | `AccessDenied`          | `AccessDenied`                          |
| `rename` with it as `to`            | `AccessDenied` | `AccessDenied`   | Allowed.                | Allowed.                                |

A link file as the parent of `create_folder`, or as `to`, is `NotAFolder`.

**The item is still in the folder.** Before a remove or a rename, the provider reads the file with `parents` and `driveId`. The item is `NotFound` when it is trashed, when `driveId` is not the drive in the id (none in My Drive), when the answer would build a different id, or when `parents` does not hold the folder. `parents` holds My Drive's real id, never `root`, so a check against the top of My Drive also reads `files.get` of `root` at the same time. Nothing is kept between calls.

**A folder that holds anything is `NotEmpty`.** Drive would trash the folder and everything in it, and the core asked for one item. The check lists children that are not trashed, one a page, and follows a next page even when a page is empty. An empty first page is not an empty folder. The check and the trash are two requests. Another client can add a child between them, and Drive then trashes it with the folder. It stays in Drive's trash.

**Remove trashes. It never deletes.** The request is `PATCH` with `{"trashed": true}`. A link file trashes its own Drive file. An item the account may not trash, such as another user's file in My Drive, gets Google's 403, so the call is `AccessDenied`.

**Rename with `replace` moves first, then trashes.** Drive allows two items with one name, so the name is never left with none. If the trash fails, both items are left and the call returns that error. A `replace` that is not in the destination is `NotFound`. A folder to replace that holds anything is `NotEmpty`. In both cases the call changes nothing.

**A file can move between My Drive and a shared drive.** The id the call returns carries the drive of the destination folder, as a listing of that folder would. A folder cannot. Google refuses to move a folder into a shared drive, and the items inside it would keep ids that still name the old drive. That move is `NotSupported`, and no change is sent.

**A folder cannot move into itself, or into a folder inside it.** Google answers that with a 400 that looks like any other 400, so the provider walks the parents from the destination until it reaches the folder, reaches the folder it is leaving, or runs out of parents.

**Retries.** `create_folder` gets one id from `generateIds` per call and sends it on every try of the POST, so a retry after a 5xx cannot make a second folder: if the first try made it, Google answers 409, and the provider reads the folder by that id. A dropped connection is `Offline`, and a new call gets a new id. The core lists the parent before a new `mkdir`, so it finds a folder an earlier call made. A move whose answer was lost is read back. Google may refuse a resent move, because the item already has that parent ("duplicate parent", a 400). So a failed move counts as done when the item is already in the destination under the name that was sent. A plain rename is read back the same way. Only `Other` and `Offline` are read back: a server error, a rate limit that outlasted the retries, a status the table does not name, a dropped connection, or an answer that cannot be read.

**A duplicate link file cannot be moved under its shown name.** The core shows the second `Notes` Doc as `Notes.document (2).html`, with ` (2)` before `.html`. That name does not keep the `.document.html` ending, so the rename is `NotSupported`, and no change is sent.

## The two byte calls

`create_file` and `replace_bytes`. Both upload through a resumable session, and open a new one when Google ends it or limits requests (`src/upload.rs`).

| Call            | Requests                                                                                                                                                                             |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `create_file`   | `files.generateIds` for one id. `POST {upload}/files?uploadType=resumable` with the id, the name, the parent, and `modifiedTime`. Then `PUT`s of the bytes.                          |
| `replace_bytes` | `files.get` to check the revision. `PATCH {upload}/files/{id}?uploadType=resumable` with `modifiedTime` and the file's `mimeType`. Then `PUT`s, with the same check before the last. |

**A file's revision is `headRevisionId`.** Drive changes it only when the bytes change. `version` also changes on a rename or a share, so it is not used. A folder has no revision, and neither has a link file.

**The modified time** goes to Google as `modifiedTime`, in UTC with milliseconds.

**A replace keeps the file's type.** It sends the `mimeType` its check read. Without it, Drive guesses the type again from the new bytes: on a real account a `.txt` file replaced with bytes that start like a PNG became `image/png`. A new file sends no type, and Drive chooses one: a `.txt` file of letters was stored as `text/plain`. Every later replace keeps that type, also after a rename to another ending.

**The session.** Opening it uploads nothing, so it goes through the same retries as every other request ([Errors and retries](#errors-and-retries)). It asks for the same fields as `item`, and the answer to the last `PUT` carries them. Google sends the session's address in `Location`.

**The `PUT`s.** Google takes any length on the request that ends the file, and only a multiple of 256 KiB on every other one. One request carries at most 16 MiB. A failed request is sent again, and the core counts only bytes past the highest count it heard ([Mount core](mount-core.md)), so 16 MiB bounds what a resend sends before the count moves. On a link slower than about 2 Mbit/s that still takes longer than the core's minute without progress, and the core ends the try and sends the file again later.

- **A new file** of up to 16 MiB goes in one request. A longer one goes in 16 MiB requests and then the rest.
- **A replace** keeps the last 1 byte to 256 KiB for a request of its own, so its revision check runs just before Google applies the new bytes. Every request before it carries whole 256 KiB pieces.
- **An empty file** is one `PUT` with no `Content-Range`.

While a request runs:

- **Memory.** The body is read from the local file one 256 KiB piece at a time.
- **Progress.** The provider reports the bytes Google already holds plus the bytes of this request read so far, every 250 ms and when the request ends. A question reports 0, which the core ignores, because it keeps the highest count it heard.
- **The size** is the local file's length when the call starts. A file that is cut during the call fails it with `Other("Marlin could not read the file it uploads")`, with no retry. The core sends the file again after the edit.

**A failure waits as `request` does, then goes on in one of two ways.**

| Failure                                                                                                 | Then                                                                                                                                                                                                                                                                                                                                                                                                           |
| ------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| A 500 to 599, a dropped connection, an answer whose body did not arrive, or a 308 that kept no new byte | The provider asks what Google holds: `PUT` with `Content-Range: bytes */{size}`. `Range: bytes=0-{n}` in the 308 means Google holds `n + 1` bytes, any number, and no `Range` means none. The next `PUT` starts there. A 200 or 201 is the finished file. An empty file is sent again instead: for it the question ends the file (Google answered it 200 on a real account), with no revision check before it. |
| A 429, a rate-limit 403, or a 404 or 410 (Google ended the session)                                     | Google's guide starts an upload again after a 4xx. The provider opens a new session with the same metadata, so a create keeps its pre-made id, and sends from the first byte.                                                                                                                                                                                                                                  |

The fourth failure in a row ends the call. Only bytes past the most Google has held in the call reset the count. The call ends with `Offline` for a connection, `Other("Google Drive ended the upload")` for a lost session, or the `Other` that `request` gives for the status. A new session on a large file can pass the core's minute without progress before it gets back to its old count. The core then ends the try as stalled and sends the file again later.

**A 401** rejects the token, and the same request goes again with a new one, through the same loop, so the revision check below runs first. A second 401 in a row is `SignInNeeded`. `storageQuotaExceeded` is `NoSpace`, and any other 403 is `AccessDenied`. Both end the call.

**The revision check runs before every request that ends the file,** a resend included. Google applies the new bytes only when the last byte arrives. So another client's change can be overwritten only between that `files.get` and the last piece, and not during the whole upload. A revision that is not the one the core based the bytes on is `Conflict`, and the rest is never sent. Google drops the unfinished session. The check before the upload means a file that already changed costs no upload.

**A 409 means this call's file already landed.** Google sends it only when the pre-made id is in use. `create_file` then returns `files.get` of that id, as `create_folder` does. The id lives for one call: a later call of the core gets a new one (#632). Drive allows two files with one name, so a taken name is never `Conflict`. On a replace's session a 409 is `Conflict`, as from `request`.

**What a call refuses before it opens a session.** `replace_bytes` learns the kind and the revision from the `files.get` of its check. The others send nothing.

| Call            | `/`, `Shared drives` | A link file    | A folder    | A file with no revision |
| --------------- | -------------------- | -------------- | ----------- | ----------------------- |
| `create_file`   | `AccessDenied`       | `NotAFolder`   | Allowed     |                         |
| `replace_bytes` | `IsAFolder`          | `NotSupported` | `IsAFolder` | `NotSupported`          |

A link file has no bytes on Drive to replace. A mount still shows it, and a file the account may only view, as writable (#631).

## Errors and retries

These are the rules of `request`, which every request but an upload's `PUT`s takes. The `PUT`s have their own: see [The two byte calls](#the-two-byte-calls).

| Google answers                                                     | The call returns                                                                                                                                                     |
| ------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Nothing, or the connection drops                                   | `Offline`                                                                                                                                                            |
| 401                                                                | One more try with a new token. A second 401 is `SignInNeeded`.                                                                                                       |
| 403 with the reason `rateLimitExceeded` or `userRateLimitExceeded` | Retries, then `Other("Google Drive is limiting requests")`.                                                                                                          |
| 429                                                                | The same.                                                                                                                                                            |
| 403 with the reason `storageQuotaExceeded`                         | `NoSpace`, with no retry.                                                                                                                                            |
| 403 with any other reason                                          | `AccessDenied`                                                                                                                                                       |
| 404                                                                | `NotFound`                                                                                                                                                           |
| 409                                                                | `Conflict`, with no retry. Google sends it only when a create's pre-made id is already in use. `create_folder` and `create_file` then return the item that id names. |
| 500 to 599                                                         | Retries, then `Other("Google Drive answered <status>")`.                                                                                                             |
| Any other status                                                   | `Other("Google Drive answered <status>")`, with no retry.                                                                                                            |
| A body that is not the expected JSON                               | `Other("Google Drive sent an answer Marlin could not read")`                                                                                                         |

- **Retries.** A request is sent again at most 3 times. A change call sends several requests, and each has its own retries. The waits are 0.5, 1, and 2 seconds, each with up to half as much again added at random, so calls that failed together do not come back together. A failed connection is not retried: the helper counts those toward `offline` ([Health](mount-helper.md#health)).
- **The core's time limit covers the retries.** The core cancels a call after 30 seconds by dropping it, and that stops a wait or a request in flight. A byte call has no such limit: the core stops it when its byte count has not grown for a minute.
- **Every failure to reach Google is `Offline`.** The only time limit on the HTTP client is 10 seconds to connect, so a timeout there also means Google could not be reached. `TimedOut` comes from the core alone.
- **The text of an `Other` is one of the three in this table, the two an upload adds ("Marlin could not read the file it uploads" and "Google Drive ended the upload"), or the one from the sign-in:** "Marlin could not use the saved Google sign-in". No file name, ID, address, token, or message from Google is in it, so the helper logs it ([Logs and secrets](mount-helper.md#logs-and-secrets)).
- The errors follow the rules on `VfsError` in `marlin-vfs/src/error.rs`: `Other` is a failure of the remote, never of one item.

## The saved sign-in

The app signs in to Google in a browser and saves the result. `src/sign_in.rs` reads what it saved. The app and the mount helper both use this code, so there is one implementation. Nothing here opens a browser.

**`OAuthClient`** is the client ID and secret that Google knows Marlin by, and the address of Google's token endpoint. `OAuthClient::load()` looks in three places, in this order:

1. The environment variables `GOOGLE_CLIENT_ID` and `GOOGLE_CLIENT_SECRET` when the program runs.
2. The same two variables when the crate was compiled. `build.rs` reads them from the environment of the build, or from `src-tauri/.env`, and hands them to the compiler. A build script reaches only its own crate, so this one must be in the crate that reads the values.
3. A `google-credentials.json` file: in `<config dir>/marlin/`, beside the program, or in the app bundle's `Resources`. A debug build also looks in the working folder and in `src-tauri/`.

`.env` is not in the repo, and the release workflow sets neither variable (#536). Cargo rebuilds the crate on every build while `src-tauri/.env` does not exist, because the build script watches that file (#466).

**The keychain entry.** Each account's refresh token is in the OS keychain: service `marlin-gdrive` (`KEYRING_SERVICE`), user = the account's email. `keyring_entry` opens it. The app writes and deletes it. `SavedSignIn` writes it only when Google sends a new refresh token.

**`SavedSignIn`** is one account's sign-in in one process. It is in one of three states (`Saved`): nothing read yet, a refresh token held, or a refresh token Google refused. `valid_token` returns an access token and refreshes it when it is missing or ends within 5 minutes.

- The refresh token is read from the keychain once and kept in memory. The access token is in memory only. A failed refresh keeps the refresh token, so it causes no second keychain read: macOS can ask the user on each read.
- One lock is held across the keychain read and the request to Google, so calls that arrive together cause one of each.
- **After Google refuses the refresh token** (`invalid_grant`), the next call reads the keychain again. The app is the only process that signs in, and it overwrites the entry, so a mount recovers with no restart. While the entry still holds the refused token, a call fails at once and sends nothing to Google: a mount gets a call for every file the OS touches.
- When the keychain has no entry, each call reads it again, for the same reason.
- The keychain read is a blocking call inside an async one. On macOS a permission prompt holds the calling thread until the user answers (#543 checks this).

| What happens                                       | `valid_token` returns | Through `AccessTokens`                                   |
| -------------------------------------------------- | --------------------- | -------------------------------------------------------- |
| The keychain has no entry                          | `NoSavedSignIn`       | `SignInNeeded`                                           |
| Google answers `invalid_grant`                     | `Rejected`            | `SignInNeeded`                                           |
| No answer in 30 seconds, or the connection fails   | `Offline(text)`       | `Offline`                                                |
| The keychain cannot be read                        | `Other(text)`         | `Other("Marlin could not use the saved Google sign-in")` |
| Any other status, or an answer that is not a token | `Other(text)`         | The same.                                                |

No text holds a token, the client secret, or any part of Google's answer: only the status. `OAuthClient` prints without its secret. `OAuthClient::load()` names a credentials file it cannot parse by path, line, and column, and quotes nothing from it.

The app's side is in `src-tauri/src/locations/gdrive/auth.rs`: the account list, the browser sign-in, the service account, and the error text the user sees. See [Location providers](location-providers.md).

## Google's limits

Google has two sets of limits for the Drive API. A Google Cloud project that used the API between November 2025 and April 2026 keeps the limits it had. A project made on or after 2026-05-01 gets limits in quota units. A project's Quotas page in the Google Cloud Console shows the ones it has. The numbers are from [Usage limits](https://developers.google.com/workspace/drive/api/guides/limits) and from the Quotas page of Marlin's own project, both read on 2026-11-16.

- **The project is the OAuth client.** Every user of a build shares the limits of the project its client ID belongs to.
- **Google plans to charge a project that passes a daily limit.** Its page sets 400,000,000 quota units a day and 1 TB a day of downloaded bytes before charges apply, and says that billing starts "later in 2026", with 90 days' notice. It does not say whether that covers a project on the older limits.

### Marlin's own project counts requests

It keeps the older limits. Its Quotas page shows:

| Limit                         | Requests |
| ----------------------------- | -------- |
| One user, one minute          | 12,000   |
| The whole project, one minute | 12,000   |

- **Its two limits in quota units are "Unlimited".** So the unit costs below do not bind it.
- **One mount does not reach the limit at the measured speed.** 12,000 a minute is 200 requests a second. One request to Google takes a quarter to half a second ([Checked against real Google](#checked-against-real-google)), so the core's 16 calls at once make 32 to 64 requests a second. A mount would need 80 ms a request to reach 200.
- **The project's limit is the same number as one user's.** Four to seven mounts under full load at the same moment, on any machines, would pass it: four at a quarter second a request, seven at half a second.

### A project made on or after 2026-05-01 counts quota units

A build with its own OAuth client from a newer project gets these limits.

| Provider call | Request                                                      | Units                                                                            |
| ------------- | ------------------------------------------------------------ | -------------------------------------------------------------------------------- |
| `item`        | `files.get`, or `drives.get` for a shared drive's top folder | 5                                                                                |
| `list`        | `files.list` or `drives.list`                                | 100 a page. A folder that lists no items costs one `item` request more, 5 units. |
| `read`        | `files.get` with `alt=media`                                 | 200, however many bytes it asks for                                              |
| `space`       | `about.get`                                                  | 5                                                                                |
| A new token   | A POST to Google's token endpoint                            | None. It is not a Drive API request.                                             |

Google's quota units for the change calls and the byte calls are not checked.

| Limit                         | Units       |
| ----------------------------- | ----------- |
| One user, one minute          | 325,000     |
| The whole project, one minute | 1,000,000   |
| The whole project, one day    | 400,000,000 |

- **Reads are what reach a unit limit.** One user's minute holds 1,625 reads, about 27 a second. The whole project's minute holds 5,000.

### What this means for a mount

**The round trip is the first limit a user meets.** Linux asks for a file about 128 KiB at a time, and each piece is one `read`. At a quarter to half a second a request, a program that waits for each piece before it asks for the next reads 256 to 512 KiB a second. Nobody has measured a real stream: the kernel can ask for more than one piece at a time. #538 tracks reading in bigger pieces.

**Past a minute's limit of either kind,** Google answers 403 or 429, and the provider waits and retries: see [Errors and retries](#errors-and-retries).

The [mount core](mount-core.md) runs at most 16 provider calls at once and ends a call after 30 seconds. Both defaults fit Drive.

- **16 calls at once is not a rate.** It stays under the request limit of Marlin's own project at the measured speed (above). It does not hold a limit in units: sixteen reads that each take half a second are 32 reads a second, over a newer project's 27. A lower number would slow every mount and still not hold a rate. The fix is fewer, bigger reads (#538).
- **30 seconds covers the retries of one request.** The three waits add up to 5.25 seconds at most. A change call sends several requests one after another, so a rate limit that lasts can end one as `TimedOut` partway: see [The three change calls](#the-three-change-calls). A listing is the long call: every page of a folder, one request after another, inside one call. How long a page of 1,000 items takes is not measured.
- **A listing kept for 5 seconds** costs at most 12 listings a minute for a folder that stays open.
- **Attributes kept for 5 seconds** cost at most 12 `files.get` a minute for a file that a program keeps asking about. After a listing, the attributes of its items cost nothing for those 5 seconds.
- **Free space kept for 30 seconds** costs at most 2 `about.get` a minute.

## Checked against real Google

`tests/live.rs` is a manual test on a real account, with the five read calls only. It reads the real keychain, it never changes anything, and it prints counts, sizes, times, and the type words of link files, but no name. Its header has the command. It ran on 2026-11-16 against one Google Workspace account with two shared drives:

- **A read at the end of a file, and past it, returns no bytes.**
- **A read at an offset returns the same bytes as that part of a read from the start.**
- **`files.get` gives the top folder of a shared drive another name than the drive's.** So `item` asks `drives.get` for that folder, and on this account its answer equals the drive's line in the `Shared drives` listing: see [The five calls](#the-five-calls).
- **Google accepts the field list with `shortcutDetails(targetId)`,** in a listing and in `files.get`. The top of My Drive listed 150 items, 88 of them link files: Google Docs, Google Sheets, and shortcuts.
- **A real shortcut carries its target, in a listing and in `files.get`.** The ID of a shortcut held a target that is not the shortcut itself. `item` of a Google Sheet and of that shortcut each returned the line from the listing.
- **A link file opens its item in the browser.** The helper mounted the account on macOS through macFUSE. The top of My Drive showed the same 150 entries, 88 of them link files. One Google Doc's link file had the same size from `stat` as the bytes read, and Firefox showed the Doc within two seconds of opening it. The page for a public Google Sheet, in a normal folder, opened the Sheet the same way.
- **One request takes a quarter to half a second.** In six runs, a listing of 150 items took 287 to 512 ms, and a read of 14 KiB took 254 to 528 ms.

`tests/live_changes.rs` is a manual test of the three change calls. It changes the account: it makes a folder `Marlin live test {time}` at the top of My Drive, works inside it, moves one folder to the top of My Drive and back, and trashes the test folder at the end. It never touches a shared drive. Its header has the command. It ran on 2026-11-20 against the same account:

- **A listing right after a change holds the change.** Five folders were made one after another, and the listing sent right after each one held it, 5 of 5. A listing right after a rename, a remove, and a move each showed the change. So the core needs no list of its own recent changes to lay over a new listing (risk 3 of [decision 0005](decisions/0005-mount-writes.md)). One run on one account is not a promise from Google.
- **Google answers 409 to a second create with the same pre-made ID,** and the first create gets 200. That is the answer `create_folder` relies on to never make a folder twice.
- **`root` works in `addParents` and `removeParents`.** A folder moved to the top of My Drive and back.
- **A link file renames and trashes.** An empty Google Doc made in the test folder listed as `….document.html`, renamed with its ending kept, and went to the trash.
- **The refusals the provider makes itself held:** a folder into its own subfolder, a folder with a child, and a link file renamed to another ending. A `replace` left one folder under the name, with the moved folder's ID.
- **A change and the listing after it took about one second.** Five creates, each with its listing, took 4.8 s. A rename and its listing took 1.0 s, a move and two listings 1.7 s. The first call of the run, which read the keychain and got a token, took 13 s.

`tests/live_uploads.rs` is a manual test of the two byte calls. It changes the account: it makes a folder `Marlin live upload test {time}` at the top of My Drive, uploads a 100 KiB text file, an empty file, and a 200 MiB file, replaces the text file twice (the second time with nothing) and the 200 MiB file once, and trashes them. It also lists the text file's revisions, downloads the first, and marks it `keepForever`, for risk 4 of [decision 0005](decisions/0005-mount-writes.md). It checks that a replace keeps the modified time it sends and the file's type, and asks one more session how it answers the question for an empty file. The trashed 200 MiB files count against the account until they leave the trash. Its header has the command, and says how to clean up after a failed run without touching anything else in the trash. It ran three times on 2026-11-20 against the same account:

- **A replace kept the file's type.** A `.txt` file replaced with bytes that start like a PNG stayed `text/plain`: see [The two byte calls](#the-two-byte-calls).
- **Drive keeps the overwritten bytes as a revision** (risk 4 of decision 0005). After a replace, the first revision was still listed, and its download returned the old bytes, before and after it was marked `keepForever`.
- **The window of risk 4 is about one second.** From the first progress report at or past the start of a 200 MiB replace's last 256 KiB to the answer took 1.0 to 1.1 s. That time holds the revision check and the last piece. It is an upper bound: it also holds the wait for the answer to the request before.
- **A 200 MiB upload took 7 to 10 seconds,** 20 to 29 MiB/s. The largest sampled growth of the process while it ran was 512 KiB. For a new 200 MiB file, the answer came 0.6 to 0.9 s after the count reached the last 256 KiB, well inside the core's minute. Three samples of a replaced 200 MiB file read back as the bytes sent.
- **The modified time a call sends is the one Drive keeps,** for a new file and for a replace, to the millisecond.
- **A stale revision was `Conflict`,** and the file kept its bytes. A replace with an empty file left 0 bytes and a new revision.
- **The question `bytes */0` ends an empty file.** Google answered 200 with the file. So the provider never asks about an empty file.
- **A replace of 150 KiB took 1.4 to 1.8 s:** its check, the session, the check before the last piece, and the one request.

Not checked live: a change in a shared drive, a file moved between drives, and what Google answers a folder moved into its own subtree (the provider refuses it before Google sees it).

Every automated test runs against the fake server, so these still rest on Google's documentation:

- That `corpora=drive` with `driveId` lists every child of a folder in a shared drive. The live test lists a shared drive, but it cannot tell whether the listing is complete.
- That every child of a folder is in the folder's own drive.
- That Google answers a read of an empty file with 416. The account had no empty file at the top of My Drive.
- **An upload that fails partway.** `tests/live_uploads.rs` uploads with no failure. What a session answers after a dropped connection, a 308 with no `Range`, a new session after a 429, a 409 on a session, and an upload into a shared drive rest on Google's guide.
- How long a page of 1,000 items takes, and so how big a folder can be before its listing passes the core's 30 seconds.
- **A dead connection after sleep.** The HTTP client's only time limit is 10 seconds to connect. After a laptop sleeps or changes network, a kept connection can be dead, and each call on it may hang until the core's 30 seconds. An upload on it may hang until the core's minute without progress.
- **Link files in another browser or on another OS.** Two were opened as a double click does, on macOS with Firefox: see above. Nobody has tried another browser or another OS, or looked at what Quick Look or a thumbnailer does with one. A Doc, a Slides file, a folder, and a shortcut's target are not checked that way. The account had no Slides file, Form, or third-party shortcut at the top of My Drive, so the names of those rest on Google's list of MIME types.
- **A 403 that covers the whole account.** Any 403 that is not a rate limit or a full drive is `AccessDenied`, which the helper counts as an answer. If Google answers the free-space request that way, a mount of such an account fails at its first call with -1005. A reason such as `domainPolicy` that starts while a mount is up leaves it `mounted` while every call fails.
- **Two 30-second limits.** A token refresh times out after 30 seconds, and so does the core's call. A slow token endpoint may show as `TimedOut` and never as `Offline`. An upload has no 30-second limit: the core ends it after a minute without progress.
- **A new token after a 401.** If Google hands back the same access token after a 401, the second 401 is `SignInNeeded`.

## Testing

`cd src-tauri && cargo test -p marlin-gdrive` needs no network and no account. A plain `cargo test` does not include it.

- Each test starts a `wiremock` server on a free local port. A provider test points `Config::api` at it, and a sign-in test points `OAuthClient::token_uri` at it.
- The sign-in tests use the in-memory keychain from the `marlin-test-keychain` crate (see [Testing](testing.md)), and one email per test. They cover a refused refresh token, a missing entry, recovery after each with the same `SavedSignIn`, a failed connection, an answer that repeats the secrets, and eight calls at once.
- The test token source hands out `token-1`, then `token-2` after `token-1` is rejected. One test holds eight calls until all have `token-1`, and checks that they cause one new token, not eight.
- Five tests put the provider under a real `Vfs`: one for duplicate names and a `/` in a name, one for a rename seen on the remote, one for link files that share a name, one for a file moved into a shared drive (it keeps its node), and one for a duplicate link file moved under its shown name.
- The link file tests cover the name and the ID of each kind, the size in a listing against the bytes a read returns, `item` of a shortcut, a read in parts, a shared drive, a shortcut whose target is gone or missing from the answer, and a read that sends the server nothing.
- The change-call tests cover creating a folder, trashing, renaming, moving, the refusals, a resent create, and a move whose answer was lost. They check the request, not only the result.
- The byte-call tests cover a new file, a replacement, the pieces, a resume from an even and an uneven offset, a dropped connection, a body cut off after a finished upload, a lost session, the revision check before a first send and a resend of the last request, and the refusals. The tests of a dropped connection, a cut-off answer, and a local file cut during an upload serve the session from a small HTTP server in the test, because wiremock cannot drop a connection partway through a body. They check the request, not only the result.
- One test checks that the five read calls send only GET.
- The helper's tests mount this provider through FUSE against a fake Google: see [Mount helper](mount-helper.md#testing).
- `tests/live.rs`, `tests/live_changes.rs`, and `tests/live_uploads.rs` are marked `#[ignore]`. Never run this crate's tests with `--ignored` or `--include-ignored`: that runs all three, and `live_changes.rs` and `live_uploads.rs` change the account. To run one, use the command in its header.
