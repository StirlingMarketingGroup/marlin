# SMB Support Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add SMB network share support to Marlin, enabling browsing and file operations on Windows file servers and NAS devices.

**Architecture:** Implement `SmbProvider` using the `LocationProvider` trait pattern. Use `pavao` crate (libsmbclient wrapper) for macOS/Linux. On Windows, SMB shares are native UNC paths handled by the existing `FileSystemProvider`.

**Tech Stack:** Rust (pavao crate), Tauri commands, React/Zustand frontend

---

## Task 1: Add pavao dependency

**Files:**
- Modify: `src-tauri/Cargo.toml`

**Step 1: Add pavao to dependencies**

Add to `Cargo.toml` under `[dependencies]`:

```toml
# SMB support (macOS/Linux only - Windows uses native UNC paths)
[target.'cfg(not(target_os = "windows"))'.dependencies]
pavao = "0.2"
```

**Step 2: Verify cargo check passes**

Run: `cd src-tauri && cargo check`
Expected: Compilation succeeds (may need `brew install samba` on macOS)

**Step 3: Commit**

```bash
git add src-tauri/Cargo.toml
git commit -m "feat(smb): add pavao dependency for SMB support"
```

---

## Task 2: Create SMB module structure

**Files:**
- Create: `src-tauri/src/locations/smb/mod.rs`
- Create: `src-tauri/src/locations/smb/auth.rs`
- Modify: `src-tauri/src/locations/mod.rs`

**Step 1: Create mod.rs with provider stub**

Create `src-tauri/src/locations/smb/mod.rs`:

```rust
mod auth;

use async_trait::async_trait;
use crate::fs_utils::FileItem;
use crate::locations::{
    Location, LocationCapabilities, LocationProvider, LocationSummary, ProviderDirectoryEntries,
};

pub use auth::{SmbServer, SmbServerInfo, get_smb_servers, add_smb_server, remove_smb_server, test_smb_connection};

#[derive(Default)]
pub struct SmbProvider;

#[async_trait]
impl LocationProvider for SmbProvider {
    fn scheme(&self) -> &'static str {
        "smb"
    }

    fn capabilities(&self, _location: &Location) -> LocationCapabilities {
        LocationCapabilities::new("smb", "SMB Share", true, true)
            .with_supports_watching(false)
    }

    async fn read_directory(&self, location: &Location) -> Result<ProviderDirectoryEntries, String> {
        let authority = location.authority()
            .ok_or_else(|| "SMB path requires server: smb://server/share/path".to_string())?;

        // TODO: Implement directory listing
        Err(format!("SMB read_directory not yet implemented for {}", authority))
    }

    async fn get_file_metadata(&self, _location: &Location) -> Result<FileItem, String> {
        Err("SMB get_file_metadata not yet implemented".to_string())
    }

    async fn create_directory(&self, _location: &Location) -> Result<(), String> {
        Err("SMB create_directory not yet implemented".to_string())
    }

    async fn delete(&self, _location: &Location) -> Result<(), String> {
        Err("SMB delete not yet implemented".to_string())
    }

    async fn rename(&self, _from: &Location, _to: &Location) -> Result<(), String> {
        Err("SMB rename not yet implemented".to_string())
    }

    async fn copy(&self, _from: &Location, _to: &Location) -> Result<(), String> {
        Err("SMB copy not yet implemented".to_string())
    }
}
```

**Step 2: Create auth.rs with credential storage**

Create `src-tauri/src/locations/smb/auth.rs`:

