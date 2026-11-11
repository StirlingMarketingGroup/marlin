# Thumbnails

The backend makes every thumbnail. The frontend asks for one by path and size, and gets back a data URL. Results are cached in memory and on disk, keyed by the file's identity, so a changed file gets a new thumbnail.

**Covers:** `src-tauri/src/thumbnails/`, the thumbnail commands in `src-tauri/src/commands.rs`, `src/hooks/useThumbnail.ts`
**Checked against the code:** 2026-11-11

## Flow

1. `useThumbnail` calls the `request_thumbnail` command with a path, a size, a quality, and a priority (`High` for visible items, `Medium` for items near the viewport, `Low` for background work).
2. `ThumbnailService` (`thumbnails/mod.rs`) builds a cache key and checks the cache. A hit returns at once.
3. On a miss, the request goes to the worker (`thumbnails/worker.rs`).
4. The worker first runs `ThumbnailGenerator::prepare`. For a remote or archive path this downloads the whole file to a temp file. See [Location providers](location-providers.md).
5. The worker then runs `ThumbnailGenerator::generate` on a blocking thread. It picks a generator by file extension.
6. The result goes into the cache and back to the frontend, with the original image width and height when the format has them.

Leaving a folder calls `cancel_all_thumbnails`. A row that scrolls away calls `cancel_thumbnail`.

## Generators (`thumbnails/generators/`)

| File                | Formats                                                                 |
| ------------------- | ----------------------------------------------------------------------- |
| `images.rs`         | JPG, PNG, GIF, WebP, BMP, TIFF, TGA, ICO                                |
| `psd.rs`            | Photoshop files                                                         |
| `pdf.rs`            | PDF, AI, EPS                                                            |
| `svg.rs`            | SVG                                                                     |
| `stl.rs`            | STL 3D models, shaded with the accent color                             |
| `video.rs`          | Video files, one frame                                                  |
| `fonts.rs`          | TTF, OTF                                                                |
| `zpl.rs`            | ZPL label files                                                         |
| `apps.rs`           | macOS `.app`, `.dmg`, `.pkg` icons                                      |
| `smb.rs`, `sftp.rs` | Not formats. They fetch file identity and contents for those providers. |

## Limits

| Limit                 | Value                        | Where       |
| --------------------- | ---------------------------- | ----------- |
| Requests in flight    | one per CPU core             | `worker.rs` |
| Generator thread pool | one per CPU core, at least 4 | `worker.rs` |
| Memory cache          | 1,000 entries or 100 MiB     | `cache.rs`  |
| Disk cache            | 500 MiB                      | `cache.rs`  |

The memory cache drops the least recently used entry first. The disk cache is in `<cache dir>/marlin_thumbnails`.

## Cache key

`generate_cache_key` in `thumbnails/mod.rs` hashes these with SHA-256 and keeps the first 16 hex characters:

- a version string (`CACHE_KEY_VERSION`); raise it to throw away every old entry
- the path
- the thumbnail size
- the file's size, its modified time in nanoseconds, and its file ID (the inode on Unix) when there is one
- the accent color, when the generator uses it

The key does not hash file contents.

For `smb` and `sftp` paths the identity comes from the provider. If that lookup fails, the key uses the current hour instead, so a network error cannot pin a stale thumbnail forever.

## Rough spots

- `ThumbnailService::request` re-generates a cached SMB image thumbnail when the cache entry has no dimensions. This backfills entries written before dimensions were stored, and it is SMB-only.
- Remote thumbnails download the whole file. A large remote video costs a full download for one frame.
