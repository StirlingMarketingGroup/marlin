use base64::Engine as _;
use chrono::{DateTime, Utc};
use dirs;
use git2::{Branch, BranchType, ErrorCode as GitErrorCode, Oid, Repository, Status, StatusOptions};
use log::{info, warn};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::path::PathBuf;
use std::process::Command as OsCommand;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{command, AppHandle, Emitter, Manager};
use tokio::process::Command as TokioCommand;
use tokio::sync::OnceCell;
use tokio::time::sleep;
use url::Url;
use urlencoding::encode;
use uuid::Uuid;
use walkdir::WalkDir;

#[cfg(target_family = "unix")]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use crate::fs_utils::{
    self, delete_file_or_directory, expand_path, read_directory_streaming,
    resolve_symlink_parent, DiskUsage, FileItem, SymlinkResolution,
};
use crate::fs_watcher;
use crate::locations::{resolve_location, LocationCapabilities, LocationInput, LocationSummary};
use crate::locations::gdrive::{
    add_google_account as add_gdrive_account, get_google_accounts as get_gdrive_accounts,
    remove_google_account as remove_gdrive_account, GoogleAccountInfo,
};
use crate::locations::gdrive::provider::{resolve_file_id_to_path, resolve_folder_id, download_file_to_temp, fetch_url_with_auth, extract_gdrive_zip, get_folder_id_by_path};
use crate::locations::gdrive::url_parser::{is_google_drive_url, parse_google_drive_url};
#[cfg(target_os = "macos")]
use crate::macos_security;
#[cfg(target_os = "macos")]
use crate::state::MacTrashUndoItem;
use crate::state::{
    DirectoryStreamHandle, DirectoryStreamState, FolderSizeState, FolderSizeTaskHandle,
    TrashUndoRecord, TrashUndoState,
};
use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use tar::Archive as TarArchive;
use xz2::read::XzDecoder;
use zip::ZipArchive;
use zstd::stream::read::Decoder as ZstdDecoder;

#[cfg(any(target_os = "windows", target_os = "linux"))]
use trash::os_limited;

#[cfg(target_os = "macos")]
use cocoa::base::{id, nil, BOOL, NO};
#[cfg(target_os = "macos")]
use cocoa::foundation::{NSAutoreleasePool, NSString};
#[cfg(target_os = "macos")]
use objc::rc::StrongPtr;
#[cfg(target_os = "macos")]
use objc::{class, msg_send, sel, sel_impl};
#[cfg(target_os = "macos")]
use std::ffi::CStr;
#[cfg(target_os = "macos")]
use std::os::raw::c_char;

const FOLDER_SIZE_EVENT: &str = "folder-size-progress";
const FOLDER_SIZE_INIT_EVENT: &str = "folder-size:init";
const FOLDER_SIZE_WINDOW_LABEL: &str = "folder-size";
const ARCHIVE_PROGRESS_EVENT: &str = "archive-progress:init";
const ARCHIVE_PROGRESS_WINDOW_LABEL: &str = "archive-progress";

// Error codes for structured error handling
// These constants define the API contract with the frontend
#[allow(dead_code)]
pub mod error_codes {
    pub const ENOENT: &str = "ENOENT"; // Path does not exist
    pub const ENOTDIR: &str = "ENOTDIR"; // Path is not a directory
    pub const EPERM: &str = "EPERM"; // Permission denied / Operation not permitted
}

/// Format an error with a code prefix for structured error handling
/// Format: "[CODE] Human readable message"
#[allow(dead_code)] // Will be used by future location providers
fn format_error(code: &str, message: &str) -> String {
    format!("[{code}] {message}")
}
const ARCHIVE_PROGRESS_UPDATE_EVENT: &str = "archive-progress:update";
const DELETE_PROGRESS_EVENT: &str = "delete-progress:init";
const DELETE_PROGRESS_UPDATE_EVENT: &str = "delete-progress:update";
const DELETE_PROGRESS_WINDOW_LABEL: &str = "delete-progress";
const SMB_CONNECT_INIT_EVENT: &str = "smb-connect:init";
const SMB_CONNECT_WINDOW_LABEL: &str = "smb-connect";
const PERMISSIONS_WINDOW_LABEL: &str = "permissions";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryListingResponse {
    pub location: LocationSummary,
    pub capabilities: LocationCapabilities,
    pub entries: Vec<FileItem>,
}

/// Generic queue for window payloads that handles the ready/pending pattern.
/// Replaces the duplicated queue_*_payload and try_emit_pending_*_payload functions.
struct WindowPayloadQueue<T: Clone + Serialize> {
    ready: AtomicBool,
    pending: Mutex<Option<T>>,
    window_label: &'static str,
    event_name: &'static str,
}

impl<T: Clone + Serialize> WindowPayloadQueue<T> {
    const fn new(window_label: &'static str, event_name: &'static str) -> Self {
        Self {
            ready: AtomicBool::new(false),
            pending: Mutex::new(None),
            window_label,
            event_name,
        }
    }

    fn queue(&self, app: &AppHandle, payload: T) {
        {
            let mut pending = self.pending.lock().expect("Failed to lock pending payload");
            *pending = Some(payload);
        }
        self.try_emit(app);
    }

    fn try_emit(&self, app: &AppHandle) {
        if !self.ready.load(Ordering::SeqCst) {
            return;
        }

        let payload_opt = {
            let mut pending = self.pending.lock().expect("Failed to lock pending payload");
            pending.take()
        };

        if let Some(payload) = payload_opt {
            let mut should_requeue = true;
            if let Some(window) = app.get_webview_window(self.window_label) {
                if let Err(err) = window.emit(self.event_name, &payload) {
                    warn!("Failed to emit {} payload: {err}", self.event_name);
                } else {
                    should_requeue = false;
                }
            } else {
                warn!(
                    "Window '{}' not available for payload emission",
                    self.window_label
                );
            }

            if should_requeue {
                let mut pending = self.pending.lock().expect("Failed to relock pending payload");
                *pending = Some(payload);
            }
        }
    }

    fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::SeqCst);
    }

    fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    fn clear_pending(&self) {
        let mut pending = self.pending.lock().expect("Failed to lock pending payload");
        *pending = None;
    }
}

static FOLDER_SIZE_QUEUE: Lazy<WindowPayloadQueue<FolderSizeInitPayload>> =
    Lazy::new(|| WindowPayloadQueue::new(FOLDER_SIZE_WINDOW_LABEL, FOLDER_SIZE_INIT_EVENT));
static ARCHIVE_PROGRESS_QUEUE: Lazy<WindowPayloadQueue<ArchiveProgressPayload>> =
    Lazy::new(|| WindowPayloadQueue::new(ARCHIVE_PROGRESS_WINDOW_LABEL, ARCHIVE_PROGRESS_EVENT));
static DELETE_PROGRESS_QUEUE: Lazy<WindowPayloadQueue<DeleteProgressPayload>> =
    Lazy::new(|| WindowPayloadQueue::new(DELETE_PROGRESS_WINDOW_LABEL, DELETE_PROGRESS_EVENT));
static SMB_CONNECT_QUEUE: Lazy<WindowPayloadQueue<SmbConnectInitPayload>> =
    Lazy::new(|| WindowPayloadQueue::new(SMB_CONNECT_WINDOW_LABEL, SMB_CONNECT_INIT_EVENT));

const FOLDER_SIZE_WINDOW_READY_POLL_INTERVAL: Duration = Duration::from_millis(25);
const FOLDER_SIZE_WINDOW_READY_POLL_ATTEMPTS: u32 = 40;
const FOLDER_SIZE_WINDOW_READY_STABILIZE_DELAY: Duration = Duration::from_millis(25);

fn emit_delete_progress_update(app: &AppHandle, payload: DeleteProgressUpdatePayload) {
    if let Err(err) = app.emit(DELETE_PROGRESS_UPDATE_EVENT, payload) {
        warn!("Failed to emit delete progress update: {err}");
    }
}

#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
fn normalize_path_for_compare(path: &Path) -> String {
    let mut value = path.to_string_lossy().replace('\\', "/");
    while value.ends_with('/') && value.len() > 1 {
        value.pop();
    }
    #[cfg(target_os = "windows")]
    {
        value = value.to_lowercase();
    }
    value
}