```rust
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::RwLock;

/// Information about a connected SMB server (safe to expose to frontend)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmbServerInfo {
    pub hostname: String,
    pub username: String,
    pub domain: Option<String>,
}

/// Stored server data with credentials
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmbServer {
    pub hostname: String,
    pub username: String,
    pub password: String,
    pub domain: Option<String>,
}

/// Storage structure for servers file
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ServerStorage {
    servers: Vec<SmbServer>,
}

/// In-memory cache of servers
static SERVERS_CACHE: Lazy<RwLock<Option<Vec<SmbServer>>>> = Lazy::new(|| RwLock::new(None));

/// Get the path to the servers storage file
fn get_servers_path() -> Result<PathBuf, String> {
    let config_dir = dirs::config_dir()
        .ok_or_else(|| "Could not determine config directory".to_string())?;
    let marlin_dir = config_dir.join("marlin");

    if !marlin_dir.exists() {
        fs::create_dir_all(&marlin_dir)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }

    Ok(marlin_dir.join("smb-servers.json"))
}

/// Load servers from disk
fn load_servers_from_disk() -> Result<Vec<SmbServer>, String> {
    let path = get_servers_path()?;

    if !path.exists() {
        return Ok(Vec::new());
    }

    let contents = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read servers file: {}", e))?;

    let storage: ServerStorage = serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse servers file: {}", e))?;

    Ok(storage.servers)
}

/// Save servers to disk
fn save_servers_to_disk(servers: &[SmbServer]) -> Result<(), String> {
    let path = get_servers_path()?;

    let storage = ServerStorage {
        servers: servers.to_vec(),
    };

    let contents = serde_json::to_string_pretty(&storage)
        .map_err(|e| format!("Failed to serialize servers: {}", e))?;

    fs::write(&path, contents)
        .map_err(|e| format!("Failed to write servers file: {}", e))?;

    Ok(())
}

/// Get all connected SMB servers (safe info only)
pub fn get_smb_servers() -> Result<Vec<SmbServerInfo>, String> {
    // Check cache first
    {
        let cache = SERVERS_CACHE.read().map_err(|e| e.to_string())?;
        if let Some(servers) = &*cache {
            return Ok(servers.iter().map(|s| SmbServerInfo {
                hostname: s.hostname.clone(),
                username: s.username.clone(),
                domain: s.domain.clone(),
            }).collect());
        }
    }

    // Load from disk
    let servers = load_servers_from_disk()?;

    // Update cache
    {
        let mut cache = SERVERS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(servers.clone());
    }

    Ok(servers.iter().map(|s| SmbServerInfo {
        hostname: s.hostname.clone(),
        username: s.username.clone(),
        domain: s.domain.clone(),
    }).collect())
}

/// Get credentials for a specific server (internal use)
pub fn get_server_credentials(hostname: &str) -> Result<SmbServer, String> {
    // Check cache first
    {
        let cache = SERVERS_CACHE.read().map_err(|e| e.to_string())?;
        if let Some(servers) = &*cache {
            if let Some(server) = servers.iter().find(|s| s.hostname.eq_ignore_ascii_case(hostname)) {
                return Ok(server.clone());
            }
        }
    }

    // Load from disk
    let servers = load_servers_from_disk()?;

    // Update cache
    {
        let mut cache = SERVERS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(servers.clone());
    }

    servers.iter()
        .find(|s| s.hostname.eq_ignore_ascii_case(hostname))
        .cloned()
        .ok_or_else(|| format!("No credentials stored for server: {}", hostname))
}

/// Add a new SMB server
pub fn add_smb_server(hostname: String, username: String, password: String, domain: Option<String>) -> Result<SmbServerInfo, String> {
    let mut servers = load_servers_from_disk()?;

    // Check if server already exists
    if let Some(existing) = servers.iter_mut().find(|s| s.hostname.eq_ignore_ascii_case(&hostname)) {
        // Update existing
        existing.username = username.clone();
        existing.password = password;
        existing.domain = domain.clone();
    } else {
        // Add new
        servers.push(SmbServer {
            hostname: hostname.clone(),
            username: username.clone(),
            password,
            domain: domain.clone(),
        });
    }

    save_servers_to_disk(&servers)?;

    // Update cache
    {
        let mut cache = SERVERS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(servers);
    }

    Ok(SmbServerInfo {
        hostname,
        username,
        domain,
    })
}

/// Remove an SMB server
pub fn remove_smb_server(hostname: &str) -> Result<(), String> {
    let mut servers = load_servers_from_disk()?;

    let original_len = servers.len();
    servers.retain(|s| !s.hostname.eq_ignore_ascii_case(hostname));

    if servers.len() == original_len {
        return Err(format!("Server not found: {}", hostname));
    }

    save_servers_to_disk(&servers)?;

    // Update cache
    {
        let mut cache = SERVERS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(servers);
    }

    Ok(())
}

/// Test connection to an SMB server (without saving credentials)
pub fn test_smb_connection(_hostname: &str, _username: &str, _password: &str, _domain: Option<&str>) -> Result<bool, String> {
    // TODO: Implement actual connection test using pavao
    Ok(true)
}
```

**Step 3: Register SMB provider in locations/mod.rs**

Modify `src-tauri/src/locations/mod.rs`. Add after `pub mod gdrive;`:

```rust
#[cfg(not(target_os = "windows"))]
pub mod smb;

#[cfg(not(target_os = "windows"))]
pub use smb::SmbProvider;
```

And in the `REGISTRY` static initialization, add after the gdrive_provider registration:

```rust
#[cfg(not(target_os = "windows"))]
{
    let smb_provider: ProviderRef = Arc::new(SmbProvider::default());
    map.insert(smb_provider.scheme().to_string(), smb_provider);
}
```

**Step 4: Verify cargo check passes**

Run: `cd src-tauri && cargo check`
Expected: Compilation succeeds

**Step 5: Commit**

```bash
git add src-tauri/src/locations/smb/
git add src-tauri/src/locations/mod.rs
git commit -m "feat(smb): add SMB provider module structure"
```

---

## Task 3: Add Tauri commands for SMB server management

