use std::path::Path;
use tauri::command;
use dirs;

use crate::fs_utils::{
    self, FileItem, read_directory_contents, get_file_info, 
    create_directory, delete_file_or_directory, 
    rename_file_or_directory, copy_file_or_directory, expand_path
};

#[command]
pub fn get_home_directory() -> Result<String, String> {
    dirs::home_dir()
        .map(|path| path.to_string_lossy().to_string())
        .ok_or_else(|| "Could not determine home directory".to_string())
}

#[command]
pub fn read_directory(path: String) -> Result<Vec<FileItem>, String> {
    let expanded_path = expand_path(&path)?;
    let path = Path::new(&expanded_path);
    
    if !path.exists() {
        return Err("Path does not exist".to_string());
    }
    
    if !path.is_dir() {
        return Err("Path is not a directory".to_string());
    }
    
    read_directory_contents(path)
}

#[command]
pub fn get_file_metadata(path: String) -> Result<FileItem, String> {
    let expanded_path = expand_path(&path)?;
    let path = Path::new(&expanded_path);
    
    if !path.exists() {
        return Err("Path does not exist".to_string());
    }
    
    get_file_info(path)
}

#[command]
pub fn create_directory(path: String) -> Result<(), String> {
    let expanded_path = expand_path(&path)?;
    let path = Path::new(&expanded_path);
    
    fs_utils::create_directory(path)
}

#[command]
pub fn delete_file(path: String) -> Result<(), String> {
    let expanded_path = expand_path(&path)?;
    let path = Path::new(&expanded_path);
    
    if !path.exists() {
        return Err("Path does not exist".to_string());
    }
    
    delete_file_or_directory(path)
}

#[command]
pub fn rename_file(from_path: String, to_path: String) -> Result<(), String> {
    let expanded_from = expand_path(&from_path)?;
    let expanded_to = expand_path(&to_path)?;
    let from = Path::new(&expanded_from);
    let to = Path::new(&expanded_to);
    
    if !from.exists() {
        return Err("Source path does not exist".to_string());
    }
    
    if to.exists() {
        return Err("Destination path already exists".to_string());
    }
    
    rename_file_or_directory(from, to)
}

#[command]
pub fn copy_file(from_path: String, to_path: String) -> Result<(), String> {
    let expanded_from = expand_path(&from_path)?;
    let expanded_to = expand_path(&to_path)?;
    let from = Path::new(&expanded_from);
    let to = Path::new(&expanded_to);
    
    if !from.exists() {
        return Err("Source path does not exist".to_string());
    }
    
    copy_file_or_directory(from, to)
}

#[command]
pub fn move_file(from_path: String, to_path: String) -> Result<(), String> {
    let expanded_from = expand_path(&from_path)?;
    let expanded_to = expand_path(&to_path)?;
    let from = Path::new(&expanded_from);
    let to = Path::new(&expanded_to);
    
    if !from.exists() {
        return Err("Source path does not exist".to_string());
    }
    
    if to.exists() {
        return Err("Destination path already exists".to_string());
    }
    
    rename_file_or_directory(from, to)
}