use async_trait::async_trait;
use chrono::Utc;
use google_drive3::api::File as DriveFile;
use google_drive3::DriveHub;
use google_drive3::hyper::client::HttpConnector;
use google_drive3::hyper_rustls::HttpsConnector;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration as StdDuration, Instant};

use super::auth::{ensure_valid_token, get_all_accounts};
use crate::fs_utils::FileItem;
use crate::locations::{
    Location, LocationCapabilities, LocationProvider, LocationSummary, ProviderDirectoryEntries,
};

/// Virtual root folder names
const VIRTUAL_MY_DRIVE: &str = "My Drive";
const VIRTUAL_SHARED: &str = "Shared with me";
const VIRTUAL_STARRED: &str = "Starred";
const VIRTUAL_RECENT: &str = "Recent";
const VIRTUAL_BY_ID: &str = "id";  // For direct ID-based navigation

/// Cache entry with TTL
#[allow(dead_code)]
struct CacheEntry<T> {
    data: T,
    created: Instant,
}

impl<T> CacheEntry<T> {
    #[allow(dead_code)]
    fn new(data: T) -> Self {
        Self {
            data,
            created: Instant::now(),
        }
    }

    #[allow(dead_code)]
    fn is_expired(&self, ttl: StdDuration) -> bool {
        self.created.elapsed() > ttl
    }
}

/// Directory listing cache
static DIR_CACHE: Lazy<RwLock<HashMap<String, CacheEntry<Vec<FileItem>>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// File ID to path cache
static PATH_CACHE: Lazy<RwLock<HashMap<String, CacheEntry<String>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// Cache TTL (30 seconds)
const CACHE_TTL: StdDuration = StdDuration::from_secs(30);

type DriveHubType = DriveHub<HttpsConnector<HttpConnector>>;

/// Google Drive location provider
pub struct GoogleDriveProvider;

impl Default for GoogleDriveProvider {
    fn default() -> Self {
        Self
    }
}

impl GoogleDriveProvider {
    /// Create a Drive hub for the given account
    async fn create_hub(&self, email: &str) -> Result<DriveHubType, String> {
        let access_token = ensure_valid_token(email).await?;

        // Use the hyper client compatible with google_drive3 (hyper 0.14)
        let connector = google_drive3::hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .map_err(|e| format!("Failed to load native roots: {}", e))?
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .build();

        let client = google_drive3::hyper::Client::builder().build(connector);

        // Create an authenticator that uses our stored token
        let auth = google_drive3::oauth2::AccessTokenAuthenticator::builder(access_token)
            .build()
            .await
            .map_err(|e| format!("Failed to create authenticator: {}", e))?;

        Ok(DriveHub::new(client, auth))
    }

    /// Parse the account email from the location authority
    fn get_account_email(&self, location: &Location) -> Result<String, String> {
        location
            .authority()
            .map(|s| s.to_string())
            .ok_or_else(|| "Google Drive location requires an account email".to_string())
    }

