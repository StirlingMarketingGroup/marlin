use async_trait::async_trait;
use std::path::{Path, PathBuf};

use tauri::async_runtime::spawn_blocking;
use url::Url;
use urlencoding::encode;

use crate::fs_utils::{expand_path, is_hidden_file, FileItem};
use crate::locations::{
    Location, LocationCapabilities, LocationProvider, LocationSummary, ProviderDirectoryEntries,
};
use crate::locations::gdrive::provider::{download_file_to_temp, get_file_id_by_path};
use crate::locations::LocationInput;

#[cfg(not(target_os = "windows"))]
use crate::thumbnails::generators::smb::download_smb_file_sync;

mod cache;
mod reader;

use reader::{normalize_internal_path, ArchiveEntry};

#[derive(Debug, Clone)]
struct ArchiveLocation {
    src: String,
    path: String,
}

fn parse_archive_uri(raw: &str) -> Result<ArchiveLocation, String> {
    let url = Url::parse(raw).map_err(|e| format!("Invalid archive URI: {e}"))?;
    if url.scheme() != "archive" {
        return Err("Not an archive URI".to_string());
    }

    let mut src: Option<String> = None;
    let mut path: Option<String> = None;

    for (key, value) in url.query_pairs() {
        if key == "src" {
            src = Some(value.to_string());
        } else if key == "path" {
            path = Some(value.to_string());
        }
    }

    let src = src.ok_or_else(|| "Archive URI missing src parameter".to_string())?;
    let path_raw = path.unwrap_or_else(|| "/".to_string());
    let path = normalize_internal_path(&path_raw)?;

    Ok(ArchiveLocation { src, path })
}

fn build_archive_uri(src: &str, internal_path: &str) -> String {
    let encoded_src = encode(src);
    let encoded_path = encode(internal_path);
    format!("archive:///?src={encoded_src}&path={encoded_path}")
}

fn extension_for_name(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
}

