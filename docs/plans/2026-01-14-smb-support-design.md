# SMB Network Share Support Design

**Date:** 2026-01-14
**Status:** Draft
**Author:** Claude + Brian

## Overview

Add full SMB (Server Message Block) support to Marlin, enabling browsing and file operations on Windows file servers, TrueNAS, Synology NAS, and other SMB-compatible shares. This follows the same `LocationProvider` pattern established by Google Drive integration.

## Use Cases

- **Primary:** Work file shares (Windows Server with Active Directory, corporate environments)
- **Secondary:** Home NAS devices (TrueNAS, Synology, QNAP)
- **Tertiary:** Simple Samba shares on Linux boxes

## Architecture

### Provider Pattern Integration

```
Provider Registry:
  file://   → FileSystemProvider (existing)
  gdrive:// → GoogleDriveProvider (existing)
  smb://    → SmbProvider (new)
```

### Module Structure

```
src-tauri/src/locations/
├── mod.rs              (add smb to registry)
├── smb/
│   ├── mod.rs          (SmbProvider implementing LocationProvider)
│   ├── auth.rs         (credential storage and management)
│   └── connection.rs   (connection pooling and session management)
```

### Library Choice: `pavao`

Using the pure Rust `pavao` crate for SMB2/3 client functionality.

**Rationale:**

- Zero system dependencies = easy cross-platform installation
- SMB2/3 covers all modern servers (Windows Server 2008+, Samba 3.6+)
- Pure Rust = consistent behavior across macOS, Windows, Linux
- No Homebrew/apt dependencies for end users

**Fallback Plan:** If `pavao` hits edge cases with specific servers, we can add `libsmbclient` bindings as an optional fallback (would require system library installation).

## URI Scheme

### Format

```
smb://server/share/path/to/file
```

Examples:

- `smb://fileserver.corp/shared/documents/report.pdf`
- `smb://nas.local/media/photos/2024`
- `smb://192.168.1.100/backups`

This matches the standard SMB URL format used by Finder's "Connect to Server" and Windows Explorer, maximizing familiarity.

### Credential Association

Credentials are stored per-server (not in the URL). When navigating to an SMB path:

1. Extract server hostname from URL
2. Look up stored credentials for that server
3. If not found, prompt user to authenticate

## Authentication

### Phase 1: Username/Password Per Server

```rust
// Stored in ~/.config/marlin/smb-credentials.json (encrypted)
{
  "servers": [
    {
      "hostname": "fileserver.corp",
      "username": "brian",
      "domain": "CORP",           // Optional, for AD environments
      "password": "<encrypted>",
      "added_at": "2026-01-14T..."
    }
  ]
}
```

**Security:** Credentials encrypted at rest using OS keychain where available, falling back to machine-derived key encryption.

### Phase 1.5: URL Credential Support

Support `smb://user:pass@server/share` format for power users:

- Parse credentials from URL
- Strip from display/history
- Store using same mechanism as manual entry
- Convenient for quick connections

### Phase 2 (Future): System Credential Integration

Separate enhancement to try macOS Keychain / Windows Credential Manager before prompting. Filed as separate issue.

## Core Operations

### LocationProvider Implementation

| Operation           | SMB Support | Notes                           |
| ------------------- | ----------- | ------------------------------- |
| `read_directory`    | ✓           | List files and folders in share |
| `get_file_metadata` | ✓           | Size, dates, attributes         |
| `create_directory`  | ✓           | Create new folders              |
| `delete`            | ✓           | Delete files/folders            |
| `rename`            | ✓           | Rename within same share        |
| `copy`              | ✓           | Copy within share or to local   |
| `move_item`         | ✓           | Move within same share          |
| `read_file`         | ✓           | Stream file contents            |
| `write_file`        | ✓           | Write/upload files              |

### Server Discovery

**Not in initial scope.** Users enter server addresses manually. Network browsing (NetBIOS/mDNS discovery) is complex and can be added later.

### Share Enumeration

When connecting to a server without a share path (`smb://server`), list available shares as the root directory contents.

## Connection Management

### Connection Pooling

```rust
// Reuse connections to avoid repeated authentication
static CONNECTION_POOL: Lazy<RwLock<HashMap<String, SmbConnection>>> = ...;

// Connection has idle timeout (e.g., 5 minutes)
// Automatic reconnection on stale connection errors
```

### Error Recovery

- Detect connection drops and auto-reconnect
- Handle credential expiration gracefully
- Retry transient network errors (with backoff)

## Caching Strategy