    /// Parse the virtual path components
    fn parse_virtual_path<'a>(&self, path: &'a str) -> (Option<&'a str>, Vec<&'a str>) {
        let trimmed = path.trim_start_matches('/');
        if trimmed.is_empty() {
            return (None, vec![]);
        }

        let parts: Vec<&str> = trimmed.split('/').collect();
        if parts.is_empty() {
            return (None, vec![]);
        }

        let root = parts[0];
        let rest = if parts.len() > 1 {
            parts[1..].to_vec()
        } else {
            vec![]
        };

        (Some(root), rest)
    }

    /// List the virtual root folders
    fn list_virtual_root(&self, email: &str) -> ProviderDirectoryEntries {
        let now = Utc::now();

        let entries = vec![
            FileItem {
                name: VIRTUAL_MY_DRIVE.to_string(),
                path: format!("gdrive://{}/{}", email, VIRTUAL_MY_DRIVE),
                size: 0,
                modified: now,
                is_directory: true,
                is_hidden: false,
                is_symlink: false,
                is_git_repo: false,
                extension: None,
                child_count: None,
                image_width: None,
                image_height: None,
                remote_id: None,
                thumbnail_url: None,
                download_url: None,
            },
            FileItem {
                name: VIRTUAL_SHARED.to_string(),
                path: format!("gdrive://{}/{}", email, VIRTUAL_SHARED),
                size: 0,
                modified: now,
                is_directory: true,
                is_hidden: false,
                is_symlink: false,
                is_git_repo: false,
                extension: None,
                child_count: None,
                image_width: None,
                image_height: None,
                remote_id: None,
                thumbnail_url: None,
                download_url: None,
            },
            FileItem {
                name: VIRTUAL_STARRED.to_string(),
                path: format!("gdrive://{}/{}", email, VIRTUAL_STARRED),
                size: 0,
                modified: now,
                is_directory: true,
                is_hidden: false,
                is_symlink: false,
                is_git_repo: false,
                extension: None,
                child_count: None,
                image_width: None,
                image_height: None,
                remote_id: None,
                thumbnail_url: None,
                download_url: None,
            },
            FileItem {
                name: VIRTUAL_RECENT.to_string(),
                path: format!("gdrive://{}/{}", email, VIRTUAL_RECENT),
                size: 0,
                modified: now,
                is_directory: true,
                is_hidden: false,
                is_symlink: false,
                is_git_repo: false,
                extension: None,
                child_count: None,
                image_width: None,
                image_height: None,
                remote_id: None,
                thumbnail_url: None,
                download_url: None,
            },
        ];

        ProviderDirectoryEntries {
            location: LocationSummary::new("gdrive", Some(email.to_string()), "/", email),
            entries,
        }
    }

    /// Convert a Drive file to FileItem
    /// For folders in "Shared with me", use ID-based paths for reliable navigation
    fn drive_file_to_file_item(&self, file: &DriveFile, email: &str, parent_path: &str) -> FileItem {
        let name = file.name.clone().unwrap_or_else(|| "Untitled".to_string());
        let is_folder = file.mime_type.as_deref() == Some("application/vnd.google-apps.folder");
        let file_id = file.id.clone().unwrap_or_default();

        // For folders in "Shared with me", use ID-based paths
        // This is because items appearing in shared listings aren't necessarily
        // marked as sharedWithMe=true (they could be in a shared drive)
        let is_shared_context = parent_path.contains(VIRTUAL_SHARED);
        let path = if is_folder && is_shared_context && !file_id.is_empty() {
            // Use ID-based path for reliable navigation
            format!("gdrive://{}/{}/{}", email, VIRTUAL_BY_ID, file_id)
        } else if parent_path.ends_with('/') {
            format!("gdrive://{}{}{}", email, parent_path, &name)
        } else {
            format!("gdrive://{}{}/{}", email, parent_path, &name)
        };

        let modified = file
            .modified_time
            .unwrap_or_else(Utc::now);

        let size = file.size
            .unwrap_or(0) as u64;

        let extension = if !is_folder {
            name.rsplit('.').next()
                .filter(|ext| ext.len() < 10 && !ext.contains(' '))
                .map(|s| s.to_lowercase())
        } else {
            None
        };

        // Extract thumbnail and download URLs from Google Drive
        let thumbnail_url = file.thumbnail_link.clone();
        let download_url = file.web_content_link.clone();

        FileItem {
            name,
            path,
            size,
            modified,
            is_directory: is_folder,
            is_hidden: false,
            is_symlink: false,
            is_git_repo: false,
            extension,
            child_count: None,
            image_width: None,
            image_height: None,
            remote_id: if file_id.is_empty() { None } else { Some(file_id) },
            thumbnail_url,
            download_url,
        }
    }

    /// List files in My Drive root
    async fn list_my_drive_root(&self, hub: &DriveHubType, email: &str) -> Result<Vec<FileItem>, String> {
        log::info!("Listing My Drive root for {}", email);
        let result = hub
            .files()
            .list()
            .q("'root' in parents and trashed = false")
            .page_size(1000)
            .add_scope(google_drive3::api::Scope::Full)
            .param("fields", "files(id,name,mimeType,size,modifiedTime,parents,thumbnailLink,webContentLink)")
            .doit()
            .await
            .map_err(|e| {
                log::error!("Failed to list Drive files: {}", e);
                format!("Failed to list Drive files: {}", e)
            })?;

        let files = result.1.files.unwrap_or_default();
        log::info!("Got {} files from My Drive root", files.len());
        for f in &files {
            log::info!("  - {:?}", f.name);
        }
        let parent_path = format!("/{}", VIRTUAL_MY_DRIVE);

        Ok(files
            .iter()
            .map(|f| self.drive_file_to_file_item(f, email, &parent_path))
            .collect())
    }

    /// List files in a specific folder by ID
    async fn list_folder_by_id(
        &self,
        hub: &DriveHubType,
        folder_id: &str,
        email: &str,
        parent_path: &str,
    ) -> Result<Vec<FileItem>, String> {
        let query = format!("'{}' in parents and trashed = false", folder_id);
        log::info!("list_folder_by_id: folder_id={}, query={}", folder_id, query);

        let result = hub
            .files()
            .list()
            .q(&query)
            .page_size(1000)
            .supports_all_drives(true)  // Required for shared drives
            .include_items_from_all_drives(true)  // Include shared drive items
            .add_scope(google_drive3::api::Scope::Full)
            .param("fields", "files(id,name,mimeType,size,modifiedTime,parents,thumbnailLink,webContentLink)")
            .doit()
            .await
            .map_err(|e| format!("Failed to list folder: {}", e))?;

        let files = result.1.files.unwrap_or_default();
        log::info!("  -> found {} files", files.len());

        Ok(files
            .iter()
            .map(|f| self.drive_file_to_file_item(f, email, parent_path))
            .collect())
    }

    /// List shared files
    async fn list_shared_with_me(&self, hub: &DriveHubType, email: &str) -> Result<Vec<FileItem>, String> {
        let result = hub
            .files()
            .list()
            .q("sharedWithMe = true and trashed = false")
            .page_size(1000)
            .add_scope(google_drive3::api::Scope::Full)
            .param("fields", "files(id,name,mimeType,size,modifiedTime,parents,thumbnailLink,webContentLink)")
            .doit()
            .await
            .map_err(|e| format!("Failed to list shared files: {}", e))?;

        let files = result.1.files.unwrap_or_default();
        let parent_path = format!("/{}", VIRTUAL_SHARED);

        Ok(files
            .iter()
            .map(|f| self.drive_file_to_file_item(f, email, &parent_path))
            .collect())
    }

    /// List starred files
    async fn list_starred(&self, hub: &DriveHubType, email: &str) -> Result<Vec<FileItem>, String> {
        let result = hub
            .files()
            .list()
            .q("starred = true and trashed = false")
            .page_size(1000)
            .add_scope(google_drive3::api::Scope::Full)
            .param("fields", "files(id,name,mimeType,size,modifiedTime,parents,thumbnailLink,webContentLink)")
            .doit()
            .await
            .map_err(|e| format!("Failed to list starred files: {}", e))?;

        let files = result.1.files.unwrap_or_default();
        let parent_path = format!("/{}", VIRTUAL_STARRED);

        Ok(files
            .iter()
            .map(|f| self.drive_file_to_file_item(f, email, &parent_path))
            .collect())
    }

    /// List recent files
    async fn list_recent(&self, hub: &DriveHubType, email: &str) -> Result<Vec<FileItem>, String> {
        let result = hub
            .files()
            .list()
            .q("trashed = false")
            .order_by("viewedByMeTime desc")
            .page_size(50)
            .add_scope(google_drive3::api::Scope::Full)
            .param("fields", "files(id,name,mimeType,size,modifiedTime,parents,thumbnailLink,webContentLink)")
            .doit()
            .await
            .map_err(|e| format!("Failed to list recent files: {}", e))?;

        let files = result.1.files.unwrap_or_default();
        let parent_path = format!("/{}", VIRTUAL_RECENT);

        Ok(files
            .iter()
            .map(|f| self.drive_file_to_file_item(f, email, &parent_path))
            .collect())
    }

    /// Find a file by path within My Drive
    async fn find_file_by_path(
        &self,
        hub: &DriveHubType,
        path_parts: &[&str],
    ) -> Result<Option<String>, String> {
        self.find_file_by_path_from_parent(hub, "root", path_parts).await
    }

    /// Find a file by path starting from a specific parent
    async fn find_file_by_path_from_parent(
        &self,
        hub: &DriveHubType,
        start_parent: &str,
        path_parts: &[&str],
    ) -> Result<Option<String>, String> {
        if path_parts.is_empty() {
            return Ok(Some(start_parent.to_string()));
        }

        let mut current_parent = start_parent.to_string();

        for part in path_parts {
            let query = format!(
                "'{}' in parents and name = '{}' and trashed = false",
                current_parent, part
            );

            let result = hub
                .files()
                .list()
                .q(&query)
                .page_size(1)
                .add_scope(google_drive3::api::Scope::Full)
                .param("fields", "files(id)")
                .doit()
                .await
                .map_err(|e| format!("Failed to search for file: {}", e))?;

            let files = result.1.files.unwrap_or_default();
            if let Some(file) = files.first() {
                current_parent = file.id.clone().unwrap_or_default();
            } else {
                return Ok(None);
            }
        }

        Ok(Some(current_parent))
    }

    /// Find a shared file by path
    /// First element must match a shared-with-me item by name, then navigate into children
    async fn find_shared_file_by_path(
        &self,
        hub: &DriveHubType,
        path_parts: &[&str],
    ) -> Result<Option<String>, String> {
        log::info!("find_shared_file_by_path: path_parts={:?}", path_parts);

        if path_parts.is_empty() {
            log::info!("  -> empty path parts, returning None");
            return Ok(None);
        }

        // First, find the shared item by name
        let first_name = path_parts[0];
        let query = format!(
            "sharedWithMe = true and name = '{}' and trashed = false",
            first_name
        );
        log::info!("  -> searching for shared item with query: {}", query);

        let result = hub
            .files()
            .list()
            .q(&query)
            .page_size(10)  // Get more results to see what's available
            .add_scope(google_drive3::api::Scope::Full)
            .param("fields", "files(id,name)")
            .doit()
            .await
            .map_err(|e| format!("Failed to search for shared file: {}", e))?;

        let files = result.1.files.unwrap_or_default();
        log::info!("  -> found {} shared items with name '{}'", files.len(), first_name);
        for f in &files {
            log::info!("    - id={:?}, name={:?}", f.id, f.name);
        }

        let shared_item = match files.first() {
            Some(f) => f.id.clone().unwrap_or_default(),
            None => {
                log::info!("  -> no shared item found with name '{}', returning None", first_name);
                return Ok(None);
            }
        };

        log::info!("  -> using shared item id: {}", shared_item);

        // If there's only one path part, we found it
        if path_parts.len() == 1 {
            log::info!("  -> single path part, returning shared_item");
            return Ok(Some(shared_item));
        }

        // Otherwise, navigate into children from the shared item
        log::info!("  -> navigating into children: {:?}", &path_parts[1..]);
        self.find_file_by_path_from_parent(hub, &shared_item, &path_parts[1..]).await
    }

    /// Get file metadata by ID
    async fn get_file_by_id(
        &self,
        hub: &DriveHubType,
        file_id: &str,
    ) -> Result<DriveFile, String> {
        let result = hub
            .files()
            .get(file_id)
            .supports_all_drives(true)
            .add_scope(google_drive3::api::Scope::Full)
            .param("fields", "id,name,mimeType,size,modifiedTime,parents,driveId")
            .doit()
            .await
            .map_err(|e| format!("Failed to get file: {}", e))?;

        Ok(result.1)
    }

    /// Build the full path for a file by walking up parents
    /// For items NOT in My Drive, we use ID-based paths for reliable navigation
    async fn build_file_path(
        &self,
        hub: &DriveHubType,
        file: &DriveFile,
        email: &str,
    ) -> Result<String, String> {
        log::info!("build_file_path: file={:?}, id={:?}, parents={:?}, driveId={:?}",
            file.name, file.id, file.parents, file.drive_id);

        let file_id = file.id.clone().unwrap_or_default();
        let mut path_parts = vec![file.name.clone().unwrap_or_else(|| "Untitled".to_string())];
        let mut current_parents = file.parents.clone().unwrap_or_default();

        // Walk up the parent chain to determine if this is in My Drive
        let mut is_my_drive = false;
        for i in 0..50 {
            log::info!("  iteration {}: current_parents={:?}", i, current_parents);

            if current_parents.is_empty() {
                // Not in My Drive (didn't find "root")
                log::info!("  -> Reached root without 'root' parent, not My Drive");
                break;
            }

            let parent_id = &current_parents[0];
            log::info!("  -> parent_id={}", parent_id);

            // "root" is the My Drive root
            if parent_id == "root" {
                log::info!("  -> Found 'root', this is My Drive");
                is_my_drive = true;
                path_parts.push(VIRTUAL_MY_DRIVE.to_string());
                break;
            }

            let parent = self.get_file_by_id(hub, parent_id).await?;
            log::info!("  -> parent name={:?}, id={:?}, parents={:?}",
                parent.name, parent.id, parent.parents);

            // Check if this parent is a Shared Drive root (has no parents but has drive_id)
            let parent_parents = parent.parents.clone().unwrap_or_default();
            if parent_parents.is_empty() && parent.drive_id.is_some() {
                log::info!("  -> Shared Drive root, not My Drive");
                break;
            }

            path_parts.push(parent.name.clone().unwrap_or_else(|| "Untitled".to_string()));
            current_parents = parent_parents;
        }

        let final_path = if is_my_drive {
            // For My Drive items, use the full path
            path_parts.reverse();
            format!("gdrive://{}/{}", email, path_parts.join("/"))
        } else {
            // For Shared items, use ID-based path for reliable navigation
            // This avoids the problem of paths not matching navigable structure
            format!("gdrive://{}/{}/{}", email, VIRTUAL_BY_ID, file_id)
        };

        log::info!("  -> final path: {}", final_path);
        Ok(final_path)
    }
}