**Files:**
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs`

**Step 1: Add SMB commands to commands.rs**

Add to `src-tauri/src/commands.rs`:

```rust
// SMB server management commands
#[cfg(not(target_os = "windows"))]
#[tauri::command]
pub fn get_smb_servers() -> Result<Vec<crate::locations::smb::SmbServerInfo>, String> {
    crate::locations::smb::get_smb_servers()
}

#[cfg(not(target_os = "windows"))]
#[tauri::command]
pub fn add_smb_server(
    hostname: String,
    username: String,
    password: String,
    domain: Option<String>,
) -> Result<crate::locations::smb::SmbServerInfo, String> {
    crate::locations::smb::add_smb_server(hostname, username, password, domain)
}

#[cfg(not(target_os = "windows"))]
#[tauri::command]
pub fn remove_smb_server(hostname: String) -> Result<(), String> {
    crate::locations::smb::remove_smb_server(&hostname)
}

#[cfg(not(target_os = "windows"))]
#[tauri::command]
pub fn test_smb_connection(
    hostname: String,
    username: String,
    password: String,
    domain: Option<String>,
) -> Result<bool, String> {
    crate::locations::smb::test_smb_connection(&hostname, &username, &password, domain.as_deref())
}

// Windows stubs - SMB is native via file:// paths
#[cfg(target_os = "windows")]
#[tauri::command]
pub fn get_smb_servers() -> Result<Vec<()>, String> {
    Ok(vec![])
}

#[cfg(target_os = "windows")]
#[tauri::command]
pub fn add_smb_server(
    _hostname: String,
    _username: String,
    _password: String,
    _domain: Option<String>,
) -> Result<(), String> {
    Err("SMB on Windows uses native UNC paths. Navigate to \\\\server\\share directly.".to_string())
}

#[cfg(target_os = "windows")]
#[tauri::command]
pub fn remove_smb_server(_hostname: String) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "windows")]
#[tauri::command]
pub fn test_smb_connection(
    _hostname: String,
    _username: String,
    _password: String,
    _domain: Option<String>,
) -> Result<bool, String> {
    Ok(true)
}
```

**Step 2: Register commands in lib.rs**

Find the `tauri::Builder` invocation in `lib.rs` and add the SMB commands to the `.invoke_handler()` macro:

```rust
get_smb_servers,
add_smb_server,
remove_smb_server,
test_smb_connection,
```

**Step 3: Verify cargo check passes**

Run: `cd src-tauri && cargo check`
Expected: Compilation succeeds

**Step 4: Commit**

```bash
git add src-tauri/src/commands.rs src-tauri/src/lib.rs
git commit -m "feat(smb): add Tauri commands for SMB server management"
```

---

## Task 4: Implement SMB connection and directory listing with pavao

**Files:**
- Modify: `src-tauri/src/locations/smb/mod.rs`
- Create: `src-tauri/src/locations/smb/connection.rs`

**Step 1: Create connection.rs for connection management**

Create `src-tauri/src/locations/smb/connection.rs`:

```rust
use once_cell::sync::Lazy;
use pavao::{SmbClient, SmbCredentials, SmbOptions};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use super::auth::get_server_credentials;

/// Connection with last-used timestamp
struct ConnectionEntry {
    client: SmbClient,
    last_used: Instant,
}

/// Connection pool with idle timeout
static CONNECTION_POOL: Lazy<RwLock<HashMap<String, ConnectionEntry>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// Connection idle timeout (5 minutes)
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(300);

/// Get or create a connection to an SMB server
pub fn get_connection(hostname: &str, share: &str) -> Result<Arc<SmbClient>, String> {
    let key = format!("{}:{}", hostname.to_lowercase(), share.to_lowercase());

    // Check for existing connection
    {
        let mut pool = CONNECTION_POOL.write().map_err(|e| e.to_string())?;

        // Clean up stale connections
        pool.retain(|_, entry| entry.last_used.elapsed() < CONNECTION_TIMEOUT);

        if let Some(entry) = pool.get_mut(&key) {
            entry.last_used = Instant::now();
            // Note: SmbClient doesn't implement Clone, so we need to recreate
            // For now, just create a new connection each time
        }
    }

    // Create new connection
    let creds = get_server_credentials(hostname)?;

    let smb_url = format!("smb://{}", hostname);
    let share_path = if share.starts_with('/') {
        share.to_string()
    } else {
        format!("/{}", share)
    };

    let mut credentials = SmbCredentials::default()
        .server(&smb_url)
        .share(&share_path)
        .username(&creds.username)
        .password(&creds.password);

    if let Some(domain) = &creds.domain {
        credentials = credentials.workgroup(domain);
    }

    let client = SmbClient::new(credentials, SmbOptions::default())
        .map_err(|e| format!("Failed to connect to SMB server: {}", e))?;

    Ok(Arc::new(client))
}

