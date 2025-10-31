use std::fs;
use std::path::{Path, PathBuf};

use super::{
    Location, LocationCapabilities, LocationProvider, LocationSummary, ProviderDirectoryEntries,
};
use crate::fs_utils::{
    copy_file_or_directory, create_directory, delete_file_or_directory, expand_path, get_file_info,
    read_directory_contents, rename_file_or_directory,
};

#[derive(Default)]
pub struct FileSystemProvider;

impl FileSystemProvider {
    fn ensure_file_scheme<'a>(&self, location: &'a Location) -> Result<&'a Location, String> {
        if location.scheme() != "file" {
            return Err("FileSystemProvider only supports file:// locations".to_string());
        }
        Ok(location)
    }

    fn resolve_path(&self, location: &Location) -> Result<(PathBuf, LocationSummary), String> {
        let location = self.ensure_file_scheme(location)?;
        let raw_path = location.to_path_string();
        let expanded = expand_path(&raw_path)?;
        let normalized = expanded.to_string_lossy().to_string();
        let summary = LocationSummary::new(
            "file",
            location.authority().map(|s| s.to_string()),
            normalized.clone(),
            normalized.clone(),
        );
        Ok((expanded, summary))
    }

    fn resolve_path_only(&self, location: &Location) -> Result<PathBuf, String> {
        let (path, _) = self.resolve_path(location)?;
        Ok(path)
    }

    fn two_stage_case_rename(&self, from: &Path, to: &Path) -> Result<(), String> {
        let parent = from
            .parent()
            .ok_or_else(|| "Invalid source path".to_string())?;

        let mut counter: u32 = 0;
        let temp_path = loop {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_else(|_| std::time::Duration::from_millis(0))
                .as_millis();
            let temp_name = format!(".__rename_tmp_{}_{}", ts, counter);
            let candidate = parent.join(&temp_name);
            if !candidate.exists() {
                break candidate;
            }
            counter += 1;
            if counter > 1_000 {
                return Err("Failed to allocate temporary name for rename".to_string());
            }
        };

        fs::rename(from, &temp_path).map_err(|e| format!("Failed to rename (stage 1): {}", e))?;
        fs::rename(&temp_path, to).map_err(|e| format!("Failed to rename (stage 2): {}", e))?;
        Ok(())
    }
}

impl LocationProvider for FileSystemProvider {
    fn scheme(&self) -> &'static str {
        "file"
    }

    fn capabilities(&self, _location: &Location) -> LocationCapabilities {
        LocationCapabilities::new("file", "Local Filesystem", true, true)
            .with_supports_watching(true)
    }

    fn read_directory(&self, location: &Location) -> Result<ProviderDirectoryEntries, String> {
        let (path, summary) = self.resolve_path(location)?;

        if !path.exists() {
            return Err("Path does not exist".to_string());
        }
        if !path.is_dir() {
            return Err("Path is not a directory".to_string());
        }

        let entries = read_directory_contents(&path)?;

        Ok(ProviderDirectoryEntries {
            location: summary,
            entries,
        })
    }

    fn get_file_metadata(&self, location: &Location) -> Result<crate::fs_utils::FileItem, String> {
        let path = self.resolve_path_only(location)?;
        if !path.exists() {
            return Err("Path does not exist".to_string());
        }
        get_file_info(&path)
    }

    fn create_directory(&self, location: &Location) -> Result<(), String> {
        let path = self.resolve_path_only(location)?;
        create_directory(&path)
    }

    fn delete(&self, location: &Location) -> Result<(), String> {
        let path = self.resolve_path_only(location)?;
        if !path.exists() {
            return Err("Path does not exist".to_string());
        }
        delete_file_or_directory(&path)
    }

    fn rename(&self, from: &Location, to: &Location) -> Result<(), String> {
        let from_path = self.resolve_path_only(from)?;
        if !from_path.exists() {
            return Err("Source path does not exist".to_string());
        }

        let to_path = self.resolve_path_only(to)?;

        let same_parent = from_path.parent() == to_path.parent();
        let from_name = from_path.file_name().and_then(|s| s.to_str());
        let to_name = to_path.file_name().and_then(|s| s.to_str());
        let is_case_only = same_parent
            && from_name.is_some()
            && to_name.is_some()
            && from_name.unwrap().ne(to_name.unwrap())
            && from_name.unwrap().eq_ignore_ascii_case(to_name.unwrap());

        if to_path.exists() && !is_case_only {
            return Err("Destination path already exists".to_string());
        }

        if is_case_only {
            return self.two_stage_case_rename(&from_path, &to_path);
        }

        rename_file_or_directory(&from_path, &to_path)
    }

    fn copy(&self, from: &Location, to: &Location) -> Result<(), String> {
        let from_path = self.resolve_path_only(from)?;
        if !from_path.exists() {
            return Err("Source path does not exist".to_string());
        }
        let to_path = self.resolve_path_only(to)?;
        copy_file_or_directory(&from_path, &to_path)
    }

    fn move_item(&self, from: &Location, to: &Location) -> Result<(), String> {
        let from_path = self.resolve_path_only(from)?;
        if !from_path.exists() {
            return Err("Source path does not exist".to_string());
        }
        let to_path = self.resolve_path_only(to)?;
        if to_path.exists() {
            return Err("Destination path already exists".to_string());
        }
        rename_file_or_directory(&from_path, &to_path)
    }
}