#[async_trait]
impl LocationProvider for GoogleDriveProvider {
    fn scheme(&self) -> &'static str {
        "gdrive"
    }

    fn capabilities(&self, _location: &Location) -> LocationCapabilities {
        LocationCapabilities {
            scheme: "gdrive".to_string(),
            display_name: "Google Drive".to_string(),
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
    }

    async fn read_directory(&self, location: &Location) -> Result<ProviderDirectoryEntries, String> {
        let email = self.get_account_email(location)?;
        let path = location.path();

        log::info!("read_directory: email={}, path={}", email, path);

        // Virtual root - show My Drive, Shared, Starred, Recent
        let (root_folder, subpath) = self.parse_virtual_path(path);
        log::info!("  root_folder={:?}, subpath={:?}", root_folder, subpath);

        if root_folder.is_none() {
            log::info!("  -> listing virtual root");
            let result = self.list_virtual_root(&email);
            log::info!("  -> returning {} entries", result.entries.len());
            return Ok(result);
        }

        let root = root_folder.unwrap();
        log::info!("  -> root={}", root);
        let hub = self.create_hub(&email).await?;

        let entries = match root {
            VIRTUAL_MY_DRIVE => {
                if subpath.is_empty() {
                    self.list_my_drive_root(&hub, &email).await?
                } else {
                    // Find folder ID by path
                    let folder_id = self.find_file_by_path(&hub, &subpath).await?
                        .ok_or_else(|| format!("Folder not found: {}", subpath.join("/")))?;

                    self.list_folder_by_id(&hub, &folder_id, &email, path).await?
                }
            }
            VIRTUAL_SHARED => {
                if subpath.is_empty() {
                    log::info!("  -> listing shared with me root");
                    self.list_shared_with_me(&hub, &email).await?
                } else {
                    // Navigate into a shared folder
                    // For shared items, use special lookup that finds shared items first
                    log::info!("  -> finding shared file by path: {:?}", subpath);
                    let folder_id = self.find_shared_file_by_path(&hub, &subpath).await?
                        .ok_or_else(|| format!("Folder not found: {}", subpath.join("/")))?;

                    log::info!("  -> found folder_id: {}", folder_id);
                    self.list_folder_by_id(&hub, &folder_id, &email, path).await?
                }
            }
            VIRTUAL_STARRED => self.list_starred(&hub, &email).await?,
            VIRTUAL_RECENT => self.list_recent(&hub, &email).await?,
            VIRTUAL_BY_ID => {
                // Direct ID-based navigation: /id/<file_id>
                if subpath.is_empty() {
                    return Err("Missing file ID in path".to_string());
                }
                let file_id = subpath[0];
                log::info!("  -> Direct ID navigation: {}", file_id);
                self.list_folder_by_id(&hub, file_id, &email, path).await?
            }
            _ => {
                return Err(format!("Unknown virtual folder: {}", root));
            }
        };

        let display_path = if path == "/" {
            email.clone()
        } else {
            format!("{}{}", email, path)
        };

        Ok(ProviderDirectoryEntries {
            location: LocationSummary::new("gdrive", Some(email), path, display_path),
            entries,
        })
    }

    async fn get_file_metadata(&self, location: &Location) -> Result<FileItem, String> {
        let email = self.get_account_email(location)?;
        let path = location.path();

        let (root_folder, subpath) = self.parse_virtual_path(path);

        // Virtual root folders
        if let Some(root) = root_folder {
            if subpath.is_empty() {
                return Ok(FileItem {
                    name: root.to_string(),
                    path: format!("gdrive://{}{}", email, path),
                    size: 0,
                    modified: Utc::now(),
                    is_directory: true,
                    is_hidden: false,
                    is_symlink: false,
                    is_git_repo: false,
                    extension: None,
                    child_count: None,
                    image_width: None,
                    image_height: None,
                    remote_id: None,
                    thumbnail_url: None,
                    download_url: None,
                });
            }
        } else {
            // Root of account
            return Ok(FileItem {
                name: email.clone(),
                path: format!("gdrive://{}/", email),
                size: 0,
                modified: Utc::now(),
                is_directory: true,
                is_hidden: false,
                is_symlink: false,
                is_git_repo: false,
                extension: None,
                child_count: None,
                image_width: None,
                image_height: None,
                remote_id: None,
                thumbnail_url: None,
                download_url: None,
            });
        }

        let hub = self.create_hub(&email).await?;

        // Find the file by path
        let file_id = self.find_file_by_path(&hub, &subpath).await?
            .ok_or_else(|| format!("File not found: {}", path))?;

        let file = self.get_file_by_id(&hub, &file_id).await?;
        let parent_path = path.rsplit('/').skip(1).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("/");

        Ok(self.drive_file_to_file_item(&file, &email, &format!("/{}", parent_path)))
    }

    async fn create_directory(&self, location: &Location) -> Result<(), String> {
        let email = self.get_account_email(location)?;
        let path = location.path();

        let (root_folder, subpath) = self.parse_virtual_path(path);

        if root_folder != Some(VIRTUAL_MY_DRIVE) {
            return Err("Can only create folders in My Drive".to_string());
        }

        if subpath.is_empty() {
            return Err("Cannot create folder at root".to_string());
        }

        let hub = self.create_hub(&email).await?;

        // Find parent folder
        let parent_path = &subpath[..subpath.len() - 1];
        let parent_id = if parent_path.is_empty() {
            "root".to_string()
        } else {
            self.find_file_by_path(&hub, parent_path).await?
                .ok_or_else(|| "Parent folder not found".to_string())?
        };

        let folder_name = subpath.last().ok_or_else(|| "Invalid path".to_string())?;

        let folder = DriveFile {
            name: Some(folder_name.to_string()),
            mime_type: Some("application/vnd.google-apps.folder".to_string()),
            parents: Some(vec![parent_id]),
            ..Default::default()
        };

        // For folder creation (metadata-only), use upload with empty content
        let empty_stream = std::io::Cursor::new(Vec::<u8>::new());
        hub.files()
            .create(folder)
            .add_scope(google_drive3::api::Scope::Full)
            .upload(empty_stream, "application/octet-stream".parse().unwrap())
            .await
            .map_err(|e| format!("Failed to create folder: {}", e))?;

        Ok(())
    }

    async fn delete(&self, location: &Location) -> Result<(), String> {
        let email = self.get_account_email(location)?;
        let path = location.path();

        let (root_folder, subpath) = self.parse_virtual_path(path);

        if root_folder.is_none() || subpath.is_empty() {
            return Err("Cannot delete virtual root folders".to_string());
        }

        let hub = self.create_hub(&email).await?;

        let file_id = self.find_file_by_path(&hub, &subpath).await?
            .ok_or_else(|| "File not found".to_string())?;

        // Move to trash instead of permanent delete
        let update = DriveFile {
            trashed: Some(true),
            ..Default::default()
        };

        hub.files()
            .update(update, &file_id)
            .add_scope(google_drive3::api::Scope::Full)
            .doit_without_upload()
            .await
            .map_err(|e| format!("Failed to delete: {}", e))?;

        Ok(())
    }

    async fn rename(&self, from: &Location, to: &Location) -> Result<(), String> {
        let from_email = self.get_account_email(from)?;
        let to_email = self.get_account_email(to)?;

        if from_email != to_email {
            return Err("Cannot rename across accounts".to_string());
        }

        let from_path = from.path();
        let to_path = to.path();

        let (_, from_subpath) = self.parse_virtual_path(from_path);
        let (_, to_subpath) = self.parse_virtual_path(to_path);

        if from_subpath.is_empty() {
            return Err("Cannot rename virtual root folders".to_string());
        }

        let hub = self.create_hub(&from_email).await?;

        let file_id = self.find_file_by_path(&hub, &from_subpath).await?
            .ok_or_else(|| "Source file not found".to_string())?;

        let new_name = to_subpath.last()
            .ok_or_else(|| "Invalid destination path".to_string())?;

        let update = DriveFile {
            name: Some(new_name.to_string()),
            ..Default::default()
        };

        hub.files()
            .update(update, &file_id)
            .add_scope(google_drive3::api::Scope::Full)
            .doit_without_upload()
            .await
            .map_err(|e| format!("Failed to rename: {}", e))?;

        Ok(())
    }

    async fn copy(&self, from: &Location, to: &Location) -> Result<(), String> {
        let from_email = self.get_account_email(from)?;
        let to_email = self.get_account_email(to)?;

        if from_email != to_email {
            return Err("Cannot copy across accounts".to_string());
        }

        let from_path = from.path();
        let to_path = to.path();

        let (_, from_subpath) = self.parse_virtual_path(from_path);
        let (to_root, to_subpath) = self.parse_virtual_path(to_path);

        if from_subpath.is_empty() {
            return Err("Cannot copy virtual root folders".to_string());
        }

        if to_root != Some(VIRTUAL_MY_DRIVE) {
            return Err("Can only copy to My Drive".to_string());
        }

        let hub = self.create_hub(&from_email).await?;

        let file_id = self.find_file_by_path(&hub, &from_subpath).await?
            .ok_or_else(|| "Source file not found".to_string())?;

        // Find destination parent
        let dest_parent_path = &to_subpath[..to_subpath.len().saturating_sub(1)];
        let dest_parent_id = if dest_parent_path.is_empty() {
            "root".to_string()
        } else {
            self.find_file_by_path(&hub, dest_parent_path).await?
                .ok_or_else(|| "Destination folder not found".to_string())?
        };

        let new_name = to_subpath.last()
            .ok_or_else(|| "Invalid destination path".to_string())?;

        let copy_request = DriveFile {
            name: Some(new_name.to_string()),
            parents: Some(vec![dest_parent_id]),
            ..Default::default()
        };

        hub.files()
            .copy(copy_request, &file_id)
            .add_scope(google_drive3::api::Scope::Full)
            .doit()
            .await
            .map_err(|e| format!("Failed to copy: {}", e))?;

        Ok(())
    }

    async fn move_item(&self, from: &Location, to: &Location) -> Result<(), String> {
        let from_email = self.get_account_email(from)?;
        let to_email = self.get_account_email(to)?;

        if from_email != to_email {
            return Err("Cannot move across accounts".to_string());
        }

        let from_path = from.path();
        let to_path = to.path();

        let (_, from_subpath) = self.parse_virtual_path(from_path);
        let (to_root, to_subpath) = self.parse_virtual_path(to_path);

        if from_subpath.is_empty() {
            return Err("Cannot move virtual root folders".to_string());
        }

        if to_root != Some(VIRTUAL_MY_DRIVE) {
            return Err("Can only move to My Drive".to_string());
        }

        let hub = self.create_hub(&from_email).await?;

        let file_id = self.find_file_by_path(&hub, &from_subpath).await?
            .ok_or_else(|| "Source file not found".to_string())?;

        // Get current parents
        let file = self.get_file_by_id(&hub, &file_id).await?;
        let current_parents = file.parents.unwrap_or_default();

        // Find destination parent
        let dest_parent_path = &to_subpath[..to_subpath.len().saturating_sub(1)];
        let dest_parent_id = if dest_parent_path.is_empty() {
            "root".to_string()
        } else {
            self.find_file_by_path(&hub, dest_parent_path).await?
                .ok_or_else(|| "Destination folder not found".to_string())?
        };

        let new_name = to_subpath.last()
            .ok_or_else(|| "Invalid destination path".to_string())?;

        // Update with new parent and possibly new name
        let update = DriveFile {
            name: Some(new_name.to_string()),
            ..Default::default()
        };

        hub.files()
            .update(update, &file_id)
            .add_parents(&dest_parent_id)
            .remove_parents(&current_parents.join(","))
            .add_scope(google_drive3::api::Scope::Full)
            .doit_without_upload()
            .await
            .map_err(|e| format!("Failed to move: {}", e))?;

        Ok(())
    }
}