fn archive_extension_from_path(path: &str) -> Option<String> {
    let lower = path.to_ascii_lowercase();
    // Check compound extensions first so nested archives resolve correctly.
    const PATTERNS: [&str; 11] = [
        ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tbz", ".tar.xz", ".txz", ".tar.zst", ".tzst",
        ".tar", ".zip",
    ];
    for pattern in PATTERNS.iter() {
        if lower.ends_with(pattern) {
            return Some(pattern.trim_start_matches('.').to_string());
        }
    }
    if lower.ends_with(".rar") {
        return Some("rar".to_string());
    }
    Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

fn entry_to_file_item(entry: ArchiveEntry, src: &str) -> FileItem {
    let extension = if entry.is_directory {
        None
    } else {
        extension_for_name(&entry.name)
    };
    FileItem {
        name: entry.name.clone(),
        path: build_archive_uri(src, &entry.internal_path),
        size: entry.size,
        modified: entry.modified,
        is_directory: entry.is_directory,
        is_hidden: is_hidden_file(&entry.name),
        is_symlink: false,
        is_git_repo: false,
        extension,
        child_count: None,
        image_width: None,
        image_height: None,
        remote_id: None,
        thumbnail_url: None,
        download_url: None,
    }
}

async fn resolve_non_archive_source(src: &str) -> Result<PathBuf, String> {
    let input = LocationInput::Raw(src.to_string());
    let location = input
        .into_location()
        .map_err(|e| format!("Invalid source location: {e}"))?;

    match location.scheme() {
        "file" => {
            let expanded = expand_path(&location.to_path_string())?;
            Ok(expanded)
        }
        "smb" => {
            #[cfg(not(target_os = "windows"))]
            {
                let smb_path = location.raw().to_string();
                let temp_path = spawn_blocking(move || {
                    download_smb_file_sync(&smb_path)
                        .map(|p| p.to_string_lossy().to_string())
                })
                .await
                .map_err(|e| format!("Task join error: {e}"))??;
                Ok(PathBuf::from(temp_path))
            }
            #[cfg(target_os = "windows")]
            {
                Err("SMB archives are not supported on Windows".to_string())
            }
        }
        "gdrive" => {
            let email = location
                .authority()
                .ok_or_else(|| "Google Drive path missing account".to_string())?;
            let path = location.path().to_string();
            let file_id = get_file_id_by_path(email, &path).await?;
            let file_name = Path::new(&path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("archive");
            let temp_path = download_file_to_temp(email, &file_id, file_name).await?;
            Ok(PathBuf::from(temp_path))
        }
        scheme => Err(format!("Unsupported archive source scheme: {scheme}")),
    }
}

/// Maximum depth of nested archives to prevent DoS via deeply nested archives
const MAX_ARCHIVE_NESTING: usize = 10;

async fn resolve_archive_source(src: &str) -> Result<PathBuf, String> {
    let mut current_src = src.to_string();
    let mut nested_stack: Vec<ArchiveLocation> = Vec::new();

    loop {
        if current_src.starts_with("archive://") {
            if nested_stack.len() >= MAX_ARCHIVE_NESTING {
                return Err("Archive nesting depth exceeded".to_string());
            }
            let nested = parse_archive_uri(&current_src)?;
            current_src = nested.src.clone();
            nested_stack.push(nested);
        } else {
            break;
        }
    }

    let mut resolved_path = resolve_non_archive_source(&current_src).await?;

    while let Some(nested) = nested_stack.pop() {
        let extension = archive_extension_from_path(&nested.path);
        let cache_key = format!("{}::{}", nested.src, nested.path);
        let cache_path = cache::cached_path_for_key(&cache_key, extension.as_deref())?;

        if cache::is_cache_fresh(&cache_path) {
            resolved_path = cache_path;
            continue;
        }

        let _lock = cache::acquire_lock(&cache_path)?;
        let archive_clone = resolved_path.clone();
        let path_clone = nested.path.clone();
        let cache_clone = cache_path.clone();

        let extracted = spawn_blocking(move || {
            reader::extract_entry_to_path(&archive_clone, &path_clone, &cache_clone)
        })
        .await
        .map_err(|e| format!("Task join error: {e}"))??;

        let _ = cache::prune_cache_debounced(cache::cache_ttl());
        resolved_path = extracted;
    }

    Ok(resolved_path)
}

pub async fn extract_archive_entry_to_temp(archive_uri: &str) -> Result<PathBuf, String> {
    let archive_location = parse_archive_uri(archive_uri)?;
    let archive_path = resolve_archive_source(&archive_location.src).await?;
    let extension = archive_extension_from_path(&archive_location.path);
    let cache_key = format!("{}::{}", archive_location.src, archive_location.path);
    let cache_path = cache::cached_path_for_key(&cache_key, extension.as_deref())?;

    if cache::is_cache_fresh(&cache_path) {
        return Ok(cache_path);
    }

    let _lock = cache::acquire_lock(&cache_path)?;
    let archive_clone = archive_path.clone();
    let path_clone = archive_location.path.clone();
    let cache_clone = cache_path.clone();

    let extracted = spawn_blocking(move || {
        reader::extract_entry_to_path(&archive_clone, &path_clone, &cache_clone)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))??;

    let _ = cache::prune_cache_debounced(cache::cache_ttl());
    Ok(extracted)
}

#[derive(Default)]
pub struct ArchiveProvider;

#[async_trait]
impl LocationProvider for ArchiveProvider {
    fn scheme(&self) -> &'static str {
        "archive"
    }

    fn capabilities(&self, _location: &Location) -> LocationCapabilities {
        LocationCapabilities::new("archive", "Archive", true, false)
    }

    async fn read_directory(&self, location: &Location) -> Result<ProviderDirectoryEntries, String> {
        let archive_location = parse_archive_uri(location.raw())?;
        let src = archive_location.src.clone();
        let archive_path = resolve_archive_source(&src).await?;
        let internal_path = archive_location.path.clone();
        let internal_path_for_task = internal_path.clone();

        let entries = spawn_blocking(move || {
            reader::list_directory(&archive_path, &internal_path_for_task)
        })
            .await
            .map_err(|e| format!("Task join error: {e}"))??;

        let file_items = entries
            .into_iter()
            .map(|entry| entry_to_file_item(entry, &src))
            .collect();

        let summary = LocationSummary {
            raw: location.raw().to_string(),
            scheme: "archive".to_string(),
            authority: None,
            path: internal_path,
            display_path: location.raw().to_string(),
        };

        Ok(ProviderDirectoryEntries {
            location: summary,
            entries: file_items,
        })
    }

    async fn get_file_metadata(&self, location: &Location) -> Result<FileItem, String> {
        let archive_location = parse_archive_uri(location.raw())?;
        let src = archive_location.src.clone();
        let archive_path = resolve_archive_source(&src).await?;
        let internal_path = archive_location.path.clone();
        let internal_path_for_task = internal_path.clone();

        let metadata = spawn_blocking(move || {
            reader::get_entry_metadata(&archive_path, &internal_path_for_task)
        })
            .await
            .map_err(|e| format!("Task join error: {e}"))??;

        let mut file_item = entry_to_file_item(metadata, &src);
        if internal_path == "/" {
            file_item.path = location.raw().to_string();
        }
        Ok(file_item)
    }

    async fn create_directory(&self, _location: &Location) -> Result<(), String> {
        Err("Archive locations are read-only".to_string())
    }

    async fn delete(&self, _location: &Location) -> Result<(), String> {
        Err("Archive locations are read-only".to_string())
    }

    async fn rename(&self, _from: &Location, _to: &Location) -> Result<(), String> {
        Err("Archive locations are read-only".to_string())
    }

    async fn copy(&self, _from: &Location, _to: &Location) -> Result<(), String> {
        Err("Archive locations are read-only".to_string())
    }
}

pub fn prune_archive_cache_on_startup() -> Result<(), String> {
    cache::prune_cache_on_startup()
}