/// Parse an SMB path into (hostname, share, path)
pub fn parse_smb_path(authority: &str, path: &str) -> Result<(String, String, String), String> {
    let hostname = authority.to_string();

    // Path format: /share/rest/of/path
    let path = if path.starts_with('/') { &path[1..] } else { path };

    let mut parts = path.splitn(2, '/');
    let share = parts.next().unwrap_or("").to_string();
    let file_path = parts.next().map(|p| format!("/{}", p)).unwrap_or_else(|| "/".to_string());

    if share.is_empty() {
        return Err("SMB path must include share name: smb://server/share/path".to_string());
    }

    Ok((hostname, share, file_path))
}
```

**Step 2: Update mod.rs to implement directory listing**

Replace the `read_directory` implementation in `src-tauri/src/locations/smb/mod.rs`:

```rust
mod auth;
mod connection;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use crate::fs_utils::FileItem;
use crate::locations::{
    Location, LocationCapabilities, LocationProvider, LocationSummary, ProviderDirectoryEntries,
};

pub use auth::{SmbServer, SmbServerInfo, get_smb_servers, add_smb_server, remove_smb_server, test_smb_connection};
use connection::{get_connection, parse_smb_path};

#[derive(Default)]
pub struct SmbProvider;

#[async_trait]
impl LocationProvider for SmbProvider {
    fn scheme(&self) -> &'static str {
        "smb"
    }

    fn capabilities(&self, _location: &Location) -> LocationCapabilities {
        LocationCapabilities::new("smb", "SMB Share", true, true)
            .with_supports_watching(false)
    }

    async fn read_directory(&self, location: &Location) -> Result<ProviderDirectoryEntries, String> {
        let authority = location.authority()
            .ok_or_else(|| "SMB path requires server: smb://server/share/path".to_string())?;

        let (hostname, share, path) = parse_smb_path(authority, location.path())?;

        // Get SMB connection
        let client = get_connection(&hostname, &share)?;

        // List directory
        let entries = client.list_dir(&path)
            .map_err(|e| format!("Failed to list directory: {}", e))?;

        let mut items: Vec<FileItem> = Vec::new();

        for entry in entries {
            let name = entry.name();

            // Skip . and ..
            if name == "." || name == ".." {
                continue;
            }

            let full_path = if path == "/" {
                format!("smb://{}/{}/{}", hostname, share, name)
            } else {
                format!("smb://{}/{}{}/{}", hostname, share, path, name)
            };

            // Get file stats if available
            let entry_path = if path == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", path, name)
            };

            let (is_dir, size, modified) = match client.stat(&entry_path) {
                Ok(stat) => {
                    let is_directory = stat.is_dir();
                    let file_size = stat.size();
                    let mtime = DateTime::<Utc>::from_timestamp(stat.mtime() as i64, 0)
                        .unwrap_or_else(Utc::now);
                    (is_directory, file_size as u64, mtime)
                }
                Err(_) => {
                    // Fallback: try to detect directory by listing it
                    let is_directory = client.list_dir(&entry_path).is_ok();
                    (is_directory, 0, Utc::now())
                }
            };

            items.push(FileItem {
                name: name.to_string(),
                path: full_path,
                is_dir,
                is_hidden: name.starts_with('.'),
                size,
                modified: modified.to_rfc3339(),
                extension: if is_dir {
                    None
                } else {
                    std::path::Path::new(name)
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.to_lowercase())
                },
                is_symlink: false,
                symlink_target: None,
                remote_id: None,
                thumbnail_url: None,
                download_url: None,
            });
        }

        // Sort: directories first, then by name
        items.sort_by(|a, b| {
            match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });

        let display_path = format!("smb://{}/{}{}", hostname, share, path);

        Ok(ProviderDirectoryEntries {
            location: LocationSummary::new(
                "smb",
                Some(hostname.clone()),
                format!("/{}{}", share, path),
                display_path,
            ),
            entries: items,
        })
    }

    async fn get_file_metadata(&self, location: &Location) -> Result<FileItem, String> {
        let authority = location.authority()
            .ok_or_else(|| "SMB path requires server".to_string())?;

        let (hostname, share, path) = parse_smb_path(authority, location.path())?;
        let client = get_connection(&hostname, &share)?;

        let stat = client.stat(&path)
            .map_err(|e| format!("Failed to get file metadata: {}", e))?;

        let name = std::path::Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&path)
            .to_string();

        let is_dir = stat.is_dir();
        let modified = DateTime::<Utc>::from_timestamp(stat.mtime() as i64, 0)
            .unwrap_or_else(Utc::now);

        Ok(FileItem {
            name: name.clone(),
            path: location.raw().to_string(),
            is_dir,
            is_hidden: name.starts_with('.'),
            size: stat.size() as u64,
            modified: modified.to_rfc3339(),
            extension: if is_dir {
                None
            } else {
                std::path::Path::new(&name)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase())
            },
            is_symlink: false,
            symlink_target: None,
            remote_id: None,
            thumbnail_url: None,
            download_url: None,
        })
    }

    async fn create_directory(&self, location: &Location) -> Result<(), String> {
        let authority = location.authority()
            .ok_or_else(|| "SMB path requires server".to_string())?;

        let (hostname, share, path) = parse_smb_path(authority, location.path())?;
        let client = get_connection(&hostname, &share)?;

        client.mkdir(&path, pavao::SmbMode::from(0o755))
            .map_err(|e| format!("Failed to create directory: {}", e))
    }

    async fn delete(&self, location: &Location) -> Result<(), String> {
        let authority = location.authority()
            .ok_or_else(|| "SMB path requires server".to_string())?;

        let (hostname, share, path) = parse_smb_path(authority, location.path())?;
        let client = get_connection(&hostname, &share)?;

        // Check if it's a directory or file
        let stat = client.stat(&path)
            .map_err(|e| format!("Failed to stat path: {}", e))?;

        if stat.is_dir() {
            client.rmdir(&path)
                .map_err(|e| format!("Failed to delete directory: {}", e))
        } else {
            client.unlink(&path)
                .map_err(|e| format!("Failed to delete file: {}", e))
        }
    }

    async fn rename(&self, from: &Location, to: &Location) -> Result<(), String> {
        let from_authority = from.authority()
            .ok_or_else(|| "SMB path requires server".to_string())?;
        let to_authority = to.authority()
            .ok_or_else(|| "SMB path requires server".to_string())?;

        if from_authority != to_authority {
            return Err("Cannot rename across different servers".to_string());
        }

        let (hostname, share, from_path) = parse_smb_path(from_authority, from.path())?;
        let (_, to_share, to_path) = parse_smb_path(to_authority, to.path())?;

        if share != to_share {
            return Err("Cannot rename across different shares".to_string());
        }

        let client = get_connection(&hostname, &share)?;

        client.rename(&from_path, &to_path)
            .map_err(|e| format!("Failed to rename: {}", e))
    }

    async fn copy(&self, _from: &Location, _to: &Location) -> Result<(), String> {
        // SMB doesn't have native copy - would need to read+write
        Err("Copy not yet implemented for SMB".to_string())
    }
}
```

**Step 3: Verify cargo check passes**

Run: `cd src-tauri && cargo check`
Expected: Compilation succeeds

**Step 4: Commit**

```bash
git add src-tauri/src/locations/smb/
git commit -m "feat(smb): implement directory listing and file operations"
```

---

## Task 5: Add frontend TypeScript types and utilities

**Files:**
- Create: `src/utils/smbPath.ts`
- Modify: `src/types/index.ts`

**Step 1: Create smbPath.ts utility**

Create `src/utils/smbPath.ts`:

```typescript
/**
 * Check if a path is an SMB network path
 */