/// Resolve a Google Drive file ID to a full path, trying all connected accounts
pub async fn resolve_file_id_to_path(file_id: &str) -> Result<(String, String), String> {
    log::info!("resolve_file_id_to_path: file_id={}", file_id);
    let accounts = get_all_accounts()?;
    log::info!("  Found {} connected accounts", accounts.len());

    if accounts.is_empty() {
        return Err("No Google accounts connected".to_string());
    }

    let provider = GoogleDriveProvider::default();

    for account in &accounts {
        log::info!("  Trying account: {}", account.email);
        let hub = match provider.create_hub(&account.email).await {
            Ok(h) => h,
            Err(e) => {
                log::warn!("    Failed to create hub: {}", e);
                continue;
            }
        };

        // Try to get the file
        match provider.get_file_by_id(&hub, file_id).await {
            Ok(file) => {
                log::info!("    Found file: {:?}", file.name);
                // Found it! Build the path
                let path = provider.build_file_path(&hub, &file, &account.email).await?;
                log::info!("    Built path: {}", path);
                return Ok((account.email.clone(), path));
            }
            Err(e) => {
                log::warn!("    Failed to get file: {}", e);
                continue;
            }
        }
    }

    Err("File not accessible with any connected account".to_string())
}

/// Resolve a Google Drive folder ID to a navigable path.
/// Returns (email, path, name) where path is either:
/// - "/My Drive/path/to/folder" for items in My Drive
/// - "/id/FOLDER_ID" for shared items not in My Drive hierarchy
pub async fn resolve_folder_id(
    accounts: &[String],
    folder_id: &str,
) -> Result<(String, String, String), String> {
    log::info!("resolve_folder_id: folder_id={}, accounts={:?}", folder_id, accounts);

    if accounts.is_empty() {
        return Err("No accounts provided".to_string());
    }

    for email in accounts {
        log::info!("  Trying account: {}", email);
        match try_resolve_folder_id(email, folder_id).await {
            Ok((path, name)) => {
                log::info!("  Success: email={}, path={}, name={}", email, path, name);
                return Ok((email.clone(), path, name));
            }
            Err(e) => {
                log::info!("  Account {} cannot access folder {}: {}", email, folder_id, e);
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
        "https://www.googleapis.com/drive/v3/files/{}?fields=id,name,parents,mimeType&supportsAllDrives=true",
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
    let file_id = metadata["id"].as_str().unwrap_or(folder_id);

    // Try to build full path by traversing parents
    if let Some(parents) = metadata["parents"].as_array() {
        if !parents.is_empty() {
            match build_path_from_parents(file_id, &access_token).await {
                Ok(path) => return Ok((path, name)),
                Err(e) => {
                    log::info!("  Can't build path, using id notation: {}", e);
                    // Can't build path, use id notation
                    return Ok((format!("/id/{}", folder_id), name));
                }
            }
        }
    }

    // No parents means it's a root or shared item
    Ok((format!("/id/{}", folder_id), name))
}

async fn build_path_from_parents(
    folder_id: &str,
    access_token: &str,
) -> Result<String, String> {
    let client = reqwest::Client::new();
    let mut path_parts: Vec<String> = Vec::new();
    let mut current_id = folder_id.to_string();

    // Traverse up to 20 levels (safety limit)
    for _ in 0..20 {
        let url = format!(
            "https://www.googleapis.com/drive/v3/files/{}?fields=id,name,parents&supportsAllDrives=true",
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

            if let Some(parent_id) = parents.first().and_then(|p| p.as_str()) {
                // Check if parent is "root" (My Drive root)
                if parent_id == "root" {
                    path_parts.reverse();
                    return Ok(format!("/{}/{}", VIRTUAL_MY_DRIVE, path_parts.join("/")));
                }
                current_id = parent_id.to_string();
            } else {
                return Err("Invalid parent ID".to_string());
            }
        } else {
            // No parents - this is root
            path_parts.reverse();
            return Ok(format!("/{}/{}", name, path_parts.join("/")));
        }
    }

    Err("Path too deep".to_string())
}

/// Fetch a URL with Google Drive authentication and return as data URL
/// This is used for thumbnail URLs that require authentication
pub async fn fetch_url_with_auth(email: &str, url: &str) -> Result<String, String> {
    use base64::Engine;

    log::info!("fetch_url_with_auth: email={}, url={}", email, url);

    let access_token = ensure_valid_token(email).await?;

    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .bearer_auth(&access_token)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch URL: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        return Err(format!("Fetch failed with status {}", status));
    }

    let content_type = response.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();

    let bytes = response.bytes()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;

    let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);

    log::info!("fetch_url_with_auth: got {} bytes, content_type={}", bytes.len(), content_type);

    Ok(format!("data:{};base64,{}", content_type, base64_data))
}

/// Download a Google Drive file to a temporary location and return the path
/// This is used for opening files that need to be downloaded first
pub async fn download_file_to_temp(email: &str, file_id: &str, file_name: &str) -> Result<String, String> {
    use std::io::Write;

    log::info!("download_file_to_temp: email={}, file_id={}, name={}", email, file_id, file_name);

    // Create temp directory if it doesn't exist
    let temp_dir = std::env::temp_dir().join("marlin-gdrive-cache");
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("Failed to create temp directory: {}", e))?;

    // Create temp file path with original filename
    let temp_path = temp_dir.join(format!("{}_{}", file_id, file_name));
    let temp_path_str = temp_path.to_string_lossy().to_string();

    // Get the access token
    let access_token = ensure_valid_token(email).await?;

    // Download using direct HTTPS request with the access token
    // The Google Drive API download endpoint: https://www.googleapis.com/drive/v3/files/{fileId}?alt=media
    let download_url = format!(
        "https://www.googleapis.com/drive/v3/files/{}?alt=media&supportsAllDrives=true",
        file_id
    );

    let client = reqwest::Client::new();
    let response = client
        .get(&download_url)
        .bearer_auth(&access_token)
        .send()
        .await
        .map_err(|e| format!("Failed to download file: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Download failed with status {}: {}", status, body));
    }

    let bytes = response.bytes()
        .await
        .map_err(|e| format!("Failed to read file content: {}", e))?;

    let mut file = std::fs::File::create(&temp_path)
        .map_err(|e| format!("Failed to create temp file: {}", e))?;

    file.write_all(&bytes)
        .map_err(|e| format!("Failed to write temp file: {}", e))?;

    log::info!("Downloaded {} bytes to {}", bytes.len(), temp_path_str);

    Ok(temp_path_str)
}