fn cleanup_trash_undo_records(state: &TrashUndoState) {
    const TTL: Duration = Duration::from_secs(300);
    const MAX_RECORDS: usize = 10;

    let now = Instant::now();
    if let Ok(mut records) = state.records.lock() {
        records.retain(|_, record| now.saturating_duration_since(record.created_at) <= TTL);

        if records.len() > MAX_RECORDS {
            let mut entries: Vec<(String, Instant)> = records
                .iter()
                .map(|(id, record)| (id.clone(), record.created_at))
                .collect();
            entries.sort_by_key(|(_, created)| *created);
            let excess = records.len() - MAX_RECORDS;
            for (id, _) in entries.into_iter().take(excess) {
                records.remove(&id);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn nsstring_to_string(ns_string: id) -> Option<String> {
    if ns_string == nil {
        return None;
    }

    unsafe {
        let raw: *const c_char = msg_send![ns_string, UTF8String];
        if raw.is_null() {
            None
        } else {
            Some(CStr::from_ptr(raw).to_string_lossy().into_owned())
        }
    }
}

#[cfg(target_os = "macos")]
fn nserror_to_string(error: id) -> Option<String> {
    if error == nil {
        return None;
    }

    unsafe {
        let description: id = msg_send![error, localizedDescription];
        nsstring_to_string(description)
    }
}

#[cfg(target_os = "macos")]
fn nsurl_path(url: id) -> Option<String> {
    if url == nil {
        return None;
    }

    unsafe {
        let path: id = msg_send![url, path];
        nsstring_to_string(path)
    }
}

#[cfg(target_os = "macos")]
fn macos_trash_items(paths: &[PathBuf]) -> Result<Vec<MacTrashUndoItem>, String> {
    use std::path::Path;

    unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        let file_manager: id = msg_send![class!(NSFileManager), defaultManager];

        if file_manager == nil {
            return Err("Failed to acquire NSFileManager".to_string());
        }

        let mut records = Vec::with_capacity(paths.len());

        for path in paths {
            let original_path_str = path.to_string_lossy().to_string();

            let _scope_guard = macos_security::retain_access(path)?;

            let ns_path = NSString::alloc(nil).init_str(&original_path_str);
            if ns_path == nil {
                return Err(format!(
                    "Failed to create NSString for path: {}",
                    original_path_str
                ));
            }
            let ns_path = StrongPtr::new(ns_path);

            let url: id = msg_send![class!(NSURL), fileURLWithPath:*ns_path];
            if url == nil {
                return Err(format!(
                    "Failed to create NSURL for path: {}",
                    original_path_str
                ));
            }
            let url = StrongPtr::retain(url);

            let mut resulting_url: id = nil;
            let mut error: id = nil;

            let success: BOOL = msg_send![file_manager,
                trashItemAtURL:*url
                resultingItemURL:&mut resulting_url
                error:&mut error
            ];

            if success == NO {
                let message = nserror_to_string(error)
                    .unwrap_or_else(|| "Operation not permitted".to_string());
                return Err(format!(
                    "Failed to move {} to Trash: {}",
                    original_path_str, message
                ));
            }

            let trashed_path = nsurl_path(resulting_url).ok_or_else(|| {
                String::from("Failed to resolve trash destination path")
            })?;

            let trashed_path_buf = Path::new(&trashed_path).to_path_buf();
            macos_security::persist_bookmark(&trashed_path_buf, "trashing item");

            records.push(MacTrashUndoItem {
                trashed_path,
                original_path: original_path_str,
            });
        }

        Ok(records)
    }
}

#[cfg(target_os = "macos")]
fn macos_restore_items(items: &[MacTrashUndoItem]) -> Result<(), String> {
    use std::path::Path;

    unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        let file_manager: id = msg_send![class!(NSFileManager), defaultManager];

        if file_manager == nil {
            return Err("Failed to acquire NSFileManager".to_string());
        }

        for item in items {
            let from_path = Path::new(&item.trashed_path);
            let to_path = Path::new(&item.original_path);

            let parent = to_path.parent().ok_or_else(|| {
                format!(
                    "Cannot restore {} because original parent directory could not be determined",
                    item.original_path
                )
            })?;

            if !parent.exists() {
                return Err(format!(
                    "Cannot restore {}; destination folder no longer exists",
                    item.original_path
                ));
            }

            if to_path.exists() {
                return Err(format!(
                    "Cannot restore {}; an item already exists at that location",
                    item.original_path
                ));
            }

            let _from_scope = macos_security::retain_access(from_path)?;
            let _to_scope = macos_security::retain_access(parent)?;

            let from_ns = NSString::alloc(nil).init_str(&item.trashed_path);
            if from_ns == nil {
                return Err(format!(
                    "Failed to create NSString for trashed path: {}",
                    item.trashed_path
                ));
            }
            let from_ns = StrongPtr::new(from_ns);
            let from_url: id = msg_send![class!(NSURL), fileURLWithPath:*from_ns];

            let to_ns = NSString::alloc(nil).init_str(&item.original_path);
            if to_ns == nil {
                return Err(format!(
                    "Failed to create NSString for original path: {}",
                    item.original_path
                ));
            }
            let to_ns = StrongPtr::new(to_ns);
            let to_url: id = msg_send![class!(NSURL), fileURLWithPath:*to_ns];

            let mut error: id = nil;
            let success: BOOL = msg_send![file_manager,
                moveItemAtURL:from_url
                toURL:to_url
                error:&mut error
            ];

            if success == NO {
                let message = nserror_to_string(error)
                    .unwrap_or_else(|| "Operation not permitted".to_string());
                return Err(format!(
                    "Failed to restore {} from Trash: {}",
                    item.original_path, message
                ));
            }

            macos_security::persist_bookmark(to_path, "restoring item from trash");
        }

        Ok(())
    }
}

fn schedule_folder_size_auto_start(app: &AppHandle, request_id: String, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        // Wait for the window to report readiness so we don't emit progress events
        // before its listeners are attached.
        for _ in 0..FOLDER_SIZE_WINDOW_READY_POLL_ATTEMPTS {
            if FOLDER_SIZE_QUEUE.is_ready() {
                break;
            }
            sleep(FOLDER_SIZE_WINDOW_READY_POLL_INTERVAL).await;
        }

        // Allow a small buffer for the renderer to process the init payload.
        sleep(FOLDER_SIZE_WINDOW_READY_STABILIZE_DELAY).await;

        let state = app_handle.state::<FolderSizeState>();
        if let Err(err) =
            calculate_folder_size(app_handle.clone(), state, request_id.clone(), paths).await
        {
            warn!(
                "Auto-start folder size calculation failed for {}: {}",
                request_id, err
            );
            emit_folder_size_event(
                &app_handle,
                &request_id,
                0,
                0,
                0,
                None,
                true,
                false,
                Some(format!("Failed to start folder size calculation: {}", err)),
            );
        }
    });
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderSizeTargetPayload {
    pub path: String,
    pub name: String,
    #[serde(default)]
    pub is_directory: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FolderSizeInitPayload {
    request_id: String,
    targets: Vec<FolderSizeTargetPayload>,
    auto_start: bool,
    initial_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveProgressPayload {
    file_name: String,
    destination_dir: String,
    format: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveProgressUpdatePayload {
    archive_name: String,
    entry_name: Option<String>,
    format: String,
    finished: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SmbConnectInitPayload {
    initial_hostname: Option<String>,
    target_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteItemPayload {
    path: String,
    name: String,
    #[serde(default)]
    is_directory: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteProgressPayload {
    request_id: String,
    total_items: usize,
    items: Vec<DeleteItemPayload>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteProgressUpdatePayload {
    request_id: String,
    current_path: Option<String>,
    completed: usize,
    total: usize,
    finished: bool,
    error: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FolderSizeProgressPayload {
    request_id: String,
    total_bytes: u64,
    total_apparent_bytes: u64,
    total_items: u64,
    current_path: Option<String>,
    finished: bool,
    cancelled: bool,
    error: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DiskUsageResponse {
    pub path: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ExtractArchiveResponse {
    pub folder_path: String,
    pub used_system_fallback: bool,
    pub format: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TrashPathsResponse {
    trashed: Vec<String>,
    undo_token: Option<String>,
    fallback_to_permanent: bool,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UndoTrashResponse {
    restored: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DeletePathsResponse {
    deleted: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusResponse {
    pub repository_root: String,
    pub branch: Option<String>,
    pub detached: bool,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: bool,
    pub has_untracked: bool,
    pub remote_url: Option<String>,
    pub remote_branch_url: Option<String>,
}

#[derive(Debug, Default)]
struct BranchState {
    name: Option<String>,
    detached: bool,
    head_oid: Option<Oid>,
    ahead: u32,
    behind: u32,
    remote_url: Option<String>,
}

fn normalize_remote_url(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }

    let trimmed = raw.trim();

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Some(trimmed.trim_end_matches(".git").to_string());
    }

    if trimmed.starts_with("git://") {
        let without_scheme = trimmed.trim_start_matches("git://");
        if let Some((host, path)) = without_scheme.split_once('/') {
            let clean_path = path.trim_end_matches(".git");
            return Some(format!("https://{host}/{clean_path}"));
        }
    }

    if trimmed.starts_with("ssh://") {
        let without_scheme = trimmed.trim_start_matches("ssh://");
        let host_and_path = if let Some(idx) = without_scheme.find('@') {
            &without_scheme[idx + 1..]
        } else {
            without_scheme
        };
        if let Some((host, path)) = host_and_path.split_once('/') {
            let clean_path = path.trim_end_matches(".git");
            return Some(format!("https://{host}/{clean_path}"));
        }
    }

    if let Some(rest) = trimmed.strip_prefix("git@") {
        if let Some((host, path)) = rest.split_once(':') {
            let clean_path = path.trim_end_matches(".git");
            return Some(format!("https://{host}/{clean_path}"));
        }
    }

    None
}

fn upstream_remote_url(repo: &Repository, upstream_branch: &Branch) -> Option<String> {
    let upstream_name = upstream_branch.name().ok().flatten()?;
    let trimmed = upstream_name.strip_prefix("refs/remotes/")?;
    let (remote_name, _) = trimmed.split_once('/')?;
    let remote = repo.find_remote(remote_name).ok()?;
    let url = remote.url()?;
    normalize_remote_url(url)
}

fn fallback_remote_url(repo: &Repository) -> Option<String> {
    if let Ok(remote) = repo.find_remote("origin") {
        if let Some(url) = remote.url() {
            if let Some(normalized) = normalize_remote_url(url) {
                return Some(normalized);
            }
        }
    }

    if let Ok(remotes) = repo.remotes() {
        for name in remotes.iter().flatten() {
            if let Ok(remote) = repo.find_remote(name) {
                if let Some(url) = remote.url() {
                    if let Some(normalized) = normalize_remote_url(url) {
                        return Some(normalized);
                    }
                }
            }
        }
    }

    None
}

fn resolve_branch_state(repo: &Repository) -> BranchState {
    let mut state = BranchState::default();

    let head = match repo.head() {
        Ok(head) => head,
        Err(_) => {
            state.detached = true;
            return state;
        }
    };

    state.name = head.shorthand().map(|s| s.to_string());
    state.detached = !head.is_branch();
    state.head_oid = head
        .target()
        .or_else(|| head.peel_to_commit().ok().map(|commit| commit.id()));

    if head.is_branch() {
        if let Some(short_name) = head.shorthand() {
            if let Ok(local_branch) = repo.find_branch(short_name, BranchType::Local) {
                if let Ok(upstream_branch) = local_branch.upstream() {
                    if let (Some(local_oid), Some(upstream_oid)) =
                        (local_branch.get().target(), upstream_branch.get().target())
                    {
                        if let Ok((ahead, behind)) =
                            repo.graph_ahead_behind(local_oid, upstream_oid)
                        {
                            state.ahead = ahead as u32;
                            state.behind = behind as u32;
                        }
                    }

                    state.remote_url = upstream_remote_url(repo, &upstream_branch);
                }
            }
        }
    }

    if state.remote_url.is_none() {
        state.remote_url = fallback_remote_url(repo);
    }

    state
}

fn build_remote_branch_url(
    remote: &str,
    branch: Option<&str>,
    detached: bool,
    head_oid: Option<Oid>,
) -> Option<String> {
    let parsed = Url::parse(remote).ok()?;
    let host = parsed.host_str()?.to_lowercase();
    let base = remote.trim_end_matches('/');

    if detached {
        let sha = head_oid?.to_string();
        if host.contains("github") {
            return Some(format!("{}/commit/{}", base, sha));
        }
        if host.contains("gitlab") {
            return Some(format!("{}/-/commit/{}", base, sha));
        }
        if host.contains("bitbucket") {
            return Some(format!("{}/commits/{}", base, sha));
        }
        return Some(format!("{}/commit/{}", base, sha));
    }

    let branch = branch?;
    let encoded = encode(branch).into_owned();

    if host.contains("github") {
        return Some(format!("{}/tree/{}", base, encoded));
    }
    if host.contains("gitlab") {
        return Some(format!("{}/-/tree/{}", base, encoded));
    }
    if host.contains("bitbucket") {
        return Some(format!("{}/branch/{}", base, encoded));
    }

    None
}

fn compute_git_status(path: &Path) -> Result<Option<GitStatusResponse>, String> {
    let repo = match Repository::discover(path) {
        Ok(repo) => repo,
        Err(err) => {
            if err.code() == GitErrorCode::NotFound {
                return Ok(None);
            }
            return Err(format!("Failed to open Git repository: {err}"));
        }
    };

    let workdir = match repo.workdir() {
        Some(path) => path,
        None => {
            // Bare repositories have no working directory; surface as unsupported for now.
            return Ok(None);
        }
    };

    let repository_root = workdir
        .canonicalize()
        .unwrap_or_else(|_| workdir.to_path_buf())
        .to_string_lossy()
        .to_string();

    let BranchState {
        name: branch_name,
        detached,
        head_oid,
        ahead,
        behind,
        remote_url,
    } = resolve_branch_state(&repo);

    let mut status_options = StatusOptions::new();
    status_options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);

    let statuses = repo
        .statuses(Some(&mut status_options))
        .map_err(|err| format!("Failed to compute repository status: {err}"))?;

    let mut dirty = false;
    let mut has_untracked = false;

    for entry in statuses.iter() {
        let status = entry.status();
        if !status.is_empty() {
            dirty = true;
        }
        if status.contains(Status::WT_NEW) {
            has_untracked = true;
        }
        if dirty && has_untracked {
            break;
        }
    }

    let remote_branch_url = remote_url.as_ref().and_then(|remote| {
        build_remote_branch_url(remote, branch_name.as_deref(), detached, head_oid)
    });

    Ok(Some(GitStatusResponse {
        repository_root,
        branch: branch_name,
        detached,
        ahead,
        behind,
        dirty,
        has_untracked,
        remote_url,
        remote_branch_url,
    }))
}

fn emit_folder_size_event(
    app: &AppHandle,
    request_id: &str,
    total_bytes: u64,
    total_apparent_bytes: u64,
    total_items: u64,
    current_path: Option<String>,
    finished: bool,
    cancelled: bool,
    error: Option<String>,
) {
    let payload = FolderSizeProgressPayload {
        request_id: request_id.to_string(),
        total_bytes,
        total_apparent_bytes,
        total_items,
        current_path,
        finished,
        cancelled,
        error,
    };
    if let Err(err) = app.emit(FOLDER_SIZE_EVENT, payload) {
        warn!("Failed to emit folder size progress event: {err}");
    }
}

const FOLDER_SIZE_EMIT_INTERVAL: Duration = Duration::from_millis(150);
const FOLDER_SIZE_EMIT_STEP: u64 = 256;

struct ProgressReporter<'a> {
    app: &'a AppHandle,
    request_id: &'a str,
    last_emit: Instant,
    items_since_emit: u64,
    total_bytes: u64,
    total_apparent_bytes: u64,
    total_items: u64,
}

impl<'a> ProgressReporter<'a> {
    fn new(app: &'a AppHandle, request_id: &'a str) -> Self {
        let mut reporter = Self {
            app,
            request_id,
            last_emit: Instant::now(),
            items_since_emit: 0,
            total_bytes: 0,
            total_apparent_bytes: 0,
            total_items: 0,
        };
        reporter.emit_internal(None, false, false, None);
        reporter
    }

    fn add_file(
        &mut self,
        metadata: &fs::Metadata,
        seen_inodes: &mut HashSet<(u64, u64)>,
        current_path: Option<&Path>,
    ) {
        self.total_apparent_bytes = self.total_apparent_bytes.saturating_add(metadata.len());
        let should_add_physical = match file_identity(metadata) {
            Some(identity) => seen_inodes.insert(identity),
            None => true,
        };
        if should_add_physical {
            self.total_bytes = self
                .total_bytes
                .saturating_add(physical_file_size(metadata));
        }
        self.record_item(current_path);
    }

    fn add_entry(&mut self, current_path: Option<&Path>) {
        self.record_item(current_path);
    }

    fn flush(&mut self, current_path: Option<&Path>) {
        if self.items_since_emit > 0 {
            self.emit_internal(current_path, false, false, None);
        }
    }

    fn emit_error(&mut self, current_path: Option<&Path>, error: impl Into<String>) {
        self.emit_internal(current_path, false, false, Some(error.into()));
    }

    fn finish(&mut self, cancelled: bool) {
        self.emit_internal(None, true, cancelled, None);
    }

    fn totals(&self) -> (u64, u64, u64) {
        (
            self.total_bytes,
            self.total_apparent_bytes,
            self.total_items,
        )
    }

    fn record_item(&mut self, current_path: Option<&Path>) {
        self.total_items = self.total_items.saturating_add(1);
        self.items_since_emit = self.items_since_emit.saturating_add(1);
        if self.items_since_emit >= FOLDER_SIZE_EMIT_STEP
            || self.last_emit.elapsed() >= FOLDER_SIZE_EMIT_INTERVAL
        {
            self.emit_internal(current_path, false, false, None);
        }
    }

    fn emit_internal(
        &mut self,
        current_path: Option<&Path>,
        finished: bool,
        cancelled: bool,
        error: Option<String>,
    ) {
        emit_folder_size_event(
            self.app,
            self.request_id,
            self.total_bytes,
            self.total_apparent_bytes,
            self.total_items,
            current_path.map(|p| p.to_string_lossy().to_string()),
            finished,
            cancelled,
            error,
        );
        self.last_emit = Instant::now();
        self.items_since_emit = 0;
    }
}

#[cfg(target_os = "macos")]
fn persist_bookmark_for_scan(path: &Path) {
    macos_security::persist_bookmark(path, "calculating folder size");
}

#[cfg(not(target_os = "macos"))]
fn persist_bookmark_for_scan(_path: &Path) {}

#[cfg(target_family = "unix")]
fn physical_file_size(metadata: &fs::Metadata) -> u64 {
    metadata.blocks().saturating_mul(512)
}

#[cfg(not(target_family = "unix"))]
fn physical_file_size(metadata: &fs::Metadata) -> u64 {
    metadata.len()
}

#[cfg(target_family = "unix")]
fn file_identity(metadata: &fs::Metadata) -> Option<(u64, u64)> {
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(target_family = "unix"))]
fn file_identity(_metadata: &fs::Metadata) -> Option<(u64, u64)> {
    None
}

#[cfg(target_os = "macos")]
fn should_skip_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy().to_lowercase();

    // Skip obvious network volumes mounted under /Volumes/
    if path.starts_with("/Volumes/") {
        let volume_name = path.strip_prefix("/Volumes/").unwrap_or(path);
        let volume_str = volume_name.to_string_lossy().to_lowercase();

        if volume_str.starts_with("smb")
            || volume_str.starts_with("afp")
            || volume_str.starts_with("nfs")
            || volume_str.contains("server")
            || volume_str.contains("share")
        {
            info!("Skipping network volume: {:?}", path);
            return true;
        }
    }

    // Skip system volumes that are not the main data volume
    if path_str.starts_with("/system/volumes/") && !path_str.starts_with("/system/volumes/data") {
        info!("Skipping system volume: {:?}", path);
        return true;
    }

    false
}

#[cfg(not(target_os = "macos"))]
fn should_skip_path(_path: &Path) -> bool {
    false
}

fn walk_paths_for_size(
    app: &AppHandle,
    request_id: &str,
    roots: &[PathBuf],
    cancel_flag: &Arc<AtomicBool>,
) -> bool {
    let mut reporter = ProgressReporter::new(app, request_id);
    let mut seen_inodes: HashSet<(u64, u64)> = HashSet::new();

    info!(
        "walk_paths_for_size started with {} roots for request {}",
        roots.len(),
        request_id
    );

    for root in roots {
        info!("Processing root path: {:?}", root);

        if should_skip_path(root) {
            continue;
        }

        if cancel_flag.load(Ordering::Relaxed) {
            reporter.finish(true);
            return true;
        }

        #[cfg(target_os = "macos")]
        let _scope_guard = match macos_security::retain_access(root) {
            Ok(guard) => guard,
            Err(initial_err) => {
                warn!(
                    "Failed to reuse security scope for {}: {}. Attempting to refresh bookmark...",
                    root.display(),
                    initial_err
                );

                if let Err(store_err) = macos_security::store_bookmark_if_needed(root) {
                    warn!(
                        "Unable to refresh bookmark for {}: {}",
                        root.display(),
                        store_err
                    );
                    reporter.emit_error(Some(root.as_path()), initial_err);
                    continue;
                }

                match macos_security::retain_access(root) {
                    Ok(guard) => guard,
                    Err(err) => {
                        reporter.emit_error(Some(root.as_path()), err);
                        continue;
                    }
                }
            }
        };

        let symlink_meta = match fs::symlink_metadata(root) {
            Ok(meta) => meta,
            Err(err) => {
                reporter.emit_error(
                    Some(root.as_path()),
                    format!("Failed to access entry: {err}"),
                );
                continue;
            }
        };

        let is_symlink = symlink_meta.file_type().is_symlink();
        let mut target_metadata: Option<fs::Metadata> = None;

        if is_symlink {
            match fs::metadata(root) {
                Ok(meta) => {
                    target_metadata = Some(meta);
                    if let Ok(resolved) = fs::canonicalize(root) {
                        if should_skip_path(&resolved) {
                            info!("Skipping symlink target path: {:?}", resolved);
                            continue;
                        }
                    }
                }
                Err(err) => {
                    warn!("Failed to resolve symlink target for {:?}: {}", root, err);
                    reporter.add_entry(Some(root.as_path()));
                    persist_bookmark_for_scan(root);
                    continue;
                }
            }
        }

        let metadata = target_metadata.as_ref().unwrap_or(&symlink_meta);

        if metadata.is_file() {
            reporter.add_file(metadata, &mut seen_inodes, Some(root.as_path()));
            persist_bookmark_for_scan(root);
            continue;
        }

        if metadata.is_dir() {
            info!("Starting directory walk for {:?}", root);
            let walker = WalkDir::new(root).follow_links(false).into_iter();
            for entry in walker {
                if cancel_flag.load(Ordering::Relaxed) {
                    reporter.finish(true);
                    return true;
                }

                let entry = match entry {
                    Ok(value) => value,
                    Err(err) => {
                        warn!("Failed to traverse directory {:?}: {err}", root);
                        continue;
                    }
                };

                let entry_path = entry.path();
                let file_type = entry.file_type();
                let metadata = match entry.metadata() {
                    Ok(meta) => meta,
                    Err(err) => {
                        warn!("Failed to read metadata for {:?}: {err}", entry_path);
                        continue;
                    }
                };

                if file_type.is_symlink() {
                    reporter.add_entry(Some(entry_path));
                } else if metadata.is_file() {
                    reporter.add_file(&metadata, &mut seen_inodes, Some(entry_path));
                } else {
                    reporter.add_entry(Some(entry_path));
                }
            }

            reporter.flush(Some(root.as_path()));
            persist_bookmark_for_scan(root);
            continue;
        }

        if is_symlink {
            reporter.add_entry(Some(root.as_path()));
            persist_bookmark_for_scan(root);
            continue;
        }

        persist_bookmark_for_scan(root);
    }

    let (total_bytes, total_apparent_bytes, total_items) = reporter.totals();
    info!(
        "Folder size calculation completed. Physical bytes: {}, Logical bytes: {}, Total items: {}",
        total_bytes, total_apparent_bytes, total_items
    );
    reporter.finish(false);
    false
}

#[command]
pub fn get_home_directory() -> Result<String, String> {
    dirs::home_dir()
        .map(|path| path.to_string_lossy().to_string())
        .ok_or_else(|| "Could not determine home directory".to_string())
}

#[command]
pub fn get_disk_usage(path: String) -> Result<DiskUsageResponse, String> {
    let expanded_path = expand_path(&path)?;
    let path = PathBuf::from(expanded_path);

    if !path.exists() {
        return Err("Path does not exist".to_string());
    }

    let usage: DiskUsage = fs_utils::get_disk_usage(&path)?;
    Ok(DiskUsageResponse {
        path: usage.path.to_string_lossy().to_string(),
        total_bytes: usage.total_bytes,
        available_bytes: usage.available_bytes,
    })
}

#[command]
pub async fn get_git_status(path: String) -> Result<Option<GitStatusResponse>, String> {
    let expanded_path = expand_path(&path)?;
    let path = PathBuf::from(expanded_path);

    if !path.exists() {
        return Err("Path does not exist".to_string());
    }

    tauri::async_runtime::spawn_blocking(move || compute_git_status(&path))
        .await
        .map_err(|err| format!("Failed to join Git status task: {err}"))?
}

#[command]
pub async fn read_directory(path: LocationInput) -> Result<DirectoryListingResponse, String> {
    let (provider, location) = resolve_location(path)?;
    let listing = provider.read_directory(&location).await?;
    let capabilities = provider.capabilities(&location);

    Ok(DirectoryListingResponse {
        location: listing.location,
        capabilities,
        entries: listing.entries,
    })
}

/// Event name for directory streaming batches (skeleton files)
const DIRECTORY_BATCH_EVENT: &str = "directory-batch";
/// Event name for file metadata updates (size, dates, etc.)
const METADATA_BATCH_EVENT: &str = "metadata-batch";

/// Response returned when starting a streaming directory read
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamingDirectoryResponse {
    pub session_id: String,
    pub location: LocationSummary,
    pub capabilities: LocationCapabilities,
}

/// Start streaming directory contents. Returns immediately with session info,
/// then emits batches via the "directory-batch" event.
/// The session_id is provided by the frontend to avoid race conditions - the frontend
/// sets up the session ID in state BEFORE calling this command, ensuring batches
/// arriving via events will be accepted immediately.
#[command]
pub async fn read_directory_streaming_command(
    app: AppHandle,
    state: tauri::State<'_, DirectoryStreamState>,
    path: LocationInput,
    session_id: String,
) -> Result<StreamingDirectoryResponse, String> {
    let (provider, location) = resolve_location(path)?;
    let capabilities = provider.capabilities(&location);

    // Only support file:// locations for streaming for now
    if location.scheme() != "file" {
        return Err("Streaming only supported for local file system".to_string());
    }

    let raw_path = location.to_path_string();
    let expanded_path = expand_path(&raw_path)?;

    if !expanded_path.exists() {
        return Err("Path does not exist".to_string());
    }
    if !expanded_path.is_dir() {
        return Err("Path is not a directory".to_string());
    }

    // Use the session_id provided by the frontend
    let cancel_flag = Arc::new(AtomicBool::new(false));

    // Register the session
    {
        let mut guard = state
            .sessions
            .lock()
            .map_err(|e| format!("Failed to lock sessions: {}", e))?;

        // If there's an existing session with the same ID (unlikely), cancel it
        if let Some(existing) = guard.insert(
            session_id.clone(),
            DirectoryStreamHandle {
                cancel_flag: cancel_flag.clone(),
            },
        ) {
            existing.cancel_flag.store(true, Ordering::SeqCst);
        }
    }

    let location_summary = LocationSummary::new(
        "file",
        location.authority().map(|s| s.to_string()),
        expanded_path.to_string_lossy().to_string(),
        expanded_path.to_string_lossy().to_string(),
    );

    // Clone values for the async task
    let app_for_task = app.clone();
    let session_for_task = session_id.clone();
    let cancel_for_task = cancel_flag;
    let path_for_task = expanded_path;

    // Spawn background task for streaming
    tauri::async_runtime::spawn(async move {
        let session_for_blocking = session_for_task.clone();
        let cancel_for_blocking = cancel_for_task.clone();
        let app_for_batches = app_for_task.clone();
        let app_for_metadata = app_for_task;

        let result = tauri::async_runtime::spawn_blocking(move || {
            read_directory_streaming(
                &path_for_task,
                session_for_blocking,
                cancel_for_blocking,
                |batch| {
                    // Emit skeleton file batches (instant UI)
                    if let Err(e) = app_for_batches.emit(DIRECTORY_BATCH_EVENT, &batch) {
                        warn!("Failed to emit directory batch: {}", e);
                    }
                },
                |metadata_batch| {
                    // Emit metadata updates (fills in size, dates, etc.)
                    if let Err(e) = app_for_metadata.emit(METADATA_BATCH_EVENT, &metadata_batch) {
                        warn!("Failed to emit metadata batch: {}", e);
                    }
                },
            )
        })
        .await;

        if let Err(e) = result {
            warn!("Directory streaming task failed: {}", e);
        }
    });

    Ok(StreamingDirectoryResponse {
        session_id,
        location: location_summary,
        capabilities,
    })
}

/// Cancel an active directory streaming session
#[command]
pub fn cancel_directory_stream(
    state: tauri::State<'_, DirectoryStreamState>,
    session_id: String,
) -> Result<(), String> {
    let guard = state
        .sessions
        .lock()
        .map_err(|e| format!("Failed to lock sessions: {}", e))?;

    if let Some(handle) = guard.get(&session_id) {
        handle.cancel_flag.store(true, Ordering::SeqCst);
    }

    Ok(())
}

#[command]
pub async fn get_file_metadata(path: LocationInput) -> Result<FileItem, String> {
    let (provider, location) = resolve_location(path)?;
    provider.get_file_metadata(&location).await
}

#[command]
pub fn resolve_symlink_parent_command(path: String) -> Result<SymlinkResolution, String> {
    let expanded_path = expand_path(&path)?;
    let path = Path::new(&expanded_path);

    resolve_symlink_parent(path)
}

#[command]
pub async fn create_directory_command(path: LocationInput) -> Result<(), String> {
    let (provider, location) = resolve_location(path)?;
    let capabilities = provider.capabilities(&location);
    if !capabilities.can_create_directories {
        return Err("Provider does not support creating directories".to_string());
    }
    provider.create_directory(&location).await
}

#[command]
pub async fn delete_file(path: LocationInput) -> Result<(), String> {
    let (provider, location) = resolve_location(path)?;
    let capabilities = provider.capabilities(&location);
    if !capabilities.can_delete {
        return Err("Provider does not support deleting items".to_string());
    }
    provider.delete(&location).await
}

#[command]
pub async fn trash_paths(app: AppHandle, paths: Vec<String>) -> Result<TrashPathsResponse, String> {
    if paths.is_empty() {
        return Ok(TrashPathsResponse {
            trashed: Vec::new(),
            undo_token: None,
            fallback_to_permanent: false,
        });
    }

    let mut expanded: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for original in paths {
        let path_buf = expand_path(&original)?;
        if !path_buf.exists() {
            return Err(format!("Path does not exist: {}", original));
        }
        expanded.push(path_buf);
    }

    let original_paths: Vec<String> = expanded
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();

    let delete_targets: Vec<PathBuf> = expanded.clone();
    let state = app.state::<TrashUndoState>();

    // Capture trash state BEFORE deletion on Windows/Linux for undo tracking
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let before_ids: HashSet<OsString> = {
        let before =
            os_limited::list().map_err(|err| format!("Failed to inspect trash: {err}"))?;
        before.into_iter().map(|item| item.id).collect()
    };

    #[cfg(target_os = "macos")]
    let mac_records = {
        let task_paths = delete_targets.clone();
        let task_result = tauri::async_runtime::spawn_blocking(move || macos_trash_items(&task_paths))
            .await
            .map_err(|err| format!("Failed to join trash task: {err}"))?;

        match task_result {
            Ok(records) => records,
            Err(err) => {
                let lowered = err.to_lowercase();
                if lowered.contains("operation not permitted") || lowered.contains("permission") {
                    return Ok(TrashPathsResponse {
                        trashed: Vec::new(),
                        undo_token: None,
                        fallback_to_permanent: true,
                    });
                }
                return Err(err);
            }
        }
    };

    #[cfg(not(target_os = "macos"))]
    {
        let delete_result = tauri::async_runtime::spawn_blocking(move || {
            trash::delete_all(delete_targets.iter())
        })
        .await
        .map_err(|err| format!("Failed to join trash task: {err}"))?;

        if let Err(err) = delete_result {
            return Err(err.to_string());
        }
    }

    cleanup_trash_undo_records(&state);

    let undo_token: Option<String> = {
        #[cfg(target_os = "macos")]
        {
            if mac_records.is_empty() {
                None
            } else {
                let token = Uuid::new_v4().to_string();
                let record = TrashUndoRecord {
                    kind: crate::state::TrashUndoKind::MacItems(mac_records.clone()),
                    original_paths: original_paths.clone(),
                    created_at: Instant::now(),
                };
                if let Ok(mut records) = state.records.lock() {
                    records.insert(token.clone(), record);
                }
                Some(token)
            }
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            let normalized_targets: HashSet<String> = expanded
                .iter()
                .map(|path| normalize_path_for_compare(path))
                .collect();
            let after = os_limited::list()
                .map_err(|err| format!("Failed to inspect trash after deletion: {err}"))?;

            let mut new_items = Vec::new();
            for item in after {
                if before_ids.contains(&item.id) {
                    continue;
                }

                if normalized_targets.contains(&normalize_path_for_compare(&item.original_path())) {
                    new_items.push(item);
                }
            }

            if !new_items.is_empty() {
                let token = Uuid::new_v4().to_string();
                let record = TrashUndoRecord {
                    kind: crate::state::TrashUndoKind::Items(new_items),
                    original_paths: original_paths.clone(),
                    created_at: Instant::now(),
                };
                if let Ok(mut records) = state.records.lock() {
                    records.insert(token.clone(), record);
                }
                Some(token)
            } else {
                None
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            None
        }
    };

    Ok(TrashPathsResponse {
        trashed: original_paths,
        undo_token,
        fallback_to_permanent: false,
    })
}

#[command]
pub async fn undo_trash(app: AppHandle, token: String) -> Result<UndoTrashResponse, String> {
    if token.trim().is_empty() {
        return Err("Undo token is required".to_string());
    }

    let state = app.state::<TrashUndoState>();
    cleanup_trash_undo_records(&state);

    let record = {
        let mut records = state
            .records
            .lock()
            .map_err(|_| "Failed to access undo state".to_string())?;
        records.remove(&token)
    };

    let Some(record) = record else {
        return Err("Undo request is no longer available.".to_string());
    };

    #[cfg(target_os = "macos")]
    {
        match record.kind {
            crate::state::TrashUndoKind::MacItems(items) => {
                let restore_items = items.clone();
                let restore_result = tauri::async_runtime::spawn_blocking(move || {
                    macos_restore_items(&restore_items)
                })
                .await
                .map_err(|err| format!("Failed to join restore task: {err}"))?;

                if let Err(err) = restore_result {
                    return Err(err);
                }
            }
        }

        return Ok(UndoTrashResponse {
            restored: record.original_paths,
        });
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        match record.kind {
            crate::state::TrashUndoKind::Items(items) => {
                let restore_result =
                    tauri::async_runtime::spawn_blocking(move || os_limited::restore_all(items))
                        .await
                        .map_err(|err| format!("Failed to join restore task: {err}"))?;
                if let Err(err) = restore_result {
                    return Err(err.to_string());
                }
            }
            #[allow(unreachable_patterns)]
            _ => {
                return Err("Undo is not supported on this platform.".to_string());
            }
        }

        return Ok(UndoTrashResponse {
            restored: record.original_paths,
        });
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = record;
        return Err("Undo is not supported on this platform.".to_string());
    }
}

#[command]
pub async fn delete_paths_permanently(
    app: AppHandle,
    paths: Vec<String>,
    request_id: String,
) -> Result<DeletePathsResponse, String> {
    if paths.is_empty() {
        return Ok(DeletePathsResponse {
            deleted: Vec::new(),
        });
    }

    let mut expanded: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for original in paths {
        let path_buf = expand_path(&original)?;
        if !path_buf.exists() {
            return Err(format!("Path does not exist: {}", original));
        }
        expanded.push(path_buf);
    }

    let total = expanded.len();
    let original_paths: Vec<String> = expanded
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();
    let targets: Vec<PathBuf> = expanded.clone();

    let app_handle = app.clone();
    let request_id_clone = request_id.clone();

    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        for (idx, target) in targets.iter().enumerate() {
            let current_path = target.to_string_lossy().to_string();
            emit_delete_progress_update(
                &app_handle,
                DeleteProgressUpdatePayload {
                    request_id: request_id_clone.clone(),
                    current_path: Some(current_path.clone()),
                    completed: idx,
                    total,
                    finished: false,
                    error: None,
                },
            );

            if let Err(err) = delete_file_or_directory(target) {
                emit_delete_progress_update(
                    &app_handle,
                    DeleteProgressUpdatePayload {
                        request_id: request_id_clone.clone(),
                        current_path: Some(current_path),
                        completed: idx,
                        total,
                        finished: true,
                        error: Some(err.clone()),
                    },
                );
                return Err(err);
            }
        }

        emit_delete_progress_update(
            &app_handle,
            DeleteProgressUpdatePayload {
                request_id: request_id_clone,
                current_path: None,
                completed: total,
                total,
                finished: true,
                error: None,
            },
        );

        Ok(())
    })
    .await
    .map_err(|err| format!("Failed to join delete task: {err}"))??;

    Ok(DeletePathsResponse {
        deleted: original_paths,
    })
}

#[command]
pub async fn rename_file(from_path: LocationInput, to_path: LocationInput) -> Result<(), String> {
    let (from_provider, from_location) = resolve_location(from_path)?;
    let (_, to_location) = resolve_location(to_path)?;

    if from_location.scheme() != to_location.scheme() {
        return Err("Renaming across different providers is not supported".to_string());
    }

    let capabilities = from_provider.capabilities(&from_location);
    if !capabilities.can_rename {
        return Err("Provider does not support renaming".to_string());
    }

    // When both locations share the same scheme we can rely on the source provider.
    from_provider.rename(&from_location, &to_location).await
}

#[command]
pub async fn copy_file(from_path: LocationInput, to_path: LocationInput) -> Result<(), String> {
    let (from_provider, from_location) = resolve_location(from_path)?;
    let (_, to_location) = resolve_location(to_path)?;

    if from_location.scheme() != to_location.scheme() {
        return Err("Copying across different providers is not yet supported".to_string());
    }

    let capabilities = from_provider.capabilities(&from_location);
    if !capabilities.can_copy {
        return Err("Provider does not support copy operations".to_string());
    }

    from_provider.copy(&from_location, &to_location).await
}

#[command]
pub async fn move_file(from_path: LocationInput, to_path: LocationInput) -> Result<(), String> {
    let (from_provider, from_location) = resolve_location(from_path)?;
    let (_, to_location) = resolve_location(to_path)?;

    if from_location.scheme() != to_location.scheme() {
        return Err("Moving across different providers is not yet supported".to_string());
    }

    let capabilities = from_provider.capabilities(&from_location);
    if !capabilities.can_move {
        return Err("Provider does not support move operations".to_string());
    }

    from_provider.move_item(&from_location, &to_location).await
}

fn allocate_destination_folder(destination_root: &Path, base_name: &str) -> Result<String, String> {
    let trimmed = base_name.trim();
    let base = if trimmed.is_empty() {
        "Archive"
    } else {
        trimmed
    };
    let mut candidate = base.to_string();
    let mut counter: u32 = 2;

    loop {
        let attempt_path = destination_root.join(&candidate);
        if !attempt_path.exists() {
            return Ok(candidate);
        }

        candidate = format!("{base} ({counter})");
        counter += 1;

        if counter > 10_000 {
            return Err("Unable to allocate unique folder name for extracted archive".to_string());
        }
    }
}

fn extract_zip_contents<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    target_dir: &Path,
    mut on_entry: impl FnMut(&str),
) -> Result<(), String> {
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|err| format!("Failed to read archive entry {index}: {err}"))?;

        let enclosed_path = match entry.enclosed_name() {
            Some(path) => path.to_path_buf(),
            None => {
                warn!(
                    "Skipping archive entry {name:?} due to invalid path",
                    name = entry.name()
                );
                continue;
            }
        };

        let out_path = target_dir.join(&enclosed_path);

        let entry_name = enclosed_path.to_string_lossy().to_string();

        if entry.name().ends_with('/') || entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|err| {
                format!("Failed to create directory {}: {}", out_path.display(), err)
            })?;
            #[cfg(target_family = "unix")]
            if let Some(mode) = entry.unix_mode() {
                if let Err(err) = fs::set_permissions(&out_path, fs::Permissions::from_mode(mode)) {
                    warn!(
                        "Failed to set permissions on {}: {}",
                        out_path.display(),
                        err
                    );
                }
            }
            continue;
        }

        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                format!("Failed to create directory {}: {}", parent.display(), err)
            })?;
        }

        let mut outfile = fs::File::create(&out_path)
            .map_err(|err| format!("Failed to create file {}: {}", out_path.display(), err))?;

        std::io::copy(&mut entry, &mut outfile)
            .map_err(|err| format!("Failed to write file {}: {}", out_path.display(), err))?;

        #[cfg(target_family = "unix")]
        if let Some(mode) = entry.unix_mode() {
            if let Err(err) = fs::set_permissions(&out_path, fs::Permissions::from_mode(mode)) {
                warn!(
                    "Failed to set permissions on {}: {}",
                    out_path.display(),
                    err
                );
            }
        }
        on_entry(&entry_name);
    }

    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum ArchiveFormat {
    Zip,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    TarZst,
}

impl ArchiveFormat {
    fn as_str(&self) -> &'static str {
        match self {
            ArchiveFormat::Zip => "zip",
            ArchiveFormat::Tar => "tar",
            ArchiveFormat::TarGz => "tar.gz",
            ArchiveFormat::TarBz2 => "tar.bz2",
            ArchiveFormat::TarXz => "tar.xz",
            ArchiveFormat::TarZst => "tar.zst",
        }
    }
}