### Directory Listing Cache

```rust
// 30-second TTL (matches Google Drive approach)
static DIR_CACHE: Lazy<RwLock<HashMap<String, CacheEntry<Vec<FileItem>>>>> = ...;
const CACHE_TTL: Duration = Duration::from_secs(30);
```

### Metadata Cache

Cache file metadata to reduce round-trips during browsing. Invalidate on write operations.

## Frontend Integration

### FileItem Enrichment

The existing `FileItem` struct works as-is. SMB files don't need special remote fields like Google Drive (no `remote_id` or `thumbnail_url`).

### Thumbnail Handling

- Use existing local thumbnail generation
- Files are streamed on-demand (no pre-download needed)
- For large files, may want to skip thumbnail generation

### Path Detection

```typescript
// src/utils/smbPath.ts
export function isSmbPath(path: string): boolean {
  return path.startsWith('smb://');
}

export function parseSmbServer(path: string): string | null {
  const match = path.match(/^smb:\/\/([^/]+)/);
  return match ? match[1] : null;
}
```

### Sidebar Integration

Add "Network" section to sidebar showing connected SMB servers (similar to Google Drive accounts).

## Commands Exposed to Frontend

```rust
// New Tauri commands
#[tauri::command]
async fn get_smb_servers() -> Result<Vec<SmbServer>, String>

#[tauri::command]
async fn add_smb_server(hostname: String, username: String, password: String, domain: Option<String>) -> Result<SmbServer, String>

#[tauri::command]
async fn remove_smb_server(hostname: String) -> Result<(), String>

#[tauri::command]
async fn test_smb_connection(hostname: String, username: String, password: String, domain: Option<String>) -> Result<bool, String>
```

## Error Handling

### Error Types

```rust
pub enum SmbError {
    ConnectionFailed(String),
    AuthenticationFailed,
    PermissionDenied,
    ShareNotFound,
    PathNotFound,
    NetworkTimeout,
    ServerUnreachable,
}
```

### User-Facing Messages

- "Could not connect to server" (with retry option)
- "Invalid username or password" (with re-authenticate option)
- "Access denied" (permission issue)
- "Server not found" (DNS/network issue)

## Performance Considerations

### Large Directories

SMB naturally handles large directories better than Google Drive (no pagination needed for listing). However:

- Consider streaming directory results for very large folders (50k+ files)
- Virtual scrolling on frontend (existing plan)

### Network Latency

- Connection pooling reduces auth overhead
- Caching reduces repeated queries
- Consider prefetching sibling folders for faster navigation

### File Operations

- Large file copies show progress
- Resumable transfers for large files (if pavao supports)

## Testing Strategy

### Unit Tests

- URL parsing and credential extraction
- Cache behavior
- Error handling

### Integration Tests

**Challenge:** Requires actual SMB server. Options:

1. Skip E2E tests for SMB (documented limitation)
2. Docker-based Samba container for CI
3. Mock SMB responses at protocol level

**Decision:** Start without E2E tests, add Docker-based testing if stability issues arise.

## Implementation Plan

### Phase 1: Core Browsing

1. Add `pavao` dependency and basic connection
2. Implement `SmbProvider` with `read_directory`
3. Add credential storage (encrypted)
4. Basic "Add Server" UI flow
5. Navigate and browse shares

### Phase 2: File Operations

1. Implement remaining `LocationProvider` methods
2. Copy/move between SMB and local
3. Delete with confirmation
4. Create new folders

### Phase 3: Polish

1. URL credential parsing (`smb://user:pass@server`)
2. Connection error recovery
3. Share enumeration at server root
4. Performance optimization

### Future Enhancements (Separate Issues)

- System credential integration (Phase 2 auth)
- Network discovery (NetBIOS/mDNS)
- Kerberos authentication for enterprise

## Open Questions

1. **Credential encryption:** Use OS keychain directly, or implement our own encryption with machine-derived key?
2. **Connection timeout:** What's a reasonable timeout for initial connection? (Suggest 10 seconds)
3. **Offline behavior:** How to handle browsing when server becomes unreachable mid-session?

## References

- [pavao crate](https://crates.io/crates/pavao) - Pure Rust SMB2/3 client
- [SMB URL scheme](<https://docs.microsoft.com/en-us/previous-versions/windows/internet-explorer/ie-developer/platform-apis/jj710207(v=vs.85)>) - Microsoft documentation
- [Google Drive integration PR](./2025-12-09-google-drive-integration-design.md) - Similar pattern reference