/// Upload a local file to Google Drive
/// Returns the file ID of the uploaded file
pub async fn upload_file_to_gdrive(
    email: &str,
    local_path: &std::path::Path,
    parent_folder_id: &str,
    file_name: &str,
) -> Result<String, String> {
    log::info!(
        "upload_file_to_gdrive: email={}, local_path={:?}, parent={}, name={}",
        email, local_path, parent_folder_id, file_name
    );

    let access_token = ensure_valid_token(email).await?;

    // Read the file content
    let file_content = std::fs::read(local_path)
        .map_err(|e| format!("Failed to read file: {}", e))?;

    // Determine MIME type based on extension
    let mime_type = mime_guess::from_path(local_path)
        .first_or_octet_stream()
        .to_string();

    // Create the file metadata
    let metadata = serde_json::json!({
        "name": file_name,
        "parents": [parent_folder_id]
    });

    // Use multipart upload
    let client = reqwest::Client::new();
    let boundary = "----WebKitFormBoundary7MA4YWxkTrZu0gW";

    let mut body = Vec::new();

    // Metadata part
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
    body.extend_from_slice(metadata.to_string().as_bytes());
    body.extend_from_slice(b"\r\n");

    // File content part
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(format!("Content-Type: {}\r\n\r\n", mime_type).as_bytes());
    body.extend_from_slice(&file_content);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{}--", boundary).as_bytes());

    let response = client
        .post("https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart&supportsAllDrives=true")
        .bearer_auth(&access_token)
        .header("Content-Type", format!("multipart/related; boundary={}", boundary))
        .body(body)
        .send()
        .await
        .map_err(|e| format!("Failed to upload file: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Upload failed with status {}: {}", status, body));
    }

    let result: serde_json::Value = response.json().await
        .map_err(|e| format!("Failed to parse upload response: {}", e))?;

    let file_id = result["id"].as_str()
        .ok_or_else(|| "No file ID in upload response".to_string())?
        .to_string();

    log::info!("Uploaded file, got ID: {}", file_id);
    Ok(file_id)
}

