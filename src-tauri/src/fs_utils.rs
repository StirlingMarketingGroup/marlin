use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileItem {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub modified: DateTime<Utc>,
    pub is_directory: bool,
    pub is_hidden: bool,
    pub is_symlink: bool,
    pub extension: Option<String>,
}

fn build_file_item(path: &Path) -> Result<FileItem, String> {
    let symlink_metadata =
        fs::symlink_metadata(path).map_err(|e| format!("Failed to get metadata: {}", e))?;

    let is_symlink = symlink_metadata.file_type().is_symlink();
    let target_metadata = if is_symlink {
        fs::metadata(path).ok()
    } else {
        None
    };

    let mut is_directory = target_metadata
        .as_ref()
        .map(|m| m.is_dir())
        .unwrap_or_else(|| symlink_metadata.is_dir());

    if !is_directory && is_symlink {
        if fs::read_dir(path).is_ok() {
            is_directory = true;
        }
    }

    let metadata = target_metadata.as_ref().unwrap_or(&symlink_metadata);

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Unknown")
        .to_string();

    let is_hidden = file_name.starts_with('.');

    let extension = if metadata.is_file() {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|s| s.to_lowercase())
    } else {
        None
    };

    let modified = metadata
        .modified()
        .map(|time| DateTime::from(time))
        .unwrap_or_else(|_| Utc::now());

    Ok(FileItem {
        name: file_name,
        path: path.to_string_lossy().to_string(),
        size: metadata.len(),
        modified,
        is_directory,
        is_hidden,
        is_symlink,
        extension,
    })
}

pub fn read_directory_contents(path: &Path) -> Result<Vec<FileItem>, String> {
    let entries = fs::read_dir(path).map_err(|e| format!("Failed to read directory: {}", e))?;

    let mut files = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let file_path = entry.path();

        match build_file_item(&file_path) {
            Ok(file_item) => files.push(file_item),
            Err(_) => continue,
        }
    }

    Ok(files)
}

pub fn get_file_info(path: &Path) -> Result<FileItem, String> {
    build_file_item(path)
}

pub fn create_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("Failed to create directory: {}", e))
}

pub fn delete_file_or_directory(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|e| format!("Failed to delete directory: {}", e))
    } else {
        fs::remove_file(path).map_err(|e| format!("Failed to delete file: {}", e))
    }
}

pub fn rename_file_or_directory(from: &Path, to: &Path) -> Result<(), String> {
    fs::rename(from, to).map_err(|e| format!("Failed to rename: {}", e))
}

pub fn copy_file_or_directory(from: &Path, to: &Path) -> Result<(), String> {
    if from.is_dir() {
        copy_dir_recursive(from, to)
    } else {
        fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| format!("Failed to copy file: {}", e))
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst)
        .map_err(|e| format!("Failed to create destination directory: {}", e))?;

    for entry in fs::read_dir(src).map_err(|e| format!("Failed to read source directory: {}", e))? {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path).map_err(|e| format!("Failed to copy file: {}", e))?;
        }
    }

    Ok(())
}

pub fn expand_path(path: &str) -> Result<PathBuf, String> {
    if path.starts_with('~') {
        let home =
            dirs::home_dir().ok_or_else(|| "Could not determine home directory".to_string())?;

        if path == "~" {
            Ok(home)
        } else if path.starts_with("~/") {
            Ok(home.join(&path[2..]))
        } else {
            Err("Invalid path format".to_string())
        }
    } else {
        Ok(PathBuf::from(path))
    }
}