export function isSmbPath(path: string): boolean {
  return path.startsWith('smb://');
}

/**
 * Extract server hostname from SMB path
 */
export function parseSmbServer(path: string): string | null {
  const match = path.match(/^smb:\/\/([^/]+)/);
  return match ? match[1] : null;
}

/**
 * Extract share name from SMB path
 */
export function parseSmbShare(path: string): string | null {
  const match = path.match(/^smb:\/\/[^/]+\/([^/]+)/);
  return match ? match[1] : null;
}

/**
 * Build an SMB path from components
 */
export function buildSmbPath(server: string, share: string, path?: string): string {
  const basePath = `smb://${server}/${share}`;
  if (!path || path === '/') {
    return basePath;
  }
  const cleanPath = path.startsWith('/') ? path.slice(1) : path;
  return `${basePath}/${cleanPath}`;
}
```

**Step 2: Add SMB types to types/index.ts**

Add to `src/types/index.ts`:

```typescript
// SMB Server types
export interface SmbServerInfo {
  hostname: string;
  username: string;
  domain?: string;
}
```

**Step 3: Run frontend build to verify**

Run: `npm run build`
Expected: Build succeeds

**Step 4: Commit**

```bash
git add src/utils/smbPath.ts src/types/index.ts
git commit -m "feat(smb): add frontend TypeScript types and utilities"
```

---

## Task 6: Add SMB server management to Zustand store

**Files:**
- Modify: `src/store/useAppStore.ts`

**Step 1: Add SMB state and actions to store**

Add to the store state interface:

```typescript
// SMB state
smbServers: SmbServerInfo[];
```

Add to the store actions:

```typescript
// SMB actions
loadSmbServers: () => Promise<void>;
addSmbServer: (hostname: string, username: string, password: string, domain?: string) => Promise<void>;
removeSmbServer: (hostname: string) => Promise<void>;
```

Add implementation in the store create function:

```typescript
smbServers: [],