/// Create a folder in Google Drive
/// Returns the folder ID
pub async fn create_gdrive_folder(
    email: &str,
    parent_folder_id: &str,
    folder_name: &str,
) -> Result<String, String> {
    log::info!(
        "create_gdrive_folder: email={}, parent={}, name={}",
        email, parent_folder_id, folder_name
    );

    let access_token = ensure_valid_token(email).await?;

    let metadata = serde_json::json!({
        "name": folder_name,
        "mimeType": "application/vnd.google-apps.folder",
        "parents": [parent_folder_id]
    });

    let client = reqwest::Client::new();
    let response = client
        .post("https://www.googleapis.com/drive/v3/files?supportsAllDrives=true")
        .bearer_auth(&access_token)
        .header("Content-Type", "application/json")
        .body(metadata.to_string())
        .send()
        .await
        .map_err(|e| format!("Failed to create folder: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Create folder failed with status {}: {}", status, body));
    }

    let result: serde_json::Value = response.json().await
        .map_err(|e| format!("Failed to parse create folder response: {}", e))?;

    let folder_id = result["id"].as_str()
        .ok_or_else(|| "No folder ID in response".to_string())?
        .to_string();

    log::info!("Created folder, got ID: {}", folder_id);
    Ok(folder_id)
}

/// Recursively upload a directory to Google Drive
pub async fn upload_directory_to_gdrive(
    email: &str,
    local_dir: &std::path::Path,
    parent_folder_id: &str,
) -> Result<String, String> {
    let dir_name = local_dir.file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "Invalid directory name".to_string())?;

    log::info!("upload_directory_to_gdrive: dir={:?}, parent={}", local_dir, parent_folder_id);

    // Create the folder in Google Drive
    let folder_id = create_gdrive_folder(email, parent_folder_id, dir_name).await?;

    // Upload all contents
    let entries = std::fs::read_dir(local_dir)
        .map_err(|e| format!("Failed to read directory: {}", e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry.path();
        let name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        // Skip hidden files (starting with .)
        if name.starts_with('.') {
            continue;
        }

        if path.is_dir() {
            Box::pin(upload_directory_to_gdrive(email, &path, &folder_id)).await?;
        } else {
            upload_file_to_gdrive(email, &path, &folder_id, name).await?;
        }
    }

    Ok(folder_id)
}