fn archive_format_from_hint(hint: &str) -> Option<ArchiveFormat> {
    match hint {
        "zip" => Some(ArchiveFormat::Zip),
        "tar" => Some(ArchiveFormat::Tar),
        "tar.gz" => Some(ArchiveFormat::TarGz),
        "tar.bz2" => Some(ArchiveFormat::TarBz2),
        "tar.xz" => Some(ArchiveFormat::TarXz),
        "tar.zst" => Some(ArchiveFormat::TarZst),
        _ => None,
    }
}

fn infer_archive_format_from_name(name: &str) -> Option<ArchiveFormat> {
    let lower = name.to_lowercase();
    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        Some(ArchiveFormat::TarGz)
    } else if lower.ends_with(".tar.bz2") || lower.ends_with(".tbz2") || lower.ends_with(".tbz") {
        Some(ArchiveFormat::TarBz2)
    } else if lower.ends_with(".tar.xz") || lower.ends_with(".txz") {
        Some(ArchiveFormat::TarXz)
    } else if lower.ends_with(".tar.zst") || lower.ends_with(".tzst") {
        Some(ArchiveFormat::TarZst)
    } else if lower.ends_with(".tar") {
        Some(ArchiveFormat::Tar)
    } else if lower.ends_with(".zip") {
        Some(ArchiveFormat::Zip)
    } else {
        None
    }
}

