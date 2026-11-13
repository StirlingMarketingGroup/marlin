# Google Drive mount provider (`marlin-gdrive`)

`marlin-gdrive` shows one Google account to the [mount core](mount-core.md) as a `Provider`. It reads the Drive API and never changes anything. Nothing links it yet: the [mount helper](mount-helper.md) gets it in #529, after the helper can sign in (#525).

This is not the code behind `gdrive://` paths. That is the older provider in the app, described in [Location providers](location-providers.md), and it stays as it is until #377.

**Covers:** `src-tauri/crates/marlin-gdrive/`
**Checked against the code:** 2026-11-13

## The parts

| File              | Holds                                                                                                |
| ----------------- | ---------------------------------------------------------------------------------------------------- |
| `src/provider.rs` | `DriveProvider`, its five `Provider` calls, the item IDs, and the two things a caller supplies.      |
| `src/api.rs`      | `get`, the one path every request takes: the token, the retries, and the error table. Google's JSON. |
| `tests/drive.rs`  | The tests. They run against a fake Drive server in the test process.                                 |

The crate does not sign in, read a keychain, or know about the helper. A caller gives it two things:

- **`AccessTokens`** hands out an access token for the account. When Google answers 401, the provider calls `rejected(token)`, and the token source must never hand that token out again. A token that is no longer the current one changes nothing, so every call in flight can report one stale token and cause one refresh. #525 implements the trait on the app's saved sign-in.
- **`Config`** holds the API's base address and the wait before the first retry. The tests point the address at the fake server.

## What a mount shows

| Folder          | Holds                                            |
| --------------- | ------------------------------------------------ |
| `My Drive`      | The account's own files.                         |
| `Shared drives` | One folder per shared drive the account can see. |

- "Shared with me", "Starred", and "Recent" are not shown. They are searches, not folders.
- Both folders have these names for every account, whatever name Google gives My Drive.
- `Shared drives` is there even when the account has none.
- **Google Docs, Sheets, Slides, and the other Google editor files are left out, and so are shortcuts.** They have no bytes to read. A folder that holds only those looks empty. #527 decides how to show them.
- A name is the name Google gives. Drive allows two files with one name in a folder, and any character in a name. The core makes the names unique and safe: see [Mount core](mount-core.md).
- A file in the trash is not listed.

## Item IDs

| ID                     | What it is                                                              |
| ---------------------- | ----------------------------------------------------------------------- |
| `/`                    | The top of the mount.                                                   |
| `root`                 | My Drive. `root` is Google's own name for that folder.                  |
| `/shared-drives`       | The `Shared drives` folder.                                             |
| `{file id}`            | An item in My Drive.                                                    |
| `{drive id}/{file id}` | An item in a shared drive. The drive itself is `{drive id}/{drive id}`. |

A Drive ID never holds `/`, so the two made-up IDs cannot be real ones.

**Why a shared drive's ID is in the item's ID.** Google lists a folder of a shared drive in full only when the request names that drive (`corpora=drive` and `driveId`). Without them, Google promises only the files the user has opened, and a listing can come back short with no error. The provider keeps nothing between calls, so the ID has to carry the drive.

What follows from that:

- A file keeps its ID when Google renames or moves it, so the core keeps its node and its open handles.
- A listing gives each child the drive of the folder listed. A folder's children are always in the folder's own drive.
- A file moved between My Drive and a shared drive gets a new ID. The core sees one item removed and one added.

## The five calls

| Call    | Requests                                                                                                            |
| ------- | ------------------------------------------------------------------------------------------------------------------- |
| `root`  | None.                                                                                                               |
| `item`  | One `files.get`. None for `/` and `/shared-drives`.                                                                 |
| `list`  | `files.list`, 1,000 items a page, until Google sends no next page. `drives.list` for the shared drives, 100 a page. |
| `read`  | One `files.get` with `alt=media` and a `Range` header for exactly the bytes asked for.                              |
| `space` | One `about.get`.                                                                                                    |

- **Listing the top asks Google for My Drive.** So a sign-in that is gone shows at the top of the mount, not as an empty folder.
- **An empty listing costs one more request.** Google answers a listing of a file, and of a folder that is gone, with an empty list and no error. When a listing comes back empty, the provider asks for the folder itself and returns `NotFound` or `NotAFolder` when that is the truth.
- **A read never downloads a whole file to serve a part.** If the server ignores the range and sends the whole file, the provider skips to the offset, keeps the bytes asked for, and drops the connection. A read at or past the end returns no bytes.
- **Free space with no limit.** Google gives some Workspace accounts no storage limit. The provider then reports 1 PiB free, because a mount that reports 0 free makes file managers refuse to copy.
- **A shared drive's modified time** in the `Shared drives` listing is the time the drive was made. Google's list of drives has no other time.
- Every request is a GET. The crate has no code that sends anything else.

## Errors and retries

| Google answers                                                     | The call returns                                               |
| ------------------------------------------------------------------ | -------------------------------------------------------------- |
| Nothing, or the connection drops                                   | `Offline`                                                      |
| 401                                                                | One more try with a new token. A second 401 is `SignInNeeded`. |
| 403 with the reason `rateLimitExceeded` or `userRateLimitExceeded` | Retries, then `Other("Google Drive is limiting requests")`.    |
| 429                                                                | The same.                                                      |
| 403 with any other reason                                          | `AccessDenied`                                                 |
| 404                                                                | `NotFound`                                                     |
| 500 to 599                                                         | Retries, then `Other("Google Drive answered <status>")`.       |
| Any other status                                                   | `Other("Google Drive answered <status>")`, with no retry.      |
| A body that is not the expected JSON                               | `Other("Google Drive sent an answer Marlin could not read")`   |

- **Retries.** A call is sent again at most 3 times. The waits are 0.5, 1, and 2 seconds, each with up to half as much again added at random, so calls that failed together do not come back together. A failed connection is not retried: the helper counts those toward `offline` ([Health](mount-helper.md#health)).
- **The core's time limit covers the retries.** The core cancels a call after 30 seconds by dropping it, and that stops a wait or a request in flight.
- **Every failure to reach Google is `Offline`.** The only time limit on the HTTP client is 10 seconds to connect, so a timeout there also means Google could not be reached. `TimedOut` comes from the core alone.
- **The text of an `Other` is one of the three in this table.** No file name, ID, address, token, or message from Google is in it, so the helper can log it (#529).
- The errors follow the rules on `VfsError` in `marlin-vfs/src/error.rs`: `Other` is a failure of the remote, never of one item.

## Not checked against real Google

Every test runs against the fake server. Until #529 runs the provider on a real account, these rest on Google's documentation:

- That `corpora=drive` with `driveId` lists every child of a folder in a shared drive.
- What `files.get` returns for the top folder of a shared drive: its name and its modified time.
- That every child of a folder is in the folder's own drive.
- That Google answers a read at the end of a file with 416, and a read of an empty file too.
- Whether the request limits and the retry waits fit a real account's quota.

## Testing

`cd src-tauri && cargo test -p marlin-gdrive` needs no network and no account. A plain `cargo test` does not include it.

- Each test starts a `wiremock` server on a free local port and points `Config::api` at it.
- The test token source hands out `token-1`, then `token-2` after `token-1` is rejected. One test holds eight calls until all have `token-1`, and checks that they cause one new token, not eight.
- Two tests put the provider under a real `Vfs`: one for duplicate names and a `/` in a name, one for a rename seen on the remote.
- One test checks that every request the server got was a GET.