/// Extract a zip file from Google Drive and upload the contents back to Google Drive
pub async fn extract_gdrive_zip(
    email: &str,
    file_id: &str,
    file_name: &str,
    destination_folder_id: &str,
) -> Result<String, String> {
    log::info!(
        "extract_gdrive_zip: email={}, file_id={}, name={}, dest={}",
        email, file_id, file_name, destination_folder_id
    );

    // Download the zip file to temp
    let temp_zip_path = download_file_to_temp(email, file_id, file_name).await?;
    let temp_zip = std::path::Path::new(&temp_zip_path);

    // Create a temp directory for extraction
    let extract_dir = std::env::temp_dir()
        .join("marlin-gdrive-extract")
        .join(format!("{}_{}", file_id, chrono::Utc::now().timestamp()));

    std::fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("Failed to create extraction directory: {}", e))?;

    log::info!("Extracting to temp dir: {:?}", extract_dir);

    // Extract the zip file
    let zip_file = std::fs::File::open(temp_zip)
        .map_err(|e| format!("Failed to open zip file: {}", e))?;

    let mut archive = zip::ZipArchive::new(zip_file)
        .map_err(|e| format!("Failed to read zip archive: {}", e))?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)
            .map_err(|e| format!("Failed to read zip entry: {}", e))?;

        let outpath = match file.enclosed_name() {
            Some(path) => extract_dir.join(path),
            None => continue,
        };

        if file.name().ends_with('/') {
            std::fs::create_dir_all(&outpath)
                .map_err(|e| format!("Failed to create directory: {}", e))?;
        } else {
            if let Some(p) = outpath.parent() {
                if !p.exists() {
                    std::fs::create_dir_all(p)
                        .map_err(|e| format!("Failed to create parent directory: {}", e))?;
                }
            }
            let mut outfile = std::fs::File::create(&outpath)
                .map_err(|e| format!("Failed to create file: {}", e))?;
            std::io::copy(&mut file, &mut outfile)
                .map_err(|e| format!("Failed to write file: {}", e))?;
        }
    }

    // Determine folder name for upload (zip name without extension)
    let folder_name = file_name.trim_end_matches(".zip")
        .trim_end_matches(".ZIP");

    // Create the destination folder in Google Drive
    let dest_folder_id = create_gdrive_folder(email, destination_folder_id, folder_name).await?;

    // Upload all extracted contents to Google Drive
    let entries = std::fs::read_dir(&extract_dir)
        .map_err(|e| format!("Failed to read extraction directory: {}", e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry.path();
        let name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        // Skip hidden files
        if name.starts_with('.') {
            continue;
        }

        if path.is_dir() {
            upload_directory_to_gdrive(email, &path, &dest_folder_id).await?;
        } else {
            upload_file_to_gdrive(email, &path, &dest_folder_id, name).await?;
        }
    }

    // Clean up temp files
    let _ = std::fs::remove_file(temp_zip);
    let _ = std::fs::remove_dir_all(&extract_dir);

    log::info!("Extraction and upload complete, folder ID: {}", dest_folder_id);
    Ok(dest_folder_id)
}

