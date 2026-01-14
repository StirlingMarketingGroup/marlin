# Google Drive URL Paste Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Allow users to paste Google Drive URLs in the PathBar and navigate directly to those folders.

**Architecture:** Detect Google Drive URLs in `navigateTo`, extract folder ID, resolve to a navigable path via backend API call that tries each connected account, then navigate. Shared folders use a special `@id/FOLDER_ID` path segment.

**Tech Stack:** TypeScript (frontend), Rust (backend), Google Drive API v3

---

## Task 1: Add URL Parsing Utility

**Files:**

- Create: `src/utils/googleDriveUrl.ts`
- Test: `src/utils/googleDriveUrl.test.ts`

**Step 1: Write the failing test**

```typescript
// src/utils/googleDriveUrl.test.ts
import { describe, it, expect } from 'vitest';
import { parseGoogleDriveUrl } from './googleDriveUrl';

describe('parseGoogleDriveUrl', () => {
  it('extracts ID from /drive/folders/ URL', () => {
    const url = 'https://drive.google.com/drive/folders/1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS';
    expect(parseGoogleDriveUrl(url)).toBe('1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS');
  });

  it('extracts ID from /open?id= URL', () => {
    const url = 'https://drive.google.com/open?id=1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS&usp=drive_fs';
    expect(parseGoogleDriveUrl(url)).toBe('1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS');
  });

  it('extracts ID from /file/d/ URL', () => {
    const url = 'https://drive.google.com/file/d/1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS/view';
    expect(parseGoogleDriveUrl(url)).toBe('1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS');
  });

  it('extracts ID from URL with account index', () => {
    const url = 'https://drive.google.com/drive/u/0/folders/1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS';
    expect(parseGoogleDriveUrl(url)).toBe('1Buzv1qFiKf79xF_RE91MfAcMxPnu6CWS');
  });

  it('returns null for non-Google Drive URLs', () => {
    expect(parseGoogleDriveUrl('https://example.com/folder')).toBeNull();
    expect(parseGoogleDriveUrl('/Users/home/folder')).toBeNull();
    expect(parseGoogleDriveUrl('gdrive://email/My Drive')).toBeNull();
  });

  it('returns null for malformed Google Drive URLs', () => {
    expect(parseGoogleDriveUrl('https://drive.google.com/drive/folders/')).toBeNull();
    expect(parseGoogleDriveUrl('https://drive.google.com/open')).toBeNull();
  });
});
```

**Step 2: Run test to verify it fails**

Run: `npm test -- src/utils/googleDriveUrl.test.ts`
Expected: FAIL with "Cannot find module"

**Step 3: Write minimal implementation**

```typescript
// src/utils/googleDriveUrl.ts

/**
 * Parse a Google Drive URL and extract the file/folder ID.
 * Supports various URL formats:
 * - https://drive.google.com/drive/folders/ID
 * - https://drive.google.com/drive/u/0/folders/ID
 * - https://drive.google.com/open?id=ID
 * - https://drive.google.com/file/d/ID/view
 *
 * @returns The extracted ID or null if not a valid Google Drive URL
 */
export function parseGoogleDriveUrl(url: string): string | null {
  // Must be a drive.google.com URL
  if (!url.includes('drive.google.com')) {
    return null;
  }

  // Try /drive/folders/ID or /drive/u/N/folders/ID
  const foldersMatch = url.match(/\/folders\/([a-zA-Z0-9_-]+)/);
  if (foldersMatch?.[1]) {
    return foldersMatch[1];
  }

  // Try /file/d/ID
  const fileMatch = url.match(/\/file\/d\/([a-zA-Z0-9_-]+)/);
  if (fileMatch?.[1]) {
    return fileMatch[1];
  }

  // Try ?id=ID
  const idMatch = url.match(/[?&]id=([a-zA-Z0-9_-]+)/);
  if (idMatch?.[1]) {
    return idMatch[1];
  }

  return null;
}
```

**Step 4: Run test to verify it passes**

Run: `npm test -- src/utils/googleDriveUrl.test.ts`
Expected: PASS

**Step 5: Commit**

```bash
git add src/utils/googleDriveUrl.ts src/utils/googleDriveUrl.test.ts
git commit -m "feat(gdrive): add URL parsing utility for Google Drive links"
```

---

## Task 2: Add Backend Command to Resolve Folder ID

**Files:**

- Modify: `src-tauri/src/locations/gdrive/provider.rs`
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs`

**Step 1: Add resolve function to provider.rs**

Add this function to `provider.rs`:

```rust
/// Resolve a Google Drive folder ID to a navigable path.
/// Returns (email, path) where path is either:
/// - "/My Drive/path/to/folder" for items in My Drive
/// - "/@id/FOLDER_ID" for shared items not in My Drive hierarchy
pub async fn resolve_folder_id(
    accounts: &[String],
    folder_id: &str,
) -> Result<(String, String, String), String> {
    for email in accounts {
        match try_resolve_folder_id(email, folder_id).await {
            Ok((path, name)) => return Ok((email.clone(), path, name)),
            Err(e) => {
                log::info!("Account {} cannot access folder {}: {}", email, folder_id, e);
                continue;
            }
        }
    }
    Err(format!("No connected account has access to folder {}", folder_id))
}