fn determine_archive_format(
    path: &Path,
    format_hint: Option<&str>,
) -> Result<ArchiveFormat, String> {
    if let Some(hint) = format_hint.and_then(archive_format_from_hint) {
        return Ok(hint);
    }

    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "Unable to determine archive file name".to_string())?;

    infer_archive_format_from_name(name)
        .ok_or_else(|| format!("Unsupported archive format for {}", path.to_string_lossy()))
}

fn extract_tar_from_reader<R: Read>(
    reader: R,
    target_dir: &Path,
    mut on_entry: impl FnMut(&str),
) -> Result<(), String> {
    let mut archive = TarArchive::new(reader);
    let entries = archive
        .entries()
        .map_err(|err| format!("Failed to iterate TAR entries: {}", err))?;

    for entry_result in entries {
        let mut entry = entry_result.map_err(|err| format!("Failed to read TAR entry: {}", err))?;
        let entry_path = entry
            .path()
            .map_err(|err| format!("Failed to resolve TAR entry path: {}", err))?;

        let entry_name = entry_path.to_string_lossy().to_string();
        entry
            .unpack_in(target_dir)
            .map_err(|err| format!("Failed to unpack TAR entry {}: {}", entry_name, err))?;

        on_entry(&entry_name);
    }

    Ok(())
}

fn create_tar_reader(
    archive_format: ArchiveFormat,
    archive_path: &Path,
) -> Result<Box<dyn Read>, String> {
    let file = fs::File::open(archive_path)
        .map_err(|err| format!("Failed to open archive {}: {}", archive_path.display(), err))?;

    let reader: Box<dyn Read> = match archive_format {
        ArchiveFormat::Tar => Box::new(file),
        ArchiveFormat::TarGz => Box::new(GzDecoder::new(file)),
        ArchiveFormat::TarBz2 => Box::new(BzDecoder::new(file)),
        ArchiveFormat::TarXz => Box::new(XzDecoder::new(file)),
        ArchiveFormat::TarZst => {
            let decoder = ZstdDecoder::new(file).map_err(|err| {
                format!("Failed to read archive {}: {}", archive_path.display(), err)
            })?;
            Box::new(decoder)
        }
        ArchiveFormat::Zip => unreachable!(),
    };

    Ok(reader)
}

fn derive_folder_base_name(path: &Path, format: ArchiveFormat) -> Result<String, String> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "Unable to derive folder name from archive".to_string())?;
    let lower = name.to_lowercase();

    let patterns: &[&str] = match format {
        ArchiveFormat::Zip => &[".zip"],
        ArchiveFormat::Tar => &[".tar"],
        ArchiveFormat::TarGz => &[".tar.gz", ".tgz"],
        ArchiveFormat::TarBz2 => &[".tar.bz2", ".tbz2", ".tbz"],
        ArchiveFormat::TarXz => &[".tar.xz", ".txz"],
        ArchiveFormat::TarZst => &[".tar.zst", ".tzst"],
    };

    for pattern in patterns {
        if lower.ends_with(pattern) && name.len() > pattern.len() {
            let end = name.len() - pattern.len();
            return Ok(name[..end].to_string());
        }
    }

    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    Ok(stem.to_string())
}

fn emit_archive_progress_update(
    app: &AppHandle,
    archive_name: &str,
    entry_name: Option<&str>,
    format: ArchiveFormat,
    finished: bool,
) {
    let payload = ArchiveProgressUpdatePayload {
        archive_name: archive_name.to_string(),
        entry_name: entry_name.map(|s| s.to_string()),
        format: format.as_str().to_string(),
        finished,
    };

    if let Err(err) = app.emit(ARCHIVE_PROGRESS_UPDATE_EVENT, payload) {
        warn!("Failed to emit archive progress update: {err}");
    }
}

fn cleanup_directory(path: &Path) {
    if let Err(err) = fs::remove_dir_all(path) {
        if path.exists() {
            warn!(
                "Failed to remove extraction directory {}: {}",
                path.display(),
                err
            );
        }
    }
}

#[cfg(target_os = "macos")]
fn extract_with_system(archive_path: &Path, target_dir: &Path) -> Result<(), String> {
    if target_dir.exists() {
        cleanup_directory(target_dir);
    }
    fs::create_dir(target_dir).map_err(|err| {
        format!(
            "Failed to create extraction directory {} before fallback: {}",
            target_dir.display(),
            err
        )
    })?;

    let status = OsCommand::new("ditto")
        .arg("-x")
        .arg("-k")
        .arg(archive_path)
        .arg(target_dir)
        .status()
        .map_err(|err| format!("Failed to spawn 'ditto' for fallback extraction: {}", err))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("ditto exited with status: {}", status))
    }
}

#[cfg(target_os = "windows")]
fn extract_with_system(archive_path: &Path, target_dir: &Path) -> Result<(), String> {
    if target_dir.exists() {
        cleanup_directory(target_dir);
    }

    let archive_str = archive_path.to_string_lossy().replace('\'', "''");
    let target_str = target_dir.to_string_lossy().replace('\'', "''");
    let command = format!(
        "Expand-Archive -LiteralPath '{archive}' -DestinationPath '{destination}' -Force",
        archive = archive_str,
        destination = target_str
    );

    let status = OsCommand::new("powershell")
        .args(["-NoLogo", "-NoProfile", "-Command", &command])
        .status()
        .map_err(|err| {
            format!(
                "Failed to spawn PowerShell for fallback extraction: {}",
                err
            )
        })?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "PowerShell Expand-Archive exited with status: {}",
            status
        ))
    }
}

#[cfg(target_os = "linux")]
fn extract_with_system(archive_path: &Path, target_dir: &Path) -> Result<(), String> {
    if target_dir.exists() {
        cleanup_directory(target_dir);
    }

    fs::create_dir_all(target_dir).map_err(|err| {
        format!(
            "Failed to create extraction directory {} before fallback: {}",
            target_dir.display(),
            err
        )
    })?;

    let status = OsCommand::new("unzip")
        .args([
            "-q",
            archive_path.to_string_lossy().as_ref(),
            "-d",
            target_dir.to_string_lossy().as_ref(),
        ])
        .status()
        .map_err(|err| format!("Failed to spawn 'unzip' for fallback extraction: {}", err))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("'unzip' exited with status: {}", status))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn extract_with_system(_archive_path: &Path, _target_dir: &Path) -> Result<(), String> {
    Err("System fallback extraction is not supported on this platform".to_string())
}