/// Look up the folder ID for a given Google Drive path
/// Path format: /My Drive/folder1/folder2 or /Shared with me/folder1
pub async fn get_folder_id_by_path(email: &str, path: &str) -> Result<String, String> {
    log::info!("get_folder_id_by_path: email={}, path={}", email, path);

    let provider = GoogleDriveProvider::default();
    let hub = provider.create_hub(email).await?;

    let (root_folder, subpath) = provider.parse_virtual_path(path);

    match root_folder {
        Some(VIRTUAL_MY_DRIVE) => {
            if subpath.is_empty() {
                Ok("root".to_string())
            } else {
                provider.find_file_by_path(&hub, &subpath).await?
                    .ok_or_else(|| format!("Folder not found: {}", path))
            }
        }
        Some(VIRTUAL_SHARED) => {
            if subpath.is_empty() {
                // Shared root doesn't have a single folder ID
                Err("Cannot get folder ID for Shared with me root".to_string())
            } else {
                provider.find_shared_file_by_path(&hub, &subpath).await?
                    .ok_or_else(|| format!("Folder not found: {}", path))
            }
        }
        Some(VIRTUAL_BY_ID) => {
            // Direct ID navigation - the ID is in the path
            if subpath.is_empty() {
                Err("Missing folder ID in path".to_string())
            } else {
                Ok(subpath[0].to_string())
            }
        }
        _ => Err(format!("Unsupported path type: {}", path)),
    }
}

// Suppress warnings for unused cache variables (will be used for optimization)
#[allow(dead_code)]
fn _use_caches() {
    let _ = &*DIR_CACHE;
    let _ = &*PATH_CACHE;
    let _ = CACHE_TTL;
}