async fn try_resolve_folder_id(email: &str, folder_id: &str) -> Result<(String, String), String> {
    let access_token = ensure_valid_token(email).await?;
    let client = reqwest::Client::new();

    // Get folder metadata
    let url = format!(
        "https://www.googleapis.com/drive/v3/files/{}?fields=id,name,parents,mimeType",
        folder_id
    );

    let response = client
        .get(&url)
        .bearer_auth(&access_token)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("API error: {}", response.status()));
    }

    let metadata: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    // Verify it's a folder
    let mime_type = metadata["mimeType"].as_str().unwrap_or("");
    if mime_type != "application/vnd.google-apps.folder" {
        return Err("ID refers to a file, not a folder".to_string());
    }

    let name = metadata["name"].as_str().unwrap_or("Unknown").to_string();

    // Try to build full path by traversing parents
    if let Some(parents) = metadata["parents"].as_array() {
        if let Some(parent_id) = parents.first().and_then(|p| p.as_str()) {
            match build_path_from_parents(email, folder_id, &access_token).await {
                Ok(path) => return Ok((path, name)),
                Err(_) => {
                    // Can't build path, use @id notation
                    return Ok((format!("/@id/{}", folder_id), name));
                }
            }
        }
    }

    // No parents means it's a root or shared item
    Ok((format!("/@id/{}", folder_id), name))
}