#[command]
pub async fn extract_archive(
    app: AppHandle,
    archive_path: String,
    destination_dir: String,
    format_hint: Option<String>,
) -> Result<ExtractArchiveResponse, String> {
    let expanded_archive = expand_path(&archive_path)?;
    let expanded_destination = expand_path(&destination_dir)?;

    let archive_path = PathBuf::from(&expanded_archive);
    if !archive_path.exists() {
        return Err("Archive path does not exist".to_string());
    }
    if !archive_path.is_file() {
        return Err("Archive path is not a file".to_string());
    }

    let destination_root = PathBuf::from(&expanded_destination);
    if !destination_root.exists() {
        return Err("Destination directory does not exist".to_string());
    }
    if !destination_root.is_dir() {
        return Err("Destination path is not a directory".to_string());
    }

    let archive_format = determine_archive_format(&archive_path, format_hint.as_deref())?;
    let base_name = derive_folder_base_name(&archive_path, archive_format)?;
    let allocated_folder = allocate_destination_folder(&destination_root, &base_name)?;

    let archive_for_task = archive_path.clone();
    let destination_for_task = destination_root.clone();
    let folder_name_for_task = allocated_folder.clone();
    let app_handle = app.clone();
    let archive_name = archive_path
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| base_name.clone());
    let archive_name_for_task = archive_name.clone();

    info!(
        "extract_archive requested: {} -> {} (format: {})",
        archive_path.display(),
        destination_root.display(),
        archive_format.as_str()
    );

    emit_archive_progress_update(&app, &archive_name, None, archive_format, false);

    let (extracted_path, used_system_fallback) =
        tauri::async_runtime::spawn_blocking(move || -> Result<(PathBuf, bool), String> {
            let archive_name = archive_name_for_task;
            let target_dir = destination_for_task.join(&folder_name_for_task);

            fs::create_dir(&target_dir).map_err(|err| {
                format!(
                    "Failed to create extraction directory {}: {}",
                    target_dir.display(),
                    err
                )
            })?;

            match archive_format {
                ArchiveFormat::Zip => {
                    let native_result = (|| -> Result<(), String> {
                        let file = fs::File::open(&archive_for_task).map_err(|err| {
                            format!(
                                "Failed to open archive {}: {}",
                                archive_for_task.display(),
                                err
                            )
                        })?;

                        let mut zip_archive = ZipArchive::new(file).map_err(|err| {
                            format!(
                                "Failed to read archive {}: {}",
                                archive_for_task.display(),
                                err
                            )
                        })?;

                        extract_zip_contents(&mut zip_archive, &target_dir, |entry_name| {
                            emit_archive_progress_update(
                                &app_handle,
                                &archive_name,
                                Some(entry_name),
                                archive_format,
                                false,
                            );
                        })
                    })();

                    match native_result {
                        Ok(()) => {
                            info!(
                                "Native ZIP extraction succeeded for {} into {}",
                                archive_for_task.display(),
                                target_dir.display()
                            );
                            Ok((target_dir, false))
                        }
                        Err(native_err) => {
                            warn!(
                                "Native ZIP extraction failed for {}: {}. Attempting system fallback...",
                                archive_for_task.display(),
                                native_err
                            );

                            cleanup_directory(&target_dir);

                            match extract_with_system(&archive_for_task, &target_dir) {
                                Ok(()) => {
                                    info!(
                                        "System fallback extraction succeeded for {} into {}",
                                        archive_for_task.display(),
                                        target_dir.display()
                                    );
                                    Ok((target_dir, true))
                                }
                                Err(fallback_err) => Err(format!(
                                    "Native extraction failed ({native_err}). System fallback failed: {fallback_err}"
                                )),
                            }
                        }
                    }
                }
                ArchiveFormat::Tar
                | ArchiveFormat::TarGz
                | ArchiveFormat::TarBz2
                | ArchiveFormat::TarXz
                | ArchiveFormat::TarZst => {
                    let extraction_result = (|| -> Result<(), String> {
                        let reader = create_tar_reader(archive_format, &archive_for_task)?;
                        extract_tar_from_reader(reader, &target_dir, |entry_name| {
                            emit_archive_progress_update(
                                &app_handle,
                                &archive_name,
                                Some(entry_name),
                                archive_format,
                                false,
                            );
                        })
                    })();

                    extraction_result.map_err(|err| {
                        cleanup_directory(&target_dir);
                        err
                    })?;

                    info!(
                        "{} extraction succeeded for {} into {}",
                        archive_format.as_str().to_uppercase(),
                        archive_for_task.display(),
                        target_dir.display()
                    );
                    Ok((target_dir, false))
                }
            }
        })
        .await
        .map_err(|err| format!("Failed to join archive extraction task: {}", err))??;

    info!(
        "Extraction complete for {} -> {} (fallback: {}, format: {})",
        archive_path.display(),
        extracted_path.display(),
        used_system_fallback,
        archive_format.as_str()
    );

    emit_archive_progress_update(&app, &archive_name, None, archive_format, true);

    Ok(ExtractArchiveResponse {
        folder_path: extracted_path.to_string_lossy().to_string(),
        used_system_fallback,
        format: archive_format.as_str().to_string(),
    })
}

#[command]
pub fn get_system_accent_color() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        // Try AppleHighlightColor first — includes a color name token we can map
        if let Ok(output) = OsCommand::new("defaults")
            .arg("read")
            .arg("-g")
            .arg("AppleHighlightColor")
            .output()
        {
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout);
                let tokens: Vec<&str> = s.split_whitespace().collect();
                if let Some(name) = tokens.last() {
                    let hex = match name.to_lowercase().as_str() {
                        "blue" => "#0a84ff",
                        "purple" => "#bf5af2",
                        "pink" => "#ff375f",
                        "red" => "#ff453a",
                        "orange" => "#ff9f0a",
                        "yellow" => "#ffd60a",
                        "green" => "#30d158",
                        "graphite" => "#8e8e93",
                        _ => "",
                    };
                    if !hex.is_empty() {
                        return Ok(hex.to_string());
                    }
                }

                // Fallback: parse first 3 floats as RGB 0..1
                let mut nums = tokens.iter().filter_map(|t| t.parse::<f32>().ok());
                if let (Some(r), Some(g), Some(b)) = (nums.next(), nums.next(), nums.next()) {
                    let to_byte = |v: f32| -> u8 { (v.clamp(0.0, 1.0) * 255.0).round() as u8 };
                    let (r, g, b) = (to_byte(r), to_byte(g), to_byte(b));
                    return Ok(format!("#{:02x}{:02x}{:02x}", r, g, b));
                }
            }
        }
        // Final fallback
        Ok("#3584e4".to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok("#3584e4".to_string())
    }
}

#[tauri::command]
pub fn get_application_icon(path: String, size: Option<u32>) -> Result<String, String> {
    // On macOS, prefer native AppKit icon rendering; otherwise unsupported.
    #[cfg(target_os = "macos")]
    {
        let size = size.unwrap_or(96);
        return crate::macos_icons::app_icon_png_base64(&path, size);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        let _ = size;
        Err("get_application_icon is only supported on macOS".to_string())
    }
}

#[tauri::command]
pub fn render_svg_to_png(svg: String, size: Option<u32>) -> Result<String, String> {
    // Parse SVG (usvg 0.43 API)
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(svg.as_bytes(), &opt)
        .map_err(|e| format!("SVG parse error: {:?}", e))?;

    // Allocate pixmap
    let target = size.unwrap_or(64).max(1);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(target, target)
        .ok_or_else(|| "Failed to allocate pixmap".to_string())?;

    // Fit into square with a small padding to avoid bleeding against edges
    let svg_size = tree.size();
    let (w, h) = (svg_size.width().max(1.0), svg_size.height().max(1.0));
    let padding = (target as f32 * 0.08).round();
    let inner = (target as f32 - 2.0 * padding).max(1.0);
    let s = (inner / w).min(inner / h);
    let scaled_w = w * s;
    let scaled_h = h * s;
    let mut ts = resvg::tiny_skia::Transform::from_scale(s, s);
    let tx = ((target as f32 - scaled_w) * 0.5).round();
    let ty = ((target as f32 - scaled_h) * 0.5).round();
    ts = ts.post_translate(tx, ty);

    let mut pmut = pixmap.as_mut();
    resvg::render(&tree, ts, &mut pmut);

    // Convert premultiplied BGRA pixels -> non-premultiplied RGBA for PNG encoder
    let data = pixmap.data();
    let mut rgba = Vec::with_capacity(data.len());
    for px in data.chunks_exact(4) {
        let r = px[0] as u32;
        let g = px[1] as u32;
        let b = px[2] as u32;
        let a = px[3] as u32;
        if a == 0 {
            rgba.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            // Un-premultiply and reorder to RGBA
            let ur = ((r * 255 + a / 2) / a).min(255) as u8;
            let ug = ((g * 255 + a / 2) / a).min(255) as u8;
            let ub = ((b * 255 + a / 2) / a).min(255) as u8;
            rgba.extend_from_slice(&[ur, ug, ub, a as u8]);
        }
    }

    // Encode PNG with the image crate
    let img = image::ImageBuffer::<image::Rgba<u8>, _>::from_vec(target, target, rgba)
        .ok_or_else(|| "Failed to create image buffer".to_string())?;
    let mut out = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut out);
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .map_err(|e| format!("PNG encode error: {}", e))?;

    let data_url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(out)
    );
    Ok(data_url)
}

#[tauri::command]
pub fn update_hidden_files_menu(
    _app: tauri::AppHandle,
    menu_state: tauri::State<crate::state::MenuState<tauri::Wry>>,
    checked: bool,
    source: Option<String>,
) -> Result<(), String> {
    let _source_str = source.unwrap_or_else(|| "UNKNOWN".to_string());

    let item_guard = menu_state
        .show_hidden_item
        .lock()
        .map_err(|e| format!("Failed to acquire lock: {}", e))?;

    if let Some(ref item) = *item_guard {
        item.set_checked(checked).map_err(|e| e.to_string())?;
    } else {
        return Err("Menu item not found in state".to_string());
    }
    // Update the stored boolean for consistency
    if let Ok(mut flag) = menu_state.show_hidden_checked.lock() {
        *flag = checked;
    }

    Ok(())
}

#[tauri::command]
pub fn update_folders_first_menu(
    _app: tauri::AppHandle,
    menu_state: tauri::State<crate::state::MenuState<tauri::Wry>>,
    checked: bool,
    _source: Option<String>,
) -> Result<(), String> {
    // Update the menu item checked state if available
    if let Ok(item_guard) = menu_state.folders_first_item.lock() {
        if let Some(ref item) = *item_guard {
            item.set_checked(checked).map_err(|e| e.to_string())?;
        }
    }

    // Update the stored boolean
    if let Ok(mut flag) = menu_state.folders_first_checked.lock() {
        *flag = checked;
    }

    Ok(())
}

#[tauri::command]
pub fn update_sort_menu_state(
    _app: tauri::AppHandle,
    menu_state: tauri::State<crate::state::MenuState<tauri::Wry>>,
    sort_by: String,
    ascending: bool,
) -> Result<(), String> {
    if let Ok(mut sb) = menu_state.current_sort_by.lock() {
        *sb = sort_by;
    }
    if let Ok(mut asc) = menu_state.sort_order_asc_checked.lock() {
        *asc = ascending;
    }
    // Update system menu checkboxes if available
    let set_checked =
        |item_mutex: &std::sync::Mutex<Option<tauri::menu::CheckMenuItem<tauri::Wry>>>,
         value: bool| {
            if let Ok(item_guard) = item_mutex.lock() {
                if let Some(ref item) = *item_guard {
                    let _ = item.set_checked(value);
                }
            }
        };
    match menu_state
        .current_sort_by
        .lock()
        .map(|s| s.clone())
        .unwrap_or_else(|_| "name".to_string())
        .as_str()
    {
        "name" => {
            set_checked(&menu_state.sort_name_item, true);
            set_checked(&menu_state.sort_size_item, false);
            set_checked(&menu_state.sort_type_item, false);
            set_checked(&menu_state.sort_modified_item, false);
        }
        "size" => {
            set_checked(&menu_state.sort_name_item, false);
            set_checked(&menu_state.sort_size_item, true);
            set_checked(&menu_state.sort_type_item, false);
            set_checked(&menu_state.sort_modified_item, false);
        }
        "type" => {
            set_checked(&menu_state.sort_name_item, false);
            set_checked(&menu_state.sort_size_item, false);
            set_checked(&menu_state.sort_type_item, true);
            set_checked(&menu_state.sort_modified_item, false);
        }
        "modified" => {
            set_checked(&menu_state.sort_name_item, false);
            set_checked(&menu_state.sort_size_item, false);
            set_checked(&menu_state.sort_type_item, false);
            set_checked(&menu_state.sort_modified_item, true);
        }
        _ => {}
    }
    set_checked(&menu_state.sort_asc_item, ascending);
    set_checked(&menu_state.sort_desc_item, !ascending);
    Ok(())
}

#[tauri::command]
pub fn update_selection_menu_state(
    _app: tauri::AppHandle,
    menu_state: tauri::State<crate::state::MenuState<tauri::Wry>>,
    has_selection: bool,
) -> Result<(), String> {
    if let Ok(mut sel) = menu_state.has_selection.lock() {
        *sel = has_selection;
    }
    Ok(())
}

#[derive(serde::Serialize)]
pub struct SystemDrive {
    pub name: String,
    pub path: String,
    pub drive_type: String,
    pub is_ejectable: bool,
}

