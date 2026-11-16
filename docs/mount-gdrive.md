# Google Drive mount provider (`marlin-gdrive`)

`marlin-gdrive` shows one Google account to the [mount core](mount-core.md) as a `Provider`. It reads the Drive API and never changes anything. It also holds the code that turns a saved Google sign-in into an access token, for the mount and for the app. The app links the crate for that sign-in code. The [mount helper](mount-helper.md) links it as the provider `gdrive`: see [Providers](mount-helper.md#providers).

This is not the code behind `gdrive://` paths. That is the older provider in the app, described in [Location providers](location-providers.md), and it stays as it is until #377.

**Covers:** `src-tauri/crates/marlin-gdrive/`
**Checked against the code:** 2026-11-16

## The parts

| File               | Holds                                                                                                            |
| ------------------ | ---------------------------------------------------------------------------------------------------------------- |
| `src/provider.rs`  | `DriveProvider`, its five `Provider` calls, the item IDs, and the two things a caller supplies.                  |
| `src/api.rs`       | `get`, the one path every request takes: the token, the retries, and the error table. Google's JSON.             |
| `src/sign_in.rs`   | `OAuthClient`, `SavedSignIn`, and the keychain entry: see [The saved sign-in](#the-saved-sign-in).               |
| `build.rs`         | Passes the OAuth client ID and secret to the compiler.                                                           |
| `tests/drive.rs`   | The provider's tests. They run against a fake Drive server in the test process.                                  |
| `tests/sign_in.rs` | The sign-in's tests. They run against a fake token endpoint and an in-memory keychain.                           |
| `tests/live.rs`    | One manual test against a real account: see [Not checked against real Google](#not-checked-against-real-google). |

The provider does not sign in, read a keychain, or know about the helper. A caller gives it two things:

- **`AccessTokens`** hands out an access token for the account. When Google answers 401, the provider calls `rejected(token)`, and the token source must never hand that token out again. A token that is no longer the current one changes nothing, so every call in flight can report one stale token and cause one refresh. `SavedSignIn` implements the trait.
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
- Every Drive API request is a GET. A token refresh sends a POST to Google's token endpoint. The crate sends nothing else.

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
- **The text of an `Other` is one of the three in this table, or the one from the sign-in:** "Marlin could not use the saved Google sign-in". No file name, ID, address, token, or message from Google is in it, so the helper logs it ([Logs and secrets](mount-helper.md#logs-and-secrets)).
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
- The keychain read is a blocking call inside an async one. On macOS a permission prompt holds the calling thread until the user answers (#373 checks this).

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

Google counts Drive API use in quota units. The numbers are from [Usage limits](https://developers.google.com/workspace/drive/api/guides/limits), read on 2026-11-16.

| Provider call | Request                           | Units                                                                         |
| ------------- | --------------------------------- | ----------------------------------------------------------------------------- |
| `item`        | `files.get`                       | 5                                                                             |
| `list`        | `files.list` or `drives.list`     | 100 a page. A folder that lists no items costs one `files.get` more, 5 units. |
| `read`        | `files.get` with `alt=media`      | 200, however many bytes it asks for                                           |
| `space`       | `about.get`                       | 5                                                                             |
| A new token   | A POST to Google's token endpoint | None. It is not a Drive API request.                                          |

| Limit                         | Units       |
| ----------------------------- | ----------- |
| One user, one minute          | 325,000     |
| The whole project, one minute | 1,000,000   |
| The whole project, one day    | 400,000,000 |

- **The project is Marlin's OAuth client, so every user of one build shares the project's limits.**
- **These are the limits of a Google Cloud project made on or after 2026-05-01.** An older project keeps the limits it had. The Quotas page of the project in the Google Cloud Console shows which ones apply.
- **Past a minute's limit,** Google answers 403 or 429, and the provider waits and retries: see [Errors and retries](#errors-and-retries).
- **Past the day's limit, Google plans to charge the project.** Its page says that billing starts "later in 2026", with 90 days' notice. The same page sets 1 TB a day of downloaded bytes before charges apply.
- **Reads are what reach a limit.** One user's minute holds 1,625 reads, about 27 a second. The whole project's minute holds 5,000. Linux asks for a file about 128 KiB at a time, and each piece is one `read`. That is about 3.4 MiB a second for one user. #538 tracks reading in bigger pieces.

**What this means for the core's limits.** The [mount core](mount-core.md) runs at most 16 provider calls at once and ends a call after 30 seconds. Both defaults fit Drive, with one limit.

- **16 calls at once is not a rate.** Sixteen reads that each take half a second make 32 reads a second, which is over one user's limit. A lower number would slow every mount and still not hold a rate. The fix is fewer, bigger reads (#538).
- **30 seconds covers the retries.** The three waits add up to 5.25 seconds at most. A listing is the long call: every page of a folder, one request after another, inside one call. How long a page of 1,000 items takes is not measured.
- **A listing kept for 5 seconds** costs at most 12 listings a minute for a folder that stays open: 1,200 units a page.

## Not checked against real Google

Every automated test runs against the fake server. `tests/live.rs` is a manual test on a real account, with the five `Provider` calls only. It checks the second and fourth lines below. It lists a shared drive, but it cannot tell whether that listing is complete. It reads the real keychain, it never changes anything, and it prints counts, sizes, and times but no name. Its header has the command. Nobody has run it yet, so these still rest on Google's documentation:

- That `corpora=drive` with `driveId` lists every child of a folder in a shared drive.
- What `files.get` returns for the top folder of a shared drive: its name and its modified time.
- That every child of a folder is in the folder's own drive.
- That Google answers a read at the end of a file with 416, and a read of an empty file too.
- How long one page of a listing takes, and so how big a folder can be before its listing passes the core's 30 seconds.
- **A dead connection after sleep.** The HTTP client's only time limit is 10 seconds to connect. After a laptop sleeps or changes network, a kept connection can be dead, and each call on it may hang until the core's 30 seconds.
- **A 403 that covers the whole account.** Any 403 that is not a rate limit is `AccessDenied`, which the helper counts as an answer. If Google answers the free-space request that way, a mount of such an account fails at its first call with -1005. A reason such as `domainPolicy` that starts while a mount is up leaves it `mounted` while every call fails.
- **Two 30-second limits.** A token refresh times out after 30 seconds, and so does the core's call. A slow token endpoint may show as `TimedOut` and never as `Offline`.
- **A new token after a 401.** If Google hands back the same access token after a 401, the second 401 is `SignInNeeded`.

## Testing

`cd src-tauri && cargo test -p marlin-gdrive` needs no network and no account. A plain `cargo test` does not include it.

- Each test starts a `wiremock` server on a free local port. A provider test points `Config::api` at it, and a sign-in test points `OAuthClient::token_uri` at it.
- The sign-in tests use the in-memory keychain from the `marlin-test-keychain` crate (see [Testing](testing.md)), and one email per test. They cover a refused refresh token, a missing entry, recovery after each with the same `SavedSignIn`, a failed connection, an answer that repeats the secrets, and eight calls at once.
- The test token source hands out `token-1`, then `token-2` after `token-1` is rejected. One test holds eight calls until all have `token-1`, and checks that they cause one new token, not eight.
- Two tests put the provider under a real `Vfs`: one for duplicate names and a `/` in a name, one for a rename seen on the remote.
- One test checks that every request the server got was a GET.
- The helper's tests mount this provider through FUSE against a fake Google: see [Mount helper](mount-helper.md#testing).
- `tests/live.rs` is marked `#[ignore]`. Never run this crate's tests with `--ignored` or `--include-ignored` unless you mean to read a real account.
