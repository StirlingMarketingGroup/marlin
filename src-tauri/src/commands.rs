use std::path::Path;
use tauri::command;
use dirs;
use std::process::Command as OsCommand;
use std::time::{Duration, Instant};

use crate::fs_utils::{
    self, FileItem, read_directory_contents, get_file_info, 
    delete_file_or_directory, 
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
pub fn create_directory_command(path: String) -> Result<(), String> {
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

#[command]
pub fn get_system_accent_color() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        // Try AppleHighlightColor first — includes a color name token we can map
        if let Ok(output) = OsCommand::new("defaults").arg("read").arg("-g").arg("AppleHighlightColor").output() {
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
        Err("get_application_icon is only supported on macOS".to_string())
    }
}

#[tauri::command]
pub fn update_hidden_files_menu(
    _app: tauri::AppHandle,
    menu_state: tauri::State<crate::state::MenuState<tauri::Wry>>, 
    checked: bool,
    source: Option<String>
) -> Result<(), String> {
    let _source_str = source.unwrap_or_else(|| "UNKNOWN".to_string());
    
    let item_guard = menu_state.show_hidden_item.lock().map_err(|e| format!("Failed to acquire lock: {}", e))?;
    
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