#[command]
pub fn get_system_drives() -> Result<Vec<SystemDrive>, String> {
    let mut drives = Vec::new();

    #[cfg(target_os = "windows")]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;

        // Get all logical drives on Windows
        let mut drive_strings = vec![0u16; 256];
        let length = unsafe {
            windows::Win32::Storage::FileSystem::GetLogicalDriveStringsW(
                drive_strings.len() as u32,
                drive_strings.as_mut_ptr(),
            )
        };

        if length > 0 && length < drive_strings.len() as u32 {
            drive_strings.truncate(length as usize);
            let drives_str = OsString::from_wide(&drive_strings);
            let drives_string = drives_str.to_string_lossy();

            for drive in drives_string.split('\0').filter(|s| !s.is_empty()) {
                if drive.len() >= 3 {
                    drives.push(SystemDrive {
                        name: format!("Local Disk ({})", &drive[..2]),
                        path: drive.to_string(),
                        drive_type: "system".to_string(),
                        is_ejectable: false, // TODO: Check if removable drive
                    });
                }
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        // On Unix-like systems, add the root filesystem
        drives.push(SystemDrive {
            name: "File System".to_string(),
            path: "/".to_string(),
            drive_type: "system".to_string(),
            is_ejectable: false,
        });

        // On macOS, also try to add mounted volumes
        #[cfg(target_os = "macos")]
        {
            if let Ok(entries) = std::fs::read_dir("/Volumes") {
                for entry in entries.flatten() {
                    if let Ok(metadata) = entry.metadata() {
                        if metadata.is_dir() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if name != "Macintosh HD" {
                                // Skip default system volume
                                drives.push(SystemDrive {
                                    name: name.clone(),
                                    path: format!("/Volumes/{}", name),
                                    drive_type: "volume".to_string(),
                                    is_ejectable: true,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(drives)
}

#[command]
pub async fn eject_drive(path: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // On macOS, use diskutil to eject the volume
        let output = TokioCommand::new("diskutil")
            .arg("eject")
            .arg(&path)
            .output()
            .await
            .map_err(|e| format!("Failed to run diskutil: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            let error_msg = String::from_utf8_lossy(&output.stderr);
            Err(format!("Failed to eject drive: {}", error_msg))
        }
    }

    #[cfg(target_os = "linux")]
    {
        // On Linux, use umount command
        let output = TokioCommand::new("umount")
            .arg(&path)
            .output()
            .await
            .map_err(|e| format!("Failed to run umount: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            let error_msg = String::from_utf8_lossy(&output.stderr);
            Err(format!("Failed to eject drive: {}", error_msg))
        }
    }

    #[cfg(target_os = "windows")]
    {
        // On Windows, we would need to use Windows API for safe removal
        // For now, return an error as it requires more complex implementation
        Err("Drive ejection not yet implemented for Windows".to_string())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Err("Drive ejection not supported on this platform".to_string())
    }
}

// Global thumbnail service instance
static THUMBNAIL_SERVICE: OnceCell<Result<Arc<crate::thumbnails::ThumbnailService>, String>> =
    OnceCell::const_new();

async fn get_thumbnail_service() -> Result<Arc<crate::thumbnails::ThumbnailService>, String> {
    THUMBNAIL_SERVICE
        .get_or_init(|| async {
            crate::thumbnails::ThumbnailService::new()
                .await
                .map(Arc::new)
        })
        .await
        .clone()
}

#[tauri::command]
pub async fn request_thumbnail(
    path: String,
    size: Option<u32>,
    quality: Option<String>,
    priority: Option<String>,
    format: Option<String>,
    accent: Option<crate::thumbnails::AccentColor>,
) -> Result<crate::thumbnails::ThumbnailResponse, String> {
    let service = get_thumbnail_service().await?;

    let quality = match quality.as_deref() {
        Some("low") => crate::thumbnails::ThumbnailQuality::Low,
        Some("high") => crate::thumbnails::ThumbnailQuality::High,
        _ => crate::thumbnails::ThumbnailQuality::Medium,
    };

    let priority = match priority.as_deref() {
        Some("high") => crate::thumbnails::ThumbnailPriority::High,
        Some("low") => crate::thumbnails::ThumbnailPriority::Low,
        _ => crate::thumbnails::ThumbnailPriority::Medium,
    };

    let format = match format.as_deref() {
        Some("jpeg") => crate::thumbnails::ThumbnailFormat::JPEG,
        Some("png") => crate::thumbnails::ThumbnailFormat::PNG,
        _ => crate::thumbnails::ThumbnailFormat::WebP,
    };

    let request = crate::thumbnails::ThumbnailRequest {
        id: crate::thumbnails::generate_request_id(),
        path,
        size: size.unwrap_or(128),
        quality,
        priority,
        format,
        accent,
    };

    service.request_thumbnail(request).await
}

#[tauri::command]
pub async fn initialize_thumbnail_service() -> bool {
    if let Some(existing) = THUMBNAIL_SERVICE.get() {
        return match existing {
            Ok(_) => {
                info!("Thumbnail service prewarm skipped; already running");
                true
            }
            Err(err) => {
                warn!("Thumbnail service prewarm previously failed: {err}");
                false
            }
        };
    }

    let start = Instant::now();
    match get_thumbnail_service().await {
        Ok(_) => {
            let elapsed_ms = start.elapsed().as_millis();
            info!("Thumbnail service prewarmed in {elapsed_ms}ms");
            true
        }
        Err(err) => {
            warn!("Thumbnail service prewarm failed: {err}");
            false
        }
    }
}

#[tauri::command]
pub async fn cancel_thumbnail(request_id: String) -> Result<bool, String> {
    let service = get_thumbnail_service().await?;
    Ok(service.cancel_request(&request_id).await)
}

#[tauri::command]
pub async fn get_thumbnail_cache_stats() -> Result<crate::thumbnails::cache::CacheStats, String> {
    let service = get_thumbnail_service().await?;
    Ok(service.get_cache_stats().await)
}

#[tauri::command]
pub async fn clear_thumbnail_cache() -> Result<(), String> {
    let service = get_thumbnail_service().await?;
    service.clear_cache().await
}

#[command]
pub fn open_path(path: String) -> Result<(), String> {
    // Normalize path (~ expansion is already handled on the frontend for navigation)
    let path_str = path;

    #[cfg(target_os = "macos")]
    {
        let status = OsCommand::new("open")
            .arg(&path_str)
            .status()
            .map_err(|e| format!("Failed to spawn 'open': {}", e))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("'open' exited with status: {}", status))
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Use cmd start to let the shell decide the default handler
        // start requires a window title arg (empty string)
        let status = OsCommand::new("cmd")
            .args(["/C", "start", "", &path_str])
            .status()
            .map_err(|e| format!("Failed to spawn 'start': {}", e))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("'start' exited with status: {}", status))
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Try xdg-open first, then gio as a fallback
        let try_xdg = OsCommand::new("xdg-open").arg(&path_str).status();
        match try_xdg {
            Ok(status) if status.success() => Ok(()),
            _ => {
                let status = OsCommand::new("gio")
                    .args(["open", &path_str])
                    .status()
                    .map_err(|e| format!("Failed to spawn 'gio open': {}", e))?;
                if status.success() {
                    Ok(())
                } else {
                    Err(format!("'gio open' exited with status: {}", status))
                }
            }
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Err("open_path is not supported on this platform".to_string())
    }
}

#[command]
pub fn open_path_with(path: String, application_path: String) -> Result<(), String> {
    let expanded_path = expand_path(&path)?;
    let expanded_application = expand_path(&application_path)?;

    let file_path = PathBuf::from(&expanded_path);
    if !file_path.exists() {
        return Err("Target path does not exist".to_string());
    }

    let app_path = PathBuf::from(&expanded_application);
    if !app_path.exists() {
        return Err("Selected application path does not exist".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        let status = OsCommand::new("open")
            .arg("-a")
            .arg(&expanded_application)
            .arg(&expanded_path)
            .status()
            .map_err(|e| format!("Failed to spawn 'open -a': {}", e))?;
        return if status.success() {
            Ok(())
        } else {
            Err(format!("'open -a' exited with status: {}", status))
        };
    }

    #[cfg(target_os = "windows")]
    {
        return OsCommand::new(&expanded_application)
            .arg(&expanded_path)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Failed to launch application: {}", e));
    }

    #[cfg(target_os = "linux")]
    {
        return OsCommand::new(&expanded_application)
            .arg(&expanded_path)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Failed to launch application: {}", e));
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Err("open_path_with is not supported on this platform".to_string())
    }
}

#[command]
pub fn new_window(app: AppHandle, path: Option<String>) -> Result<(), String> {
    let window_label = format!(
        "window-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );

    let mut url = tauri::WebviewUrl::App("index.html".into());

    // If a path is provided, pass it as a query parameter
    if let Some(initial_path) = path {
        let encoded_path = urlencoding::encode(&initial_path);
        url = tauri::WebviewUrl::App(format!("index.html?path={}", encoded_path).into());
    }

    let builder = tauri::WebviewWindowBuilder::new(&app, &window_label, url)
        .title("")
        .inner_size(1200.0, 800.0)
        .resizable(true)
        .fullscreen(false)
        .decorations(true);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(18.0, 18.0));

    let _window = builder
        .build()
        .map_err(|e| format!("Failed to create window: {}", e))?;

    Ok(())
}

#[command]
pub fn open_folder_size_window(
    app: AppHandle,
    targets: Vec<FolderSizeTargetPayload>,
) -> Result<(), String> {
    if targets.is_empty() {
        return Err("At least one directory must be selected".to_string());
    }

    if !targets.iter().any(|target| target.is_directory) {
        return Err("Folder size requires at least one directory".to_string());
    }

    let mut unique = HashSet::new();
    let mut deduped: Vec<FolderSizeTargetPayload> = Vec::new();
    let mut path_args: Vec<String> = Vec::new();

    for target in targets {
        if unique.insert(target.path.clone()) {
            path_args.push(target.path.clone());
            deduped.push(target);
        }
    }

    if deduped.is_empty() {
        return Err("At least one directory must be selected".to_string());
    }

    let request_id = Uuid::new_v4().to_string();

    let url = tauri::WebviewUrl::App("index.html?view=folder-size".into());

    let payload = FolderSizeInitPayload {
        request_id: request_id.clone(),
        targets: deduped,
        auto_start: true,
        initial_error: None,
    };

    if let Some(existing) = app.get_webview_window(FOLDER_SIZE_WINDOW_LABEL) {
        FOLDER_SIZE_QUEUE.queue(&app, payload.clone());
        schedule_folder_size_auto_start(&app, request_id, path_args);
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }

    let builder = tauri::WebviewWindowBuilder::new(&app, FOLDER_SIZE_WINDOW_LABEL, url)
        .title("Folder Size")
        .inner_size(420.0, 480.0)
        .resizable(false)
        .fullscreen(false)
        .minimizable(false)
        .maximizable(false)
        .closable(true)
        .decorations(true);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(18.0, 18.0));

    FOLDER_SIZE_QUEUE.set_ready(false);

    let _window = builder
        .build()
        .map_err(|e| format!("Failed to create folder size window: {}", e))?;

    FOLDER_SIZE_QUEUE.queue(&app, payload);
    schedule_folder_size_auto_start(&app, request_id, path_args);

    Ok(())
}

#[command]
pub fn folder_size_window_ready(app: AppHandle) -> Result<(), String> {
    FOLDER_SIZE_QUEUE.set_ready(true);
    FOLDER_SIZE_QUEUE.try_emit(&app);
    Ok(())
}

#[command]
pub fn folder_size_window_unready() -> Result<(), String> {
    FOLDER_SIZE_QUEUE.set_ready(false);
    Ok(())
}

#[command]
pub fn show_archive_progress_window(
    app: AppHandle,
    file_name: String,
    destination_dir: String,
    format: Option<String>,
) -> Result<(), String> {
    let payload = ArchiveProgressPayload {
        file_name,
        destination_dir,
        format: format.unwrap_or_else(|| "archive".to_string()),
    };

    if let Some(existing) = app.get_webview_window(ARCHIVE_PROGRESS_WINDOW_LABEL) {
        ARCHIVE_PROGRESS_QUEUE.queue(&app, payload);
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }

    let url = tauri::WebviewUrl::App("index.html?view=archive-progress".into());
    let builder = tauri::WebviewWindowBuilder::new(&app, ARCHIVE_PROGRESS_WINDOW_LABEL, url)
        .title("Extracting Archive")
        .inner_size(420.0, 480.0)
        .resizable(false)
        .fullscreen(false)
        .minimizable(true)
        .maximizable(false)
        .closable(true)
        .decorations(true);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(14.0, 14.0));

    ARCHIVE_PROGRESS_QUEUE.set_ready(false);

    builder
        .build()
        .map_err(|e| format!("Failed to create archive progress window: {}", e))?;

    ARCHIVE_PROGRESS_QUEUE.queue(&app, payload);
    Ok(())
}

#[command]
pub fn hide_archive_progress_window(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(ARCHIVE_PROGRESS_WINDOW_LABEL) {
        if let Err(err) = window.close() {
            warn!("Failed to close archive progress window: {err}");
        }
    }

    ARCHIVE_PROGRESS_QUEUE.set_ready(false);
    ARCHIVE_PROGRESS_QUEUE.clear_pending();
    Ok(())
}

#[command]
pub fn archive_progress_window_ready(app: AppHandle) -> Result<(), String> {
    ARCHIVE_PROGRESS_QUEUE.set_ready(true);
    ARCHIVE_PROGRESS_QUEUE.try_emit(&app);
    Ok(())
}

#[command]
pub fn archive_progress_window_unready() -> Result<(), String> {
    ARCHIVE_PROGRESS_QUEUE.set_ready(false);
    Ok(())
}

#[command]
pub fn show_delete_progress_window(
    app: AppHandle,
    request_id: String,
    items: Vec<DeleteItemPayload>,
) -> Result<(), String> {
    let total_items = items.len();
    let payload = DeleteProgressPayload {
        request_id,
        total_items,
        items,
    };

    if let Some(existing) = app.get_webview_window(DELETE_PROGRESS_WINDOW_LABEL) {
        DELETE_PROGRESS_QUEUE.queue(&app, payload);
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }

    let url = tauri::WebviewUrl::App("index.html?view=delete-progress".into());
    let builder = tauri::WebviewWindowBuilder::new(&app, DELETE_PROGRESS_WINDOW_LABEL, url)
        .title("Deleting Items")
        .inner_size(420.0, 420.0)
        .resizable(false)
        .fullscreen(false)
        .minimizable(true)
        .maximizable(false)
        .closable(true)
        .decorations(true);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(14.0, 14.0));

    DELETE_PROGRESS_QUEUE.set_ready(false);

    builder
        .build()
        .map_err(|e| format!("Failed to create delete progress window: {}", e))?;

    DELETE_PROGRESS_QUEUE.queue(&app, payload);
    Ok(())
}

fn configure_modal_utility_window<'a, R: tauri::Runtime, M: tauri::Manager<R>>(
    builder: tauri::WebviewWindowBuilder<'a, R, M>,
) -> tauri::WebviewWindowBuilder<'a, R, M> {
    builder
        .resizable(false)
        .fullscreen(false)
        .minimizable(false)
        .maximizable(false)
        .closable(true)
        .decorations(true)
}

#[command]
pub fn open_smb_connect_window(
    app: AppHandle,
    initial_hostname: Option<String>,
    target_path: Option<String>,
) -> Result<(), String> {
    let payload = SmbConnectInitPayload {
        initial_hostname,
        target_path,
    };

    if let Some(existing) = app.get_webview_window(SMB_CONNECT_WINDOW_LABEL) {
        SMB_CONNECT_QUEUE.queue(&app, payload);
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }

    let url = tauri::WebviewUrl::App("index.html?view=smb-connect".into());
    let builder = configure_modal_utility_window(tauri::WebviewWindowBuilder::new(
        &app,
        SMB_CONNECT_WINDOW_LABEL,
        url,
    ))
        .title("Add Network Share")
        .inner_size(460.0, 520.0);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(14.0, 14.0));

    SMB_CONNECT_QUEUE.set_ready(false);

    builder
        .build()
        .map_err(|e| format!("Failed to create SMB connect window: {}", e))?;

    SMB_CONNECT_QUEUE.queue(&app, payload);
    Ok(())
}

#[command]
pub fn hide_smb_connect_window(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(SMB_CONNECT_WINDOW_LABEL) {
        if let Err(err) = window.close() {
            warn!("Failed to close SMB connect window: {err}");
        }
    }

    SMB_CONNECT_QUEUE.set_ready(false);
    SMB_CONNECT_QUEUE.clear_pending();
    Ok(())
}

#[command]
pub fn smb_connect_window_ready(app: AppHandle) -> Result<(), String> {
    SMB_CONNECT_QUEUE.set_ready(true);
    SMB_CONNECT_QUEUE.try_emit(&app);
    Ok(())
}

#[command]
pub fn smb_connect_window_unready() -> Result<(), String> {
    SMB_CONNECT_QUEUE.set_ready(false);
    Ok(())
}

#[command]
pub fn open_permissions_window(app: AppHandle) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window(PERMISSIONS_WINDOW_LABEL) {
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }

    let url = tauri::WebviewUrl::App("index.html?view=permissions".into());
    let builder = configure_modal_utility_window(tauri::WebviewWindowBuilder::new(
        &app,
        PERMISSIONS_WINDOW_LABEL,
        url,
    ))
        .title("Full Disk Access")
        .inner_size(520.0, 560.0);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(14.0, 14.0));

    builder
        .build()
        .map_err(|e| format!("Failed to create permissions window: {}", e))?;

    Ok(())
}

#[command]
pub fn hide_delete_progress_window(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(DELETE_PROGRESS_WINDOW_LABEL) {
        if let Err(err) = window.close() {
            warn!("Failed to close delete progress window: {err}");
        }
    }

    DELETE_PROGRESS_QUEUE.set_ready(false);
    DELETE_PROGRESS_QUEUE.clear_pending();
    Ok(())
}

#[command]
pub fn delete_progress_window_ready(app: AppHandle) -> Result<(), String> {
    DELETE_PROGRESS_QUEUE.set_ready(true);
    DELETE_PROGRESS_QUEUE.try_emit(&app);
    Ok(())
}

#[command]
pub fn delete_progress_window_unready() -> Result<(), String> {
    DELETE_PROGRESS_QUEUE.set_ready(false);
    Ok(())
}

#[tauri::command]
pub fn show_native_context_menu(
    app: AppHandle,
    window_label: Option<String>,
    x: f64,
    y: f64,
    sort_by: Option<String>,
    sort_order: Option<String>,
    path: Option<String>,
    has_file_context: Option<bool>,
    file_paths: Option<Vec<String>>,
    selected_is_symlink: Option<bool>,
    selection_has_directory: Option<bool>,
) -> Result<(), String> {
    // Resolve window
    let webview = if let Some(label) = window_label {
        app.get_webview_window(&label)
            .ok_or_else(|| "Window not found".to_string())?
    } else {
        app.get_webview_window("main")
            .ok_or_else(|| "Main window not found".to_string())?
    };

    // Build a native menu mirroring our React context menu
    use tauri::menu::{CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder, SubmenuBuilder};

    let state: tauri::State<crate::state::MenuState<tauri::Wry>> = app.state();
    let show_hidden_checked = state
        .show_hidden_checked
        .lock()
        .map_err(|e| e.to_string())
        .map(|v| *v)
        .unwrap_or(false);
    let folders_first_checked = state
        .folders_first_checked
        .lock()
        .map_err(|e| e.to_string())
        .map(|v| *v)
        .unwrap_or(true);

    let show_hidden_item = CheckMenuItemBuilder::with_id("ctx:toggle_hidden", "Show Hidden Files")
        .checked(show_hidden_checked)
        .build(&app)
        .map_err(|e| e.to_string())?;

    // Determine effective sort_by from args or stored prefs
    let sort_by_val = if let Some(sb) = sort_by.clone() {
        sb
    } else if let Some(p) = path.clone() {
        // Try to read per-directory or global preferences for fallback
        let norm = normalize_path(p);
        if let Ok(v) = read_prefs_value() {
            if let Some(dir_prefs) = v.get("directoryPreferences").and_then(|d| d.get(&norm)) {
                if let Some(sb) = dir_prefs.get("sortBy").and_then(|x| x.as_str()) {
                    sb.to_string()
                } else {
                    v.get("globalPreferences")
                        .and_then(|g| g.get("sortBy"))
                        .and_then(|x| x.as_str())
                        .unwrap_or("name")
                        .to_string()
                }
            } else {
                v.get("globalPreferences")
                    .and_then(|g| g.get("sortBy"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("name")
                    .to_string()
            }
        } else {
            "name".to_string()
        }
    } else {
        "name".to_string()
    };
    let sort_name = CheckMenuItemBuilder::with_id("ctx:sort_name", "Name")
        .checked(matches!(sort_by_val.as_str(), "name"))
        .build(&app)
        .map_err(|e| e.to_string())?;
    let sort_size = CheckMenuItemBuilder::with_id("ctx:sort_size", "Size")
        .checked(matches!(sort_by_val.as_str(), "size"))
        .build(&app)
        .map_err(|e| e.to_string())?;
    let sort_type = CheckMenuItemBuilder::with_id("ctx:sort_type", "Type")
        .checked(matches!(sort_by_val.as_str(), "type"))
        .build(&app)
        .map_err(|e| e.to_string())?;
    let sort_modified = CheckMenuItemBuilder::with_id("ctx:sort_modified", "Date Modified")
        .checked(matches!(sort_by_val.as_str(), "modified"))
        .build(&app)
        .map_err(|e| e.to_string())?;

    // Determine current sort order from arg or fallback to stored state
    let sort_order_asc_checked = if let Some(ord) = sort_order.as_deref() {
        ord.eq_ignore_ascii_case("asc")
    } else if let Some(p) = path.clone() {
        if let Ok(v) = read_prefs_value() {
            let norm = normalize_path(p);
            let so = v
                .get("directoryPreferences")
                .and_then(|d| d.get(&norm))
                .and_then(|dp| dp.get("sortOrder"))
                .and_then(|x| x.as_str())
                .or_else(|| {
                    v.get("globalPreferences")
                        .and_then(|g| g.get("sortOrder"))
                        .and_then(|x| x.as_str())
                });
            match so {
                Some("asc") => true,
                Some("desc") => false,
                _ => matches!(sort_by_val.as_str(), "name" | "type"),
            }
        } else {
            matches!(sort_by_val.as_str(), "name" | "type")
        }
    } else {
        state
            .sort_order_asc_checked
            .lock()
            .map_err(|e| e.to_string())
            .map(|v| *v)
            .unwrap_or(true)
    };
    // Keep backend state roughly in sync if parameter provided
    if sort_order.is_some() {
        if let Ok(mut asc) = state.sort_order_asc_checked.lock() {
            *asc = sort_order_asc_checked;
        }
    }

    let sort_order_asc = CheckMenuItemBuilder::with_id("ctx:sort_order_asc", "Ascending")
        .checked(sort_order_asc_checked)
        .build(&app)
        .map_err(|e| e.to_string())?;
    let sort_order_desc = CheckMenuItemBuilder::with_id("ctx:sort_order_desc", "Descending")
        .checked(!sort_order_asc_checked)
        .build(&app)
        .map_err(|e| e.to_string())?;

    let folders_first_item = CheckMenuItemBuilder::with_id("ctx:folders_first", "Folders on Top")
        .checked(folders_first_checked)
        .build(&app)
        .map_err(|e| e.to_string())?;

    let sort_submenu = SubmenuBuilder::new(&app, "Sort by")
        .item(&sort_name)
        .item(&sort_size)
        .item(&sort_type)
        .item(&sort_modified)
        .separator()
        .item(&sort_order_asc)
        .item(&sort_order_desc)
        .separator()
        .item(&folders_first_item)
        .build()
        .map_err(|e| e.to_string())?;

    // Only include file-specific actions when the right-click is on a file
    // (or explicit file paths are provided by the frontend).
    let selection_len = file_paths.as_ref().map(|v| v.len()).unwrap_or(0);
    let is_file_ctx = has_file_context.unwrap_or(false) || selection_len > 0;
    let has_directory_selection = selection_has_directory.unwrap_or(false);

    let mut builder = MenuBuilder::new(&app);
    if is_file_ctx {
        let rename_item = MenuItemBuilder::with_id("ctx:rename", "Rename")
            .build(&app)
            .map_err(|e| e.to_string())?;
        let copy_name_item = MenuItemBuilder::with_id("ctx:copy_name", "Copy File Name")
            .build(&app)
            .map_err(|e| e.to_string())?;
        let copy_full_name_item = MenuItemBuilder::with_id("ctx:copy_full_name", "Copy Full Path")
            .build(&app)
            .map_err(|e| e.to_string())?;
        let calculate_size_item = if has_directory_selection {
            Some(
                MenuItemBuilder::with_id("ctx:calculate_total_size", "Calculate Total Size")
                    .build(&app)
                    .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };

        let reveal_item = if selected_is_symlink.unwrap_or(false) {
            Some(
                MenuItemBuilder::with_id("ctx:reveal_symlink", "Reveal Original Location")
                    .build(&app)
                    .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
        builder = builder
            .item(&rename_item)
            .item(&copy_name_item)
            .item(&copy_full_name_item);
        if let Some(ref item) = calculate_size_item {
            builder = builder.item(item);
        }
        if let Some(ref item) = reveal_item {
            builder = builder.item(item);
        }
        builder = builder.separator();
    }

    let ctx_menu = builder
        .items(&[&show_hidden_item, &sort_submenu])
        .build()
        .map_err(|e| e.to_string())?;

    // Popup at screen coordinates
    // Prefer using physical coordinates to avoid scaling issues.
    use tauri::{LogicalPosition, Position};

    // Some platforms may not support popup positioning; try without position if needed.
    webview
        .popup_menu_at(&ctx_menu, Position::Logical(LogicalPosition { x, y }))
        .map_err(|e| e.to_string())
}

#[command]
pub async fn calculate_folder_size(
    app: AppHandle,
    state: tauri::State<'_, FolderSizeState>,
    request_id: String,
    paths: Vec<String>,
) -> Result<(), String> {
    let trimmed_id = request_id.trim();
    if trimmed_id.is_empty() {
        return Err("request_id cannot be empty".to_string());
    }
    if paths.is_empty() {
        return Err("At least one path must be provided".to_string());
    }

    let mut unique = HashSet::new();
    let mut resolved: Vec<PathBuf> = Vec::new();
    for raw in paths {
        let expanded = expand_path(&raw)?;
        let candidate = PathBuf::from(&expanded);
        if !candidate.exists() {
            continue;
        }
        if unique.insert(candidate.clone()) {
            resolved.push(candidate);
        }
    }

    if resolved.is_empty() {
        return Err("None of the provided paths could be accessed".to_string());
    }

    let normalized_id = trimmed_id.to_string();
    let cancel_flag = Arc::new(AtomicBool::new(false));

    {
        let mut guard = state
            .tasks
            .lock()
            .map_err(|_| "Failed to access folder size state".to_string())?;
        if let Some(existing) = guard.insert(
            normalized_id.clone(),
            FolderSizeTaskHandle {
                cancel_flag: cancel_flag.clone(),
            },
        ) {
            existing.cancel_flag.store(true, Ordering::SeqCst);
        }
    }

    let app_for_task = app.clone();
    let request_key = normalized_id.clone();
    info!(
        "Starting folder size calculation task for request {}",
        request_key
    );
    tauri::async_runtime::spawn(async move {
        let paths_for_task = resolved;
        let cancel_for_task = cancel_flag;
        let app_for_compute = app_for_task.clone();
        let request_for_compute = request_key.clone();

        info!("Spawning blocking task for {} paths", paths_for_task.len());
        let join_result = tauri::async_runtime::spawn_blocking(move || {
            info!(
                "Starting walk_paths_for_size for request {}",
                request_for_compute
            );
            walk_paths_for_size(
                &app_for_compute,
                &request_for_compute,
                &paths_for_task,
                &cancel_for_task,
            )
        })
        .await;

        if let Err(join_err) = join_result {
            emit_folder_size_event(
                &app_for_task,
                &request_key,
                0,
                0,
                0,
                None,
                true,
                false,
                Some(format!("Folder size task failed: {join_err}")),
            );
        }

        {
            let folder_state = app_for_task.state::<FolderSizeState>();
            if let Ok(mut guard) = folder_state.tasks.lock() {
                guard.remove(&request_key);
            };
        }
    });

    Ok(())
}

#[command]
pub fn cancel_folder_size_calculation(
    _app: AppHandle,
    state: tauri::State<'_, FolderSizeState>,
    request_id: String,
) -> Result<(), String> {
    let trimmed_id = request_id.trim();
    if trimmed_id.is_empty() {
        return Err("request_id cannot be empty".to_string());
    }

    let handle = {
        let guard = state
            .tasks
            .lock()
            .map_err(|_| "Failed to access folder size state".to_string())?;
        guard.get(trimmed_id).cloned()
    };

    if let Some(task) = handle {
        task.cancel_flag.store(true, Ordering::SeqCst);
    }

    Ok(())
}

fn preferences_path() -> Result<PathBuf, String> {
    let base =
        dirs::config_dir().ok_or_else(|| "Could not resolve config directory".to_string())?;
    let app_dir = base.join("Marlin");
    if !app_dir.exists() {
        fs::create_dir_all(&app_dir).map_err(|e| format!("Failed to create config dir: {}", e))?;
    }
    Ok(app_dir.join("preferences.json"))
}

#[tauri::command]
pub fn read_preferences() -> Result<String, String> {
    let path = preferences_path()?;
    if !path.exists() {
        return Ok("{}".to_string());
    }
    let mut file =
        fs::File::open(&path).map_err(|e| format!("Failed to open preferences: {}", e))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|e| format!("Failed to read preferences: {}", e))?;
    Ok(contents)
}

#[tauri::command]
pub fn write_preferences(json: String) -> Result<(), String> {
    let path = preferences_path()?;
    let mut file =
        fs::File::create(&path).map_err(|e| format!("Failed to create preferences: {}", e))?;
    file.write_all(json.as_bytes())
        .map_err(|e| format!("Failed to write preferences: {}", e))?;
    Ok(())
}

fn normalize_path(s: String) -> String {
    if s.is_empty() {
        return "/".to_string();
    }

    // For URI schemes (e.g., gdrive://user@email/path), don't apply file path normalization
    // URI schemes contain "://" and should be kept as-is
    if s.contains("://") {
        // This is a URI - return as-is without path normalization
        return s;
    }

    // Regular file path normalization
    let mut result = s.replace('\\', "/");
    while result.contains("//") {
        result = result.replace("//", "/");
    }
    if result.len() > 1 && result.ends_with('/') {
        result.pop();
    }
    if result.len() == 2 && result.chars().nth(1) == Some(':') {
        result.push('/');
    }
    if result.is_empty() {
        result = "/".into();
    }
    result
}

fn read_prefs_value() -> Result<Value, String> {
    let path = preferences_path()?;
    if !path.exists() {
        return Ok(json!({}));
    }
    let mut file =
        fs::File::open(&path).map_err(|e| format!("Failed to open preferences: {}", e))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|e| format!("Failed to read preferences: {}", e))?;
    let v: Value = serde_json::from_str(&contents).unwrap_or_else(|_| json!({}));
    Ok(v)
}

fn write_prefs_value(v: &Value) -> Result<(), String> {
    let path = preferences_path()?;
    let mut file =
        fs::File::create(&path).map_err(|e| format!("Failed to create preferences: {}", e))?;
    let s = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    file.write_all(s.as_bytes())
        .map_err(|e| format!("Failed to write preferences: {}", e))?;
    Ok(())
}

#[tauri::command]
pub fn get_dir_prefs(path: String) -> Result<String, String> {
    let norm = normalize_path(path);
    let v = read_prefs_value()?;
    let dirs = v
        .get("directoryPreferences")
        .and_then(|d| d.as_object())
        .cloned()
        .unwrap_or_default();
    let out = dirs.get(&norm).cloned().unwrap_or(json!({}));
    Ok(out.to_string())
}

#[tauri::command]
pub fn set_dir_prefs(path: String, prefs: String) -> Result<(), String> {
    let norm = normalize_path(path);
    let mut v = read_prefs_value()?;
    let dirs = v
        .get("directoryPreferences")
        .and_then(|d| d.as_object())
        .cloned()
        .unwrap_or_default();
    let incoming: Value =
        serde_json::from_str(&prefs).map_err(|e| format!("Invalid prefs JSON: {}", e))?;
    let mut merged = dirs.get(&norm).cloned().unwrap_or(json!({}));
    if let (Some(obj_in), Some(obj_existing)) = (incoming.as_object(), merged.as_object_mut()) {
        for (k, val) in obj_in.iter() {
            obj_existing.insert(k.clone(), val.clone());
        }
    } else {
        merged = incoming;
    }
    let mut new_dirs = serde_json::Map::from_iter(dirs.into_iter());
    new_dirs.insert(norm, merged);
    v["directoryPreferences"] = Value::Object(new_dirs);
    write_prefs_value(&v)
}

#[tauri::command]
pub fn clear_all_dir_prefs() -> Result<(), String> {
    let mut v = read_prefs_value()?;
    v["directoryPreferences"] = json!({});
    write_prefs_value(&v)
}

#[tauri::command]
pub fn set_last_dir(path: String) -> Result<(), String> {
    let mut v = read_prefs_value()?;
    v["lastDir"] = Value::String(normalize_path(path));
    write_prefs_value(&v)
}

#[tauri::command]
pub fn toggle_menu_visibility(app: AppHandle) -> Result<bool, String> {
    #[cfg(target_os = "linux")]
    {
        let window = app
            .get_webview_window("main")
            .ok_or_else(|| "Main window not found".to_string())?;
        let is_visible = window.is_menu_visible().map_err(|e| e.to_string())?;
        if is_visible {
            window.hide_menu().map_err(|e| e.to_string())?;
        } else {
            window.show_menu().map_err(|e| e.to_string())?;
        }
        Ok(!is_visible)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = app;
        Ok(true)
    }
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub fn start_native_drag(
    paths: Vec<String>,
    preview_image: Option<String>,
    drag_offset_y: Option<f64>,
) -> Result<(), String> {
    crate::native_drag::start_native_drag(paths, preview_image, drag_offset_y)
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn start_native_drag(
    _paths: Vec<String>,
    _preview_image: Option<String>,
    _drag_offset_y: Option<f64>,
) -> Result<(), String> {
    Err("Native drag is only supported on macOS".to_string())
}

// File system watcher commands
#[command]
pub fn start_watching_directory(path: String) -> Result<(), String> {
    if let Some(watcher) = fs_watcher::get_watcher() {
        watcher.start_watching(&path)
    } else {
        Err("File system watcher not initialized".to_string())
    }
}

#[command]
pub fn stop_watching_directory(path: String) -> Result<(), String> {
    if let Some(watcher) = fs_watcher::get_watcher() {
        watcher.stop_watching(&path)
    } else {
        Err("File system watcher not initialized".to_string())
    }
}

#[command]
pub fn stop_all_watchers() -> Result<(), String> {
    if let Some(watcher) = fs_watcher::get_watcher() {
        watcher.stop_all_watchers();
        Ok(())
    } else {
        Err("File system watcher not initialized".to_string())
    }
}

#[command]
pub fn is_watching_directory(path: String) -> Result<bool, String> {
    if let Some(watcher) = fs_watcher::get_watcher() {
        Ok(watcher.is_watching(&path))
    } else {
        Ok(false)
    }
}

#[command]
pub fn get_watched_directories() -> Result<Vec<String>, String> {
    if let Some(watcher) = fs_watcher::get_watcher() {
        Ok(watcher.get_watched_paths())
    } else {
        Ok(vec![])
    }
}

/// Internal representation stored in JSON (minimal fields for persistence)
#[derive(Serialize, Deserialize, Clone)]
struct StoredPinnedDirectory {
    pub name: String,
    pub path: String,
    pub pinned_at: DateTime<Utc>,
}

/// Public representation with computed metadata (returned to frontend)
#[derive(Serialize, Clone)]
pub struct PinnedDirectory {
    pub name: String,
    pub path: String,
    pub pinned_at: DateTime<Utc>,
    pub is_git_repo: bool,
    pub is_symlink: bool,
}

fn pinned_directories_path() -> Result<PathBuf, String> {
    let base =
        dirs::config_dir().ok_or_else(|| "Could not resolve config directory".to_string())?;
    let app_dir = base.join("Marlin");
    if !app_dir.exists() {
        fs::create_dir_all(&app_dir).map_err(|e| format!("Failed to create config dir: {}", e))?;
    }
    Ok(app_dir.join("pinned_directories.json"))
}

/// Load stored pinned directories from JSON (internal use)
fn load_stored_pinned_directories() -> Result<Vec<StoredPinnedDirectory>, String> {
    let path = pinned_directories_path()?;
    if !path.exists() {
        return Ok(vec![]);
    }

    let mut file =
        fs::File::open(&path).map_err(|e| format!("Failed to open pinned directories: {}", e))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|e| format!("Failed to read pinned directories: {}", e))?;

    let pinned_dirs: Vec<StoredPinnedDirectory> = serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse pinned directories: {}", e))?;

    Ok(pinned_dirs)
}

/// Compute git repo and symlink status for a path
fn compute_pin_metadata(path: &Path) -> (bool, bool) {
    // Check if symlink using symlink_metadata (doesn't follow symlinks)
    let is_symlink = fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);

    // Check if git repo (has .git dir or file)
    let is_git_repo = {
        let git_path = path.join(".git");
        git_path.is_dir() || git_path.is_file()
    };

    (is_git_repo, is_symlink)
}

#[command]
pub fn get_pinned_directories() -> Result<Vec<PinnedDirectory>, String> {
    let stored = load_stored_pinned_directories()?;

    // Enrich with computed metadata
    let enriched: Vec<PinnedDirectory> = stored
        .into_iter()
        .map(|stored_pin| {
            let path = Path::new(&stored_pin.path);
            let (is_git_repo, is_symlink) = compute_pin_metadata(path);

            PinnedDirectory {
                name: stored_pin.name,
                path: stored_pin.path,
                pinned_at: stored_pin.pinned_at,
                is_git_repo,
                is_symlink,
            }
        })
        .collect();

    Ok(enriched)
}

#[command]
pub fn add_pinned_directory(path: String, name: Option<String>) -> Result<PinnedDirectory, String> {
    let expanded_path = expand_path(&path)?;
    let path_obj = &expanded_path;

    if !path_obj.exists() {
        return Err("Path does not exist".to_string());
    }

    if !path_obj.is_dir() {
        return Err("Path is not a directory".to_string());
    }

    let expanded_path_str = expanded_path.to_string_lossy().to_string();
    let mut stored_pins = load_stored_pinned_directories()?;

    // Check if already pinned
    if stored_pins.iter().any(|p| p.path == expanded_path_str) {
        return Err("Directory is already pinned".to_string());
    }

    // Limit to 20 pinned directories
    if stored_pins.len() >= 20 {
        return Err("Maximum number of pinned directories reached (20)".to_string());
    }

    let dir_name = name.unwrap_or_else(|| {
        path_obj
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unknown")
            .to_string()
    });

    let new_stored = StoredPinnedDirectory {
        name: dir_name.clone(),
        path: expanded_path_str.clone(),
        pinned_at: Utc::now(),
    };

    stored_pins.push(new_stored.clone());
    save_pinned_directories(&stored_pins)?;

    // Compute metadata for the response
    let (is_git_repo, is_symlink) = compute_pin_metadata(path_obj);

    Ok(PinnedDirectory {
        name: dir_name,
        path: expanded_path_str,
        pinned_at: new_stored.pinned_at,
        is_git_repo,
        is_symlink,
    })
}

#[command]
pub fn remove_pinned_directory(path: String) -> Result<bool, String> {
    let expanded_path = expand_path(&path)?;
    let expanded_path_str = expanded_path.to_string_lossy().to_string();
    let mut stored_pins = load_stored_pinned_directories()?;

    let initial_len = stored_pins.len();
    stored_pins.retain(|p| p.path != expanded_path_str);

    if stored_pins.len() < initial_len {
        save_pinned_directories(&stored_pins)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

#[command]
pub fn reorder_pinned_directories(paths: Vec<String>) -> Result<(), String> {
    let current_pins = load_stored_pinned_directories()?;
    let mut reordered = Vec::new();

    // Reorder based on the provided paths list
    for path in paths {
        if let Some(pin) = current_pins.iter().find(|p| p.path == path) {
            reordered.push(pin.clone());
        }
    }

    // Add any missing pins that weren't in the reorder list
    for pin in &current_pins {
        if !reordered.iter().any(|p| p.path == pin.path) {
            reordered.push(pin.clone());
        }
    }

    save_pinned_directories(&reordered)
}

fn save_pinned_directories(pinned_dirs: &[StoredPinnedDirectory]) -> Result<(), String> {
    let path = pinned_directories_path()?;
    let json = serde_json::to_string_pretty(pinned_dirs)
        .map_err(|e| format!("Failed to serialize pinned directories: {}", e))?;

    let mut file = fs::File::create(&path)
        .map_err(|e| format!("Failed to create pinned directories file: {}", e))?;

    file.write_all(json.as_bytes())
        .map_err(|e| format!("Failed to write pinned directories: {}", e))?;

    Ok(())
}

// ============================================================================
// Google Drive Integration Commands
// ============================================================================

/// Get all connected Google accounts
#[command]
pub fn get_google_accounts() -> Result<Vec<GoogleAccountInfo>, String> {
    get_gdrive_accounts()
}

/// Add a new Google account via OAuth flow
#[command]
pub async fn add_google_account() -> Result<GoogleAccountInfo, String> {
    add_gdrive_account().await
}

/// Remove a Google account
#[command]
pub fn remove_google_account(email: String) -> Result<(), String> {
    remove_gdrive_account(&email)
}

/// Result of resolving a Google Drive URL
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveGoogleDriveUrlResult {
    pub email: String,
    pub path: String,
    pub is_folder: bool,
}

/// Resolve a Google Drive URL to a gdrive:// path
/// Tries all connected accounts until one works
#[command]
pub async fn resolve_google_drive_url(url: String) -> Result<ResolveGoogleDriveUrlResult, String> {
    // Check if it's a Google Drive URL
    if !is_google_drive_url(&url) {
        return Err("Not a Google Drive URL".to_string());
    }

    // Parse the URL to get the file/folder ID
    let url_info = parse_google_drive_url(&url)
        .ok_or_else(|| "Could not parse Google Drive URL".to_string())?;

    // Try to resolve the file ID to a path using connected accounts
    let (email, path) = resolve_file_id_to_path(&url_info.id).await?;

    Ok(ResolveGoogleDriveUrlResult {
        email,
        path,
        is_folder: url_info.is_folder,
    })
}

/// Download a Google Drive file to a temporary location and open it
/// Returns the temporary file path
#[command]
pub async fn download_gdrive_file(email: String, file_id: String, file_name: String) -> Result<String, String> {
    download_file_to_temp(&email, &file_id, &file_name).await
}

/// Fetch a URL with Google Drive authentication and return as data URL
/// This is used for thumbnail URLs that require authentication
#[command]
pub async fn fetch_gdrive_url(email: String, url: String) -> Result<String, String> {
    fetch_url_with_auth(&email, &url).await
}

/// Get the user's Downloads directory path
#[command]
pub fn get_downloads_dir() -> Result<String, String> {
    dirs::download_dir()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "Could not determine Downloads directory".to_string())
}

/// Extract a zip file from Google Drive and upload contents back to Google Drive
/// Returns the folder ID of the created folder
#[command]
pub async fn extract_gdrive_archive(
    email: String,
    file_id: String,
    file_name: String,
    destination_folder_id: String,
) -> Result<String, String> {
    extract_gdrive_zip(&email, &file_id, &file_name, &destination_folder_id).await
}

/// Get the folder ID for a Google Drive path
#[command]
pub async fn get_gdrive_folder_id(email: String, path: String) -> Result<String, String> {
    get_folder_id_by_path(&email, &path).await
}

/// Resolve a Google Drive folder ID to a navigable path
/// Tries each account in order until one succeeds
/// Returns (email, path, folder_name) as a tuple
#[command]
pub async fn resolve_gdrive_folder_url(
    folder_id: String,
    accounts: Vec<String>,
) -> Result<(String, String, String), String> {
    resolve_folder_id(&accounts, &folder_id).await
}

// SMB Network Share Commands (macOS/Linux only)
// ============================================================================

/// Get all connected SMB servers
#[cfg(not(target_os = "windows"))]
#[command]
pub fn get_smb_servers() -> Result<Vec<crate::locations::smb::SmbServerInfo>, String> {
    crate::locations::smb::get_smb_servers()
}

/// Get all connected SMB servers (Windows stub - SMB uses native UNC paths)
#[cfg(target_os = "windows")]
#[command]
pub fn get_smb_servers() -> Result<Vec<()>, String> {
    Ok(vec![])
}

/// Add a new SMB server
#[cfg(not(target_os = "windows"))]
#[command]
pub fn add_smb_server(
    hostname: String,
    username: String,
    password: String,
    domain: Option<String>,
) -> Result<crate::locations::smb::SmbServerInfo, String> {
    crate::locations::smb::add_smb_server(hostname, username, password, domain)
}

/// Add a new SMB server (Windows stub)
#[cfg(target_os = "windows")]
#[command]
pub fn add_smb_server(
    _hostname: String,
    _username: String,
    _password: String,
    _domain: Option<String>,
) -> Result<(), String> {
    Err("SMB on Windows uses native UNC paths. Navigate to \\\\server\\share directly.".to_string())
}

/// Remove an SMB server
#[cfg(not(target_os = "windows"))]
#[command]
pub fn remove_smb_server(hostname: String) -> Result<(), String> {
    crate::locations::smb::remove_smb_server(&hostname)
}

/// Remove an SMB server (Windows stub)
#[cfg(target_os = "windows")]
#[command]
pub fn remove_smb_server(_hostname: String) -> Result<(), String> {
    Ok(())
}

/// Test connection to an SMB server
#[cfg(not(target_os = "windows"))]
#[command]
pub fn test_smb_connection(
    hostname: String,
    username: String,
    password: String,
    domain: Option<String>,
) -> Result<bool, String> {
    crate::locations::smb::test_smb_connection(&hostname, &username, &password, domain.as_deref())
}

/// Test connection to an SMB server (Windows stub)
#[cfg(target_os = "windows")]
#[command]
pub fn test_smb_connection(
    _hostname: String,
    _username: String,
    _password: String,
    _domain: Option<String>,
) -> Result<bool, String> {
    Ok(true)
}

/// Download an SMB file to a temporary location (for drag-out/open-in-external-app).
/// Returns the temporary file path.
#[cfg(all(not(target_os = "windows"), feature = "smb"))]
#[command]
pub async fn download_smb_file(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::thumbnails::generators::smb::download_smb_file_sync(&path)
            .map(|p| p.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Download an SMB file to a temporary location (Windows / SMB-disabled stub).
#[cfg(any(target_os = "windows", not(feature = "smb")))]
#[command]
pub async fn download_smb_file(_path: String) -> Result<String, String> {
    Err("SMB download not supported in this build".to_string())
}
