use async_trait::async_trait;
use std::path::{Path, PathBuf};

use tauri::async_runtime::spawn_blocking;
use url::Url;
use urlencoding::encode;

use crate::fs_utils::{expand_path, is_hidden_file, FileItem};
use crate::locations::gdrive::provider::{download_file_to_temp, get_file_id_by_path};
use crate::locations::LocationInput;
use crate::locations::{
    Location, LocationCapabilities, LocationProvider, LocationSummary, ProviderDirectoryEntries,
};

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
                    download_smb_file_sync(&smb_path).map(|p| p.to_string_lossy().to_string())
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
            let file_name = path.rsplit('/').next().unwrap_or_default();
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
        resolved_path = extract_to_cache(resolved_path, &nested).await?;
    }

    Ok(resolved_path)
}

/// Extract the entry `location` names from the archive file `archive_path` into the cache, or reuse the cached copy while it is fresh.
async fn extract_to_cache(
    archive_path: PathBuf,
    location: &ArchiveLocation,
) -> Result<PathBuf, String> {
    let cache_key = format!("{}::{}", location.src, location.path);
    let cache_path = cache::cached_path_for_key(
        &cache_key,
        location.path.rsplit('/').next().unwrap_or_default(),
    )?;

    if cache::is_cache_fresh(&cache_path) {
        return Ok(cache_path);
    }

    let _lock = cache::acquire_lock(&cache_path)?;
    let internal_path = location.path.clone();
    let extracted = spawn_blocking(move || {
        reader::extract_entry_to_path(&archive_path, &internal_path, &cache_path)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))??;
    // RAR extraction keeps the archived time, and the cache judges an entry by its modified time.
    std::fs::set_times(
        &extracted,
        std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()),
    )
    .map_err(|e| format!("Failed to date the extracted entry: {e}"))?;

    let _ = cache::prune_cache_debounced(cache::cache_ttl());
    Ok(extracted)
}

pub async fn extract_archive_entry_to_temp(archive_uri: &str) -> Result<PathBuf, String> {
    let archive_location = parse_archive_uri(archive_uri)?;
    let archive_path = resolve_archive_source(&archive_location.src).await?;
    extract_to_cache(archive_path, &archive_location).await
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

    async fn read_directory(
        &self,
        location: &Location,
    ) -> Result<ProviderDirectoryEntries, String> {
        let archive_location = parse_archive_uri(location.raw())?;
        let src = archive_location.src.clone();
        let archive_path = resolve_archive_source(&src).await?;
        let internal_path = archive_location.path.clone();
        let internal_path_for_task = internal_path.clone();

        let entries =
            spawn_blocking(move || reader::list_directory(&archive_path, &internal_path_for_task))
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

#[cfg(test)]
mod tests {
    use super::{build_archive_uri, extract_archive_entry_to_temp};
    use std::io::Write;

    #[tokio::test]
    async fn extract_archive_entry_to_temp_keeps_the_entry_name() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("sample.zip");
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("docs/.env", options).unwrap();
        writer.write_all(b"SECRET=1\n").unwrap();
        writer.finish().unwrap();

        let uri = build_archive_uri(zip_path.to_str().unwrap(), "docs/.env");
        let extracted = extract_archive_entry_to_temp(&uri).await.unwrap();
        assert_eq!(
            extracted.file_name().and_then(|name| name.to_str()),
            Some(".env")
        );
        assert_eq!(std::fs::read_to_string(&extracted).unwrap(), "SECRET=1\n");

        let _ = std::fs::remove_dir_all(extracted.parent().unwrap());
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn an_extracted_entry_is_dated_now() {
        // A copy of the fixture, so no earlier run's cache entry answers.
        let dir = tempfile::tempdir().unwrap();
        let rar = dir.path().join("sample.rar");
        std::fs::copy(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rar/sample.rar"),
            &rar,
        )
        .unwrap();

        let uri = build_archive_uri(rar.to_str().unwrap(), "/folder/inner.txt");
        let extracted = extract_archive_entry_to_temp(&uri).await.unwrap();
        let modified = std::fs::metadata(&extracted).unwrap().modified().unwrap();
        let age = std::time::SystemTime::now()
            .duration_since(modified)
            .unwrap();
        assert!(
            age < std::time::Duration::from_secs(60),
            "dated {age:?} ago"
        );

        let _ = std::fs::remove_dir_all(extracted.parent().unwrap());
    }
}
