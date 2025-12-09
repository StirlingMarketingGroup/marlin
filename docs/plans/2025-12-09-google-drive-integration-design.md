# Google Drive Integration Design

## Overview

Full Google Drive integration for Marlin, allowing users to browse their Google Drive as seamlessly as local folders. Users can paste Google Drive URLs directly in the path bar and navigate to them automatically.

## URI Format

```
gdrive://<account-email>/<virtual-path>
```

Examples:
- `gdrive://brian@gmail.com/My Drive`
- `gdrive://brian@gmail.com/My Drive/Projects/design.fig`
- `gdrive://brian@work.com/Shared with me/Team Docs`

## Core Architecture

### Google Drive Provider

New `GoogleDriveProvider` in `src-tauri/src/locations/gdrive/` implementing the `LocationProvider` trait. Registers under the `gdrive` scheme.

**Virtual root folders** (shown at `gdrive://account@email.com/`):
- My Drive
- Shared with me
- Starred
- Recent

The provider translates virtual paths to Google Drive API calls using file IDs internally, but users always see friendly paths.

### Rust Dependencies

```toml
# src-tauri/Cargo.toml
google-drive3 = "5"      # Google Drive API client
yup-oauth2 = "8"         # OAuth 2.0 for Google APIs
hyper = "0.14"           # HTTP server for OAuth callback
```

## Authentication

### OAuth 2.0 Flow

1. App registers with Google Cloud Console (Drive API enabled)
2. When auth needed, open system browser to Google consent screen
3. Local HTTP server on random port listens for OAuth callback
4. Exchange code for access + refresh tokens
5. Store tokens locally

### Account Storage

Location: `~/.config/marlin/gdrive-accounts.json` (or platform equivalent)

```json
{
  "accounts": [
    {
      "email": "brian@gmail.com",
      "accessToken": "...",
      "refreshToken": "...",
      "expiresAt": 1234567890
    }
  ]
}
```

### Multiple Accounts

Full support for multiple Google accounts:
- Each appears as separate sidebar entry
- URIs namespaced by email
- Independent auth state per account

### Token Management

- Automatic refresh when access token expires
- Refresh tokens are long-lived
- Re-auth prompt if refresh fails (token revoked, etc.)

## URL Pasting & Resolution

### Supported URL Formats

```
https://drive.google.com/open?id=XXXXX
https://drive.google.com/file/d/XXXXX/view
https://drive.google.com/drive/folders/XXXXX
https://drive.google.com/drive/u/0/folders/XXXXX
```

### Resolution Flow

1. Detect Google Drive URL pattern in path bar input
2. Extract file/folder ID from URL
3. Try each connected account in sequence:
   - Call `files.get` API with the ID
   - If 200: Navigate to `gdrive://account@email.com/<resolved-path>`
   - If 403/404: Try next account
4. If no account works:
   - Show error: "Can't access this file"
   - Offer "Add Google Account" button
5. If no accounts connected:
   - Auto-launch OAuth flow
   - Retry resolution after auth completes

### Path Resolution

When file found by ID, walk parent chain to build human-readable path for display in path bar.

## Sidebar Integration

### Cloud Storage Section

New section in sidebar (below Favorites, above Locations):

```
▾ Cloud Storage
    Google Drive (brian@gmail.com)
    Google Drive (brian@work.com)
    Add Google Account...
```

### Account Entry Behavior

- Click: Navigate to `gdrive://email/` showing virtual root
- Right-click: Context menu with "Disconnect account"
- Visual: Cloud icon, loading spinner during operations, error badge on auth failure

### Virtual Root View

When navigating to an account root:
- My Drive
- Shared with me
- Starred
- Recent

## File Operations

### Capabilities

```rust
LocationCapabilities {
    can_read: true,
    can_write: true,
    can_create_directories: true,
    can_delete: true,
    can_rename: true,
    can_copy: true,
    can_move: true,
    supports_watching: false,
    requires_explicit_refresh: true,
}
```

### API Mappings

| Operation | Google Drive API |
|-----------|------------------|
| List directory | `files.list` with parent query |
| Get metadata | `files.get` |
| Create folder | `files.create` (folder mimeType) |
| Delete | `files.update` (trash) |
| Rename | `files.update` (name field) |
| Copy | `files.copy` |
| Move | `files.update` (parent field) |
| Download | `files.get` with `alt=media` |
| Upload | `files.create` with content |

### Drag Operations

**Drag OUT:**
1. Download file to temp directory
2. Initiate native drag with temp file
3. Clean up temp files periodically

**Drag IN:**
1. Upload via `files.create`
2. Show progress for large files

## Caching & Performance

### Directory Listing Cache

- In-memory cache with 30-second TTL
- Invalidate after write operations
- Manual refresh (Cmd+R) bypasses cache

### File ID to Path Cache

- Cache ID-to-path mappings
- Rebuild on cache miss by walking parent chain

### Thumbnail Cache

- Reuse existing thumbnail infrastructure
- Cache key: `gdrive-{fileId}-{modifiedTime}`
- Fetch from Google-provided thumbnail URLs

### Rate Limiting

- Google quota: ~1000 requests/100 seconds
- Queue and throttle requests
- Batch metadata requests where possible

### Offline Behavior

- Show cached listings with "offline" indicator
- Write operations fail with clear error

## Implementation Files

### Backend (Rust)

| File | Purpose |
|------|---------|
| `src-tauri/src/locations/gdrive/mod.rs` | Provider implementation |
| `src-tauri/src/locations/gdrive/auth.rs` | OAuth flow, token storage |
| `src-tauri/src/locations/gdrive/api.rs` | Drive API wrapper |
| `src-tauri/src/locations/gdrive/cache.rs` | Listing and path caches |
| `src-tauri/src/locations/gdrive/url.rs` | Google Drive URL parsing |
| `src-tauri/src/locations/mod.rs` | Register gdrive provider |
| `src-tauri/src/commands.rs` | Account management commands |

### Frontend (TypeScript)

| File | Purpose |
|------|---------|
| `src/components/Sidebar.tsx` | Cloud Storage section |
| `src/components/PathBar.tsx` | Google Drive URL detection |
| `src/store/useAppStore.ts` | Connected accounts state |
| `src/components/GoogleAuthPrompt.tsx` | Auth error/prompt UI |

## Google Cloud Setup

Required for OAuth to work:

1. Create project in Google Cloud Console
2. Enable Google Drive API
3. Configure OAuth consent screen
4. Create OAuth 2.0 credentials (Desktop app type)
5. Add credentials to app configuration

The client ID/secret will need to be bundled with the app or configured at build time.

## Future Enhancements

- Shared drive support (Google Workspace)
- Search within Google Drive
- Offline file sync
- Conflict resolution for simultaneous edits
- Google Docs/Sheets preview (export as PDF)