async fn build_path_from_parents(
    email: &str,
    folder_id: &str,
    access_token: &str,
) -> Result<String, String> {
    let client = reqwest::Client::new();
    let mut path_parts: Vec<String> = Vec::new();
    let mut current_id = folder_id.to_string();

    // Traverse up to 20 levels (safety limit)
    for _ in 0..20 {
        let url = format!(
            "https://www.googleapis.com/drive/v3/files/{}?fields=id,name,parents",
            current_id
        );

        let response = client
            .get(&url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if !response.status().is_success() {
            return Err("Cannot access parent".to_string());
        }

        let metadata: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse: {}", e))?;

        let name = metadata["name"].as_str().unwrap_or("Unknown");

        // Check if this is the root "My Drive"
        if let Some(parents) = metadata["parents"].as_array() {
            if parents.is_empty() {
                // This is a root
                path_parts.reverse();
                return Ok(format!("/{}/{}", name, path_parts.join("/")));
            }
            path_parts.push(name.to_string());
            current_id = parents[0].as_str().unwrap_or("").to_string();
        } else {
            // No parents - this is root
            path_parts.reverse();
            return Ok(format!("/{}/{}", name, path_parts.join("/")));
        }
    }

    Err("Path too deep".to_string())
}
```

**Step 2: Add command in commands.rs**

```rust
#[command]
pub async fn resolve_gdrive_folder_url(
    folder_id: String,
    accounts: Vec<String>,
) -> Result<(String, String, String), String> {
    crate::locations::gdrive::provider::resolve_folder_id(&accounts, &folder_id).await
}
```

**Step 3: Register command in lib.rs**

Add to the `invoke_handler` macro:

```rust
commands::resolve_gdrive_folder_url,
```

**Step 4: Build and verify**

Run: `cd src-tauri && cargo build`
Expected: Clean build

**Step 5: Commit**

```bash
git add src-tauri/src/locations/gdrive/provider.rs src-tauri/src/commands.rs src-tauri/src/lib.rs
git commit -m "feat(gdrive): add backend command to resolve folder ID to path"
```

---

## Task 3: Update navigateTo to Handle Google Drive URLs

**Files:**

- Modify: `src/store/useAppStore.ts`

**Step 1: Import the URL parser**

At top of file, add:

```typescript
import { parseGoogleDriveUrl } from '@/utils/googleDriveUrl';
```

**Step 2: Update navigateTo function**

Replace the `navigateTo` implementation with async version that handles Google Drive URLs:

```typescript
navigateTo: async (path) => {
  const { pathHistory, historyIndex, googleAccounts } = get();
  const trimmed = path.trim();

  // Check if this is a Google Drive URL
  const gdriveId = parseGoogleDriveUrl(trimmed);
  if (gdriveId) {
    // Resolve the folder ID to a path
    const accountEmails = googleAccounts.map(a => a.email);
    if (accountEmails.length === 0) {
      console.error('No Google accounts connected');
      // Could show a toast/dialog here
      return;
    }

    try {
      const [email, resolvedPath, folderName] = await invoke<[string, string, string]>(
        'resolve_gdrive_folder_url',
        { folderId: gdriveId, accounts: accountEmails }
      );

      const fullPath = `gdrive://${email}${resolvedPath}`;
      console.info('[navigateTo] Resolved Google Drive URL to:', fullPath);

      const newHistory = [...pathHistory.slice(0, historyIndex + 1), fullPath];
      set({
        currentPath: fullPath,
        currentLocationRaw: fullPath,
        pathHistory: newHistory,
        historyIndex: newHistory.length - 1,
        filterText: '',
        showFilterInput: false,
      });
      void get().refreshGitStatus({ path: fullPath });
      return;
    } catch (error) {
      console.error('Failed to resolve Google Drive URL:', error);
      // Show error to user
      const { message } = await import('@tauri-apps/plugin-dialog');
      await message(
        `Could not access this Google Drive folder. Make sure the folder exists and you have access with one of your connected accounts.`,
        { title: 'Cannot Open Folder', kind: 'error' }
      );
      return;
    }
  }

  // Handle gdrive:// URIs - don't use normalizePath for these
  let norm: string;
  let locationRaw: string;
  if (trimmed.startsWith('gdrive://')) {
    norm = trimmed;
    locationRaw = trimmed;
  } else {
    norm = normalizePath(path);
    locationRaw = toFileUri(norm);
  }

  const newHistory = [...pathHistory.slice(0, historyIndex + 1), norm];
  set({
    currentPath: norm,
    currentLocationRaw: locationRaw,
    pathHistory: newHistory,
    historyIndex: newHistory.length - 1,
    filterText: '',
    showFilterInput: false,
  });
  void get().refreshGitStatus({ path: norm });
},
```

**Step 3: Update type signature**

In the interface, change:

```typescript
navigateTo: (path: string) => Promise<void>;
```

**Step 4: Build and verify**

Run: `npm run build`
Expected: Clean build

**Step 5: Commit**

```bash
git add src/store/useAppStore.ts
git commit -m "feat(gdrive): handle Google Drive URLs in navigateTo"
```

---

## Task 4: Support @id Path Segments in Provider

**Files:**

- Modify: `src-tauri/src/locations/gdrive/provider.rs`

**Step 1: Update read_directory to handle @id paths**

In the `read_directory` function, add handling for `@id` paths at the start:

```rust
pub async fn read_directory(email: &str, path: &str) -> Result<Vec<GDriveFileEntry>, String> {
    // Handle @id/FOLDER_ID paths (for shared folders)
    if path.starts_with("/@id/") {
        let folder_id = &path[5..]; // Skip "/@id/"
        return read_directory_by_id(email, folder_id).await;
    }

    // ... rest of existing implementation
}

async fn read_directory_by_id(email: &str, folder_id: &str) -> Result<Vec<GDriveFileEntry>, String> {
    let access_token = ensure_valid_token(email).await?;
    let client = reqwest::Client::new();

    let mut all_entries = Vec::new();
    let mut page_token: Option<String> = None;

    loop {
        let mut url = format!(
            "https://www.googleapis.com/drive/v3/files?q='{}'+in+parents+and+trashed=false&fields=nextPageToken,files(id,name,mimeType,size,modifiedTime,thumbnailLink,iconLink)&pageSize=1000",
            folder_id
        );

        if let Some(token) = &page_token {
            url.push_str(&format!("&pageToken={}", token));
        }

        let response = client
            .get(&url)
            .bearer_auth(&access_token)
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("API error: {}", response.status()));
        }

        let data: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse: {}", e))?;

        if let Some(files) = data["files"].as_array() {
            for file in files {
                let entry = parse_file_entry(file, &format!("/@id/{}", folder_id));
                all_entries.push(entry);
            }
        }

        page_token = data["nextPageToken"].as_str().map(String::from);
        if page_token.is_none() {
            break;
        }
    }

    Ok(all_entries)
}
```

**Step 2: Update parent path calculation for @id paths**

In the path building logic, handle @id paths:

```rust
fn get_parent_path(path: &str) -> String {
    if path.starts_with("/@id/") {
        // For @id paths, we can't go up (would need to track parent IDs)
        // Return the same path or a sensible default
        return "/My Drive".to_string();
    }
    // ... existing logic
}
```

**Step 3: Build and verify**

Run: `cd src-tauri && cargo build`
Expected: Clean build

**Step 4: Commit**

```bash
git add src-tauri/src/locations/gdrive/provider.rs
git commit -m "feat(gdrive): support @id path segments for shared folders"
```

---

## Task 5: Manual Testing

**Test cases:**

1. Paste `https://drive.google.com/drive/folders/FOLDER_ID` for a folder in My Drive
   - Should navigate to `gdrive://email/My Drive/path/to/folder`

2. Paste `https://drive.google.com/open?id=FOLDER_ID&usp=drive_fs` for a shared folder
   - Should navigate to `gdrive://email/@id/FOLDER_ID`
   - Should show folder contents

3. Paste a URL for a folder you don't have access to
   - Should show error dialog

4. Paste a regular path (not a Google Drive URL)
   - Should work as before

5. Paste a URL that points to a file (not folder)
   - Should show appropriate error

**Commit:**

```bash
git commit --allow-empty -m "test(gdrive): manual testing of URL paste feature complete"
```

---

## Summary

This implementation:

1. Parses various Google Drive URL formats to extract folder IDs
2. Tries each connected account to find one with access
3. Resolves the folder ID to either a full path (My Drive) or @id notation (shared)
4. Handles errors gracefully with user-friendly messages
5. Works seamlessly in the existing PathBar
