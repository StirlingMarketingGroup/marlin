use std::path::Path;
use tauri::command;
use dirs;
use std::process::Command as OsCommand;
use std::sync::Arc;
use tokio::sync::OnceCell;

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
            name: "Root".to_string(),
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
                            if name != "Macintosh HD" { // Skip default system volume
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
pub fn eject_drive(path: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // On macOS, use diskutil to eject the volume
        let output = OsCommand::new("diskutil")
            .arg("eject")
            .arg(&path)
            .output()
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
        let output = OsCommand::new("umount")
            .arg(&path)
            .output()
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
static THUMBNAIL_SERVICE: OnceCell<Result<Arc<crate::thumbnails::ThumbnailService>, String>> = OnceCell::const_new();

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
    };

    service.request_thumbnail(request).await
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
        if status.success() { Ok(()) } else { Err(format!("'open' exited with status: {}", status)) }
    }

    #[cfg(target_os = "windows")]
    {
        // Use cmd start to let the shell decide the default handler
        // start requires a window title arg (empty string)
        let status = OsCommand::new("cmd")
            .args(["/C", "start", "", &path_str])
            .status()
            .map_err(|e| format!("Failed to spawn 'start': {}", e))?;
        if status.success() { Ok(()) } else { Err(format!("'start' exited with status: {}", status)) }
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
                if status.success() { Ok(()) } else { Err(format!("'gio open' exited with status: {}", status)) }
            }
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Err("open_path is not supported on this platform".to_string())
    }
}