loadSmbServers: async () => {
  try {
    const servers = await invoke<SmbServerInfo[]>('get_smb_servers');
    set({ smbServers: servers });
  } catch (error) {
    console.error('Failed to load SMB servers:', error);
  }
},

addSmbServer: async (hostname: string, username: string, password: string, domain?: string) => {
  try {
    const server = await invoke<SmbServerInfo>('add_smb_server', {
      hostname,
      username,
      password,
      domain,
    });
    set((state) => ({
      smbServers: [...state.smbServers.filter(s => s.hostname !== hostname), server],
    }));
  } catch (error) {
    console.error('Failed to add SMB server:', error);
    throw error;
  }
},

removeSmbServer: async (hostname: string) => {
  try {
    await invoke('remove_smb_server', { hostname });
    set((state) => ({
      smbServers: state.smbServers.filter(s => s.hostname !== hostname),
    }));
  } catch (error) {
    console.error('Failed to remove SMB server:', error);
    throw error;
  }
},
```

**Step 2: Add import for SmbServerInfo**

Add to imports at top:

```typescript
import type { SmbServerInfo } from '../types';
```

**Step 3: Run tests**

Run: `npm run test:run`
Expected: Existing tests pass

**Step 4: Commit**

```bash
git add src/store/useAppStore.ts
git commit -m "feat(smb): add SMB server management to Zustand store"
```

---

## Task 7: Add SMB section to sidebar

**Files:**
- Modify: `src/components/Sidebar.tsx`

**Step 1: Add SMB servers section to sidebar**

Find where Google Drive accounts are rendered and add a similar section for SMB:

```tsx
{/* SMB Servers */}
{smbServers.length > 0 && (
  <div className="mb-4">
    <div className="text-xs font-medium text-gray-500 dark:text-gray-400 px-2 mb-1">
      Network
    </div>
    {smbServers.map((server) => (
      <SidebarItem
        key={server.hostname}
        icon={<HardDrives size={16} weight="fill" />}
        label={server.hostname}
        path={`smb://${server.hostname}`}
        isActive={currentPath.startsWith(`smb://${server.hostname}`)}
        onNavigate={() => handleNavigate(`smb://${server.hostname}`)}
      />
    ))}
  </div>
)}
```

**Step 2: Add SMB state selector**

Add to the component's state selectors:

```typescript
const smbServers = useAppStore((state) => state.smbServers);
const loadSmbServers = useAppStore((state) => state.loadSmbServers);
```

**Step 3: Load SMB servers on mount**

Add to the useEffect that loads data on mount:

```typescript
loadSmbServers();
```

**Step 4: Run frontend build**

Run: `npm run build`
Expected: Build succeeds

**Step 5: Commit**

```bash
git add src/components/Sidebar.tsx
git commit -m "feat(smb): add SMB servers section to sidebar"
```

---

## Task 8: Add "Add SMB Server" dialog

**Files:**
- Create: `src/components/AddSmbServerDialog.tsx`
- Modify: `src/components/Sidebar.tsx`

**Step 1: Create AddSmbServerDialog component**

Create `src/components/AddSmbServerDialog.tsx`:

```tsx
import { useState } from 'react';
import { X, HardDrives, CircleNotch } from '@phosphor-icons/react';
import { useAppStore } from '../store/useAppStore';
import { invoke } from '@tauri-apps/api/core';

interface AddSmbServerDialogProps {
  isOpen: boolean;
  onClose: () => void;
}

