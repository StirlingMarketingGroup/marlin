# Thumbnails

The backend makes every thumbnail. The frontend asks for one by path and size, and gets back a data URL. Results are cached in memory and on disk, keyed by the file's identity, so a changed file gets a new thumbnail.

**Covers:** `src-tauri/src/thumbnails/`, the thumbnail commands in `src-tauri/src/commands.rs`, `remote_thumbnail` in `src-tauri/src/mount_helper.rs`, `src/hooks/useThumbnail.ts`
**Checked against the code:** 2026-11-25

## Flow

1. `useThumbnail` calls the `request_thumbnail` command with a path, a size, a quality, and a priority (`High` for visible items, `Medium` for items near the viewport, `Low` for background work).
2. `ThumbnailService` (`thumbnails/mod.rs`) builds a cache key and checks the cache. A hit returns at once.
3. On a miss, the request goes to the worker (`thumbnails/worker.rs`). The worker builds the cache key again before it reads anything: see [Cache key](#cache-key).
4. The worker then runs `ThumbnailGenerator::prepare`. For a file in a Google Drive mount it asks the mount helper for Drive's own picture: see [Files in a Google Drive mount](#files-in-a-google-drive-mount). For an `archive://`, `gdrive://`, or `sftp://` path it extracts or downloads the whole file to a temp file, and the worker deletes that file once the thumbnail is made. Some of those files are caches that other actions reuse, and a cancelled thumbnail leaves its file behind (#761). An `smb://` path is read by the SMB generator in the next step. See [Location providers](location-providers.md).
5. The worker then runs `ThumbnailGenerator::generate` on a blocking thread. It picks a generator by file extension, or resizes Drive's picture.
6. The result goes into the cache and back to the frontend, with the original image width and height when the format has them.

Leaving a folder calls `cancel_all_thumbnails`. A row that scrolls away ignores its pending result. `useThumbnail` learns a request's ID only from its answer, so it cannot send `cancel_thumbnail` for a request that is still pending.

## Generators (`thumbnails/generators/`)

| File                | Formats                                                                 |
| ------------------- | ----------------------------------------------------------------------- |
| `images.rs`         | JPG, PNG, GIF, WebP, BMP, TIFF, TGA, ICO                                |
| `psd.rs`            | Photoshop files                                                         |
| `pdf.rs`            | PDF, AI                                                                 |
| `svg.rs`            | SVG                                                                     |
| `stl.rs`            | STL 3D models, shaded with the accent color                             |
| `video.rs`          | Video files, one frame                                                  |
| `fonts.rs`          | TTF, OTF                                                                |
| `zpl.rs`            | ZPL label files                                                         |
| `apps.rs`           | macOS `.app`, `.dmg`, `.pkg` icons                                      |
| `smb.rs`, `sftp.rs` | Not formats. They fetch file identity and contents for those providers. |

## Limits

| Limit                 | Value                        | Where               |
| --------------------- | ---------------------------- | ------------------- |
| Requests in flight    | one per CPU core             | `worker.rs`         |
| Generator thread pool | one per CPU core, at least 4 | `worker.rs`         |
| ZPL renders in flight | one                          | `generators/zpl.rs` |
| Memory cache          | 1,000 entries or 100 MiB     | `cache.rs`          |
| Disk cache            | 500 MiB                      | `cache.rs`          |

ZPL labels render one at a time because the Go library under `zpl-rs` is not safe to call from two threads. Two renders at once abort the app ([go-zpl issue 33](https://github.com/StirlingMarketingGroup/go-zpl/issues/33)).

The memory cache drops the least recently used entry first. The disk cache is in `<cache dir>/marlin_thumbnails`.

## Cache key

`generate_cache_key` in `thumbnails/mod.rs` hashes these with SHA-256 and keeps the first 16 hex characters:

- a version string (`CACHE_KEY_VERSION`); raise it to throw away every old entry
- the path
- the thumbnail size
- the file's size, its modified time in nanoseconds, and its file ID (the inode on Unix) when there is one
- the accent color, when the generator uses it

The key does not hash file contents.

The worker builds the key before `prepare`, and stores the thumbnail under that key. So a file that changes while its thumbnail is made gets that thumbnail under its old size and time, which no later request asks for. The next request builds a new key and makes a new thumbnail.

For `smb` and `sftp` paths the identity comes from the provider. If that lookup fails, the key uses the current hour instead, so a network error cannot pin a stale thumbnail forever.

## Files in a Google Drive mount

A file in a Google Drive mount has a local path, so its cache key is the one any local file gets: the path, and the size, modified time, and inode the mount shows. An edit through the mount changes the size or the time, so it makes a new entry.

The generators would read such a file whole through the mount, and a folder of photos would download every photo. So `prepare` first asks the mount helper for Drive's own picture of the file (`remote_thumbnail` in `mount_helper.rs`, which sends the helper's [`thumbnail` request](mount-helper.md#thumbnails)).

- **Drive has a picture.** The helper writes it into a private temp folder that the app makes for the request. `generate` resizes it with the image generator. That keeps the request's output format, and for a WebP request picks one by the file's own extension, as for the file itself. The folder goes when the thumbnail is made. A request cancelled while it waits for the helper drops the folder at once, so a helper that answers late finds no folder.
- **The size the UI shows is the file's own**, from Drive: an image's or a video's width and height. Drive gives no size for other files, so a PDF from Drive shows none, where a PDF the generators read shows its page size.
- **The generators read the file** when Drive has no picture (`no-picture`), when the file has bytes that are not on Drive yet (`not-eligible`; the mount serves spooled bytes from the spool), when Drive's picture does not decode, and when the helper answers an error.
- **Only a Google Drive mount is asked.** SFTP and SMB providers make no pictures, and their files are read through the mount.
- **The mount table must know the mount.** It is empty until a command connects to the helper, so before that the generators read the file: see [The app side](mount-helper.md#the-app-side). A thumbnail never starts a helper. While a command starts one, the table is empty, so the generators read the file. At the end of a connect, a thumbnail can wait a moment while the command lists the mounts. No part of the UI sends a mount command yet (#377), so today the generators read every file in a mount.

## Rough spots

- `ThumbnailService::request` re-generates a cached SMB image thumbnail when its cache entry has no dimensions. It does this for SMB only.
- A remote file that reaches a generator is downloaded whole. A `gdrive://` item with a thumbnail URL skips this: `useThumbnail` fetches Drive's picture through `fetch_gdrive_url`. A large remote video costs a full download for one frame. A file in a Google Drive mount avoids it when Drive has a picture: see [Files in a Google Drive mount](#files-in-a-google-drive-mount).
- PDF and AI thumbnails are drawn by `hayro`, a pure Rust renderer. It reads the whole file into memory. A PDF that needs a password to open gets no thumbnail: Marlin gives `hayro` no password. Its README says it does not yet draw blend modes, isolated or knockout transparency groups, or color key masks. In a debug build (`npm run tauri dev`) it is 20 to 30 times slower than in a release build: a page takes 0.1 to 6 seconds.
- EPS files and Illustrator files older than version 9 are PostScript, not PDF. They get no thumbnail. The frontend asks for one for `.eps` and gets an error.
