# Location Provider Architecture

Marlin's filesystem commands can now operate on pluggable "location providers". The
initial implementation continues to use the local filesystem provider (`file://`),
but the abstraction allows future backends like `s3://`, `smb://`, or `ftp://` to
plug in without touching command handlers.

## Provider registry

`src-tauri/src/locations/mod.rs` defines the core pieces:

- `LocationInput` parses the path payload that comes from Tauri commands. It accepts
  either a raw string (e.g. `/Users/me` or `s3://my-bucket/path`) or a structured
  object with `scheme`, `authority`, and `path` fields.
- `Location` represents the parsed result and provides helpers for building native
  paths when the provider expects them.
- `LocationProvider` is the trait each backend implements. It covers directory
  listing, metadata, and basic file operations (create, delete, rename, copy, move).
- `LocationCapabilities` communicates which operations are allowed so the frontend
  can disable unsupported actions.
- `ProviderDirectoryEntries` wraps the normalized `LocationSummary` plus the
  `FileItem` list after a directory read.

The static registry registers the local filesystem provider by default:

```rust
static REGISTRY: Lazy<RwLock<ProviderMap>> = Lazy::new(|| {
    let mut map = HashMap::new();
    let file_provider: ProviderRef = Arc::new(FileSystemProvider::default());
    map.insert(file_provider.scheme().to_string(), file_provider);
    RwLock::new(map)
});
```

`resolve_location` is used by Tauri commands to convert the inbound payload into a
`Location` and look up the matching provider.

## FileSystemProvider

`src-tauri/src/locations/file.rs` contains the current `file://` provider. It
wraps the existing helpers in `fs_utils.rs` to:

- Expand the incoming path (including `~` on macOS/Linux).
- Validate directory / file existence before acting.
- Preserve the previous case-insensitive rename behaviour by using a two-stage
  rename when only letter casing changes.
- Report full capabilities (`canCopy`, `canMove`, `supportsWatching`, etc.).

Future providers should follow the same pattern: resolve the raw location into a
provider-specific native representation, implement the trait functions, and return
a meaningful `LocationSummary` so the frontend can store a canonical URI.

## Command changes

`src-tauri/src/commands.rs` now accepts `LocationInput` for the core file
operations (`read_directory`, `get_file_metadata`, `create_directory_command`,
`delete_file`, `rename_file`, `copy_file`, `move_file`). `read_directory` returns a
`DirectoryListingResponse` that includes both the file entries and the provider
capabilities.

Commands that still rely on direct filesystem access (e.g. disk usage, Git status,
watchers) continue to operate on local paths and now use the normalized path from
the provider response.

## Frontend adjustments

The frontend store (`src/store/useAppStore.ts`) continues to track `currentPath`
as before but now consumes the richer `DirectoryListingResponse`. It stores the
provider capabilities so UI surfaces can disable actions when the backend reports
they are unavailable.

## Adding a new provider

1. Implement a new module in `src-tauri/src/locations/` that satisfies the
   `LocationProvider` trait.
2. Register it in the registry (e.g. insert under `s3` or `smb` scheme).
3. Update the frontend to send the appropriate scheme in command invocations and
   to respect the reported capabilities.
4. Extend the UI as needed (auth prompts, status indicators, etc.).

With these pieces in place the backend no longer assumes every path is local, and
new protocols can be introduced incrementally without rewriting the command layer.