export function AddSmbServerDialog({ isOpen, onClose }: AddSmbServerDialogProps) {
  const addSmbServer = useAppStore((state) => state.addSmbServer);

  const [hostname, setHostname] = useState('');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [domain, setDomain] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    setIsLoading(true);

    try {
      // Test connection first
      await invoke('test_smb_connection', {
        hostname,
        username,
        password,
        domain: domain || undefined,
      });

      // If successful, add the server
      await addSmbServer(hostname, username, password, domain || undefined);

      // Reset form and close
      setHostname('');
      setUsername('');
      setPassword('');
      setDomain('');
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setIsLoading(false);
    }
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
      <div className="bg-white dark:bg-gray-800 rounded-lg shadow-xl w-full max-w-md">
        <div className="flex items-center justify-between p-4 border-b border-gray-200 dark:border-gray-700">
          <div className="flex items-center gap-2">
            <HardDrives size={20} weight="fill" className="text-blue-500" />
            <h2 className="text-lg font-semibold">Add Network Share</h2>
          </div>
          <button
            onClick={onClose}
            className="p-1 rounded hover:bg-gray-100 dark:hover:bg-gray-700"
          >
            <X size={20} />
          </button>
        </div>

        <form onSubmit={handleSubmit} className="p-4 space-y-4">
          <div>
            <label className="block text-sm font-medium mb-1">Server Address</label>
            <input
              type="text"
              value={hostname}
              onChange={(e) => setHostname(e.target.value)}
              placeholder="server.local or 192.168.1.100"
              className="w-full px-3 py-2 border rounded-lg dark:bg-gray-700 dark:border-gray-600"
              required
            />
          </div>

          <div>
            <label className="block text-sm font-medium mb-1">Username</label>
            <input
              type="text"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              placeholder="username"
              className="w-full px-3 py-2 border rounded-lg dark:bg-gray-700 dark:border-gray-600"
              required
            />
          </div>

          <div>
            <label className="block text-sm font-medium mb-1">Password</label>
            <input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              className="w-full px-3 py-2 border rounded-lg dark:bg-gray-700 dark:border-gray-600"
              required
            />
          </div>

          <div>
            <label className="block text-sm font-medium mb-1">Domain (optional)</label>
            <input
              type="text"
              value={domain}
              onChange={(e) => setDomain(e.target.value)}
              placeholder="WORKGROUP"
              className="w-full px-3 py-2 border rounded-lg dark:bg-gray-700 dark:border-gray-600"
            />
          </div>

          {error && (
            <div className="text-red-500 text-sm">{error}</div>
          )}

          <div className="flex justify-end gap-2 pt-2">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-2 rounded-lg hover:bg-gray-100 dark:hover:bg-gray-700"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={isLoading}
              className="px-4 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600 disabled:opacity-50 flex items-center gap-2"
            >
              {isLoading && <CircleNotch size={16} className="animate-spin" />}
              Connect
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
```

**Step 2: Add dialog trigger to Sidebar**

Add import and state for dialog in Sidebar.tsx:

```tsx
import { AddSmbServerDialog } from './AddSmbServerDialog';

// In component:
const [showAddSmbDialog, setShowAddSmbDialog] = useState(false);
```

Add button after SMB servers section:

```tsx
<button
  onClick={() => setShowAddSmbDialog(true)}
  className="flex items-center gap-2 w-full px-2 py-1 text-sm text-gray-600 dark:text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-800 rounded"
>
  <Plus size={16} />
  Add Network Share
</button>

<AddSmbServerDialog
  isOpen={showAddSmbDialog}
  onClose={() => setShowAddSmbDialog(false)}
/>
```

**Step 3: Run frontend build**

Run: `npm run build`
Expected: Build succeeds

**Step 4: Commit**

```bash
git add src/components/AddSmbServerDialog.tsx src/components/Sidebar.tsx
git commit -m "feat(smb): add dialog for connecting to SMB servers"
```

---

## Task 9: Handle share enumeration at server root

**Files:**
- Modify: `src-tauri/src/locations/smb/mod.rs`

**Step 1: Add share enumeration when path is just the server**

Update the `read_directory` method to handle the case when only server is specified (no share):

```rust
async fn read_directory(&self, location: &Location) -> Result<ProviderDirectoryEntries, String> {
    let authority = location.authority()
        .ok_or_else(|| "SMB path requires server: smb://server/share/path".to_string())?;

    let path = location.path();

    // If path is just "/" or empty, enumerate shares
    if path == "/" || path.is_empty() {
        return self.list_shares(authority).await;
    }

    let (hostname, share, dir_path) = parse_smb_path(authority, path)?;

    // ... rest of existing implementation
}

async fn list_shares(&self, hostname: &str) -> Result<ProviderDirectoryEntries, String> {
    let creds = auth::get_server_credentials(hostname)?;

    let smb_url = format!("smb://{}", hostname);

    let mut credentials = SmbCredentials::default()
        .server(&smb_url)
        .share("/")  // Root share for enumeration
        .username(&creds.username)
        .password(&creds.password);

    if let Some(domain) = &creds.domain {
        credentials = credentials.workgroup(domain);
    }

    let client = SmbClient::new(credentials, SmbOptions::default())
        .map_err(|e| format!("Failed to connect: {}", e))?;

    // List shares (directories at the root)
    let entries = client.list_dir("/")
        .map_err(|e| format!("Failed to enumerate shares: {}", e))?;

    let items: Vec<FileItem> = entries
        .filter_map(|entry| {
            let name = entry.name();
            // Skip hidden shares (ending in $) and special entries
            if name == "." || name == ".." || name.ends_with('$') {
                return None;
            }

            Some(FileItem {
                name: name.to_string(),
                path: format!("smb://{}/{}", hostname, name),
                is_dir: true,
                is_hidden: false,
                size: 0,
                modified: Utc::now().to_rfc3339(),
                extension: None,
                is_symlink: false,
                symlink_target: None,
                remote_id: None,
                thumbnail_url: None,
                download_url: None,
            })
        })
        .collect();

    Ok(ProviderDirectoryEntries {
        location: LocationSummary::new(
            "smb",
            Some(hostname.to_string()),
            "/",
            format!("smb://{}", hostname),
        ),
        entries: items,
    })
}
```

**Step 2: Verify cargo check passes**

Run: `cd src-tauri && cargo check`
Expected: Compilation succeeds

**Step 3: Commit**

```bash
git add src-tauri/src/locations/smb/mod.rs
git commit -m "feat(smb): add share enumeration at server root"
```

---

## Task 10: Add URL credential parsing support

**Files:**
- Modify: `src-tauri/src/locations/smb/mod.rs`
- Modify: `src-tauri/src/locations/mod.rs`

**Step 1: Add URL credential extraction**

Add function to extract credentials from URL format `smb://user:pass@server/share`:

```rust
/// Extract credentials from SMB URL if present
/// Format: smb://user:pass@server/share or smb://domain;user:pass@server/share
pub fn extract_url_credentials(url: &str) -> Option<(String, String, String, Option<String>)> {
    // Parse smb://[domain;]user:pass@server/...
    let without_scheme = url.strip_prefix("smb://")?;

    let at_pos = without_scheme.find('@')?;
    let auth_part = &without_scheme[..at_pos];
    let server_part = &without_scheme[at_pos + 1..];

    // Extract server (before first /)
    let server = server_part.split('/').next()?.to_string();

    // Check for domain;user:pass format
    let (domain, user_pass) = if auth_part.contains(';') {
        let mut parts = auth_part.splitn(2, ';');
        (Some(parts.next()?.to_string()), parts.next()?)
    } else {
        (None, auth_part)
    };

    // Split user:pass
    let colon_pos = user_pass.find(':')?;
    let username = user_pass[..colon_pos].to_string();
    let password = user_pass[colon_pos + 1..].to_string();

    // URL decode the parts
    let username = urlencoding::decode(&username).ok()?.to_string();
    let password = urlencoding::decode(&password).ok()?.to_string();
    let domain = domain.and_then(|d| urlencoding::decode(&d).ok().map(|s| s.to_string()));

    Some((server, username, password, domain))
}

/// Strip credentials from URL for display/storage
pub fn strip_url_credentials(url: &str) -> String {
    if let Some(without_scheme) = url.strip_prefix("smb://") {
        if let Some(at_pos) = without_scheme.find('@') {
            return format!("smb://{}", &without_scheme[at_pos + 1..]);
        }
    }
    url.to_string()
}
```

**Step 2: Update connection handling to auto-save URL credentials**

When connecting with URL credentials, automatically save them:

```rust
pub fn get_connection(hostname: &str, share: &str) -> Result<Arc<SmbClient>, String> {
    // ... existing code ...
}

/// Connect using URL with embedded credentials
pub fn connect_with_url_credentials(url: &str) -> Result<(), String> {
    if let Some((server, username, password, domain)) = extract_url_credentials(url) {
        // Save credentials
        auth::add_smb_server(server, username, password, domain)?;
        Ok(())
    } else {
        Err("No credentials in URL".to_string())
    }
}
```

**Step 3: Verify cargo check passes**

Run: `cd src-tauri && cargo check`
Expected: Compilation succeeds

**Step 4: Commit**

```bash
git add src-tauri/src/locations/smb/
git commit -m "feat(smb): add URL credential parsing support"
```

---

## Task 11: Full build verification and cleanup

**Files:** All modified files

**Step 1: Run frontend build**

Run: `npm run build`
Expected: Build succeeds with no errors or warnings

**Step 2: Run Rust build**

Run: `cd src-tauri && cargo build`
Expected: Build succeeds with no errors or warnings

**Step 3: Run all tests**

Run: `npm run test:run`
Expected: All tests pass

**Step 4: Run cargo test**

Run: `cd src-tauri && cargo test`
Expected: All tests pass

**Step 5: Final commit if any fixes were needed**

```bash
git add -A
git commit -m "chore(smb): build verification and cleanup"
```

---

## Summary

This implementation plan adds SMB network share support through:

1. **Backend (Rust)**
   - `SmbProvider` implementing `LocationProvider` trait
   - Credential storage in `~/.config/marlin/smb-servers.json`
   - Connection management with `pavao` crate
   - Share enumeration and directory listing
   - File operations (create, delete, rename)

2. **Frontend (TypeScript/React)**
   - SMB path utilities
   - Zustand store integration
   - Sidebar "Network" section
   - "Add SMB Server" dialog

3. **Platform handling**
   - macOS/Linux: Use `pavao` (libsmbclient wrapper)
   - Windows: Native UNC paths via `FileSystemProvider`

**Dependencies:**
- `pavao = "0.2"` (macOS/Linux only)
- Requires `brew install samba` on macOS or `apt install libsmbclient-dev` on Linux
