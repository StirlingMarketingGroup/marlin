use base64::Engine as _;
use chrono::{DateTime, Utc};
use dirs;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::path::PathBuf;
use std::process::Command as OsCommand;
use std::sync::Arc;
use tauri::{command, AppHandle, Manager};
use tokio::process::Command as TokioCommand;
use tokio::sync::OnceCell;

use crate::fs_utils::{
    self, copy_file_or_directory, delete_file_or_directory, expand_path, get_file_info,
    read_directory_contents, rename_file_or_directory, FileItem,
};
use crate::fs_watcher;

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

    // Allow case-only renames on case-insensitive filesystems by using a two-step rename.
    // If the destination exists but only differs by letter casing, perform: from -> temp -> to
    let same_parent = from.parent() == to.parent();
    let from_name = from.file_name().and_then(|s| s.to_str());
    let to_name = to.file_name().and_then(|s| s.to_str());
    let is_case_only = same_parent
        && from_name.is_some()
        && to_name.is_some()
        && from_name != to_name
        && from_name.unwrap().eq_ignore_ascii_case(to_name.unwrap());

    if to.exists() && !is_case_only {
        return Err("Destination path already exists".to_string());
    }

    if is_case_only {
        // Two-step rename to update case where FS is case-insensitive
        let parent = from
            .parent()
            .ok_or_else(|| "Invalid source path".to_string())?;
        // Generate a temporary name that shouldn't collide
        let mut counter: u32 = 0;
        let mut temp_path;
        loop {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_else(|_| std::time::Duration::from_millis(0))
                .as_millis();
            let temp_name = format!(".__rename_tmp_{}_{}", ts, counter);
            temp_path = parent.join(&temp_name);
            if !temp_path.exists() {
                break;
            }
            counter += 1;
            if counter > 1000 {
                return Err("Failed to allocate temporary name for rename".to_string());
            }
        }

        fs::rename(from, &temp_path).map_err(|e| format!("Failed to rename (stage 1): {}", e))?;
        fs::rename(&temp_path, to).map_err(|e| format!("Failed to rename (stage 2): {}", e))?;
        return Ok(());
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
        .write_to(&mut cursor, image::ImageOutputFormat::Png)
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

    let window = tauri::WebviewWindowBuilder::new(&app, &window_label, url)
        .title("")
        .inner_size(1200.0, 800.0)
        .resizable(true)
        .fullscreen(false)
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .build()
        .map_err(|e| format!("Failed to create window: {}", e))?;

    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_decorum::WebviewWindowExt;
        let _ = window.set_traffic_lights_inset(16.0, 24.0);
    }

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
        builder = builder
            .items(&[&rename_item, &copy_name_item, &copy_full_name_item])
            .separator();
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

fn normalize_path(mut s: String) -> String {
    if s.is_empty() {
        return "/".to_string();
    }
    s = s.replace('\\', "/");
    while s.contains("//") {
        s = s.replace("//", "/");
    }
    if s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    if s.len() == 2 && s.chars().nth(1) == Some(':') {
        s.push('/');
    }
    if s.is_empty() {
        s = "/".into();
    }
    s
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

#[derive(Serialize, Deserialize, Clone)]
pub struct PinnedDirectory {
    pub name: String,
    pub path: String,
    pub pinned_at: DateTime<Utc>,
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

#[command]
pub fn get_pinned_directories() -> Result<Vec<PinnedDirectory>, String> {
    let path = pinned_directories_path()?;
    if !path.exists() {
        return Ok(vec![]);
    }

    let mut file =
        fs::File::open(&path).map_err(|e| format!("Failed to open pinned directories: {}", e))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|e| format!("Failed to read pinned directories: {}", e))?;

    let pinned_dirs: Vec<PinnedDirectory> = serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse pinned directories: {}", e))?;

    Ok(pinned_dirs)
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
    let mut pinned_dirs = get_pinned_directories()?;

    // Check if already pinned
    if pinned_dirs.iter().any(|p| p.path == expanded_path_str) {
        return Err("Directory is already pinned".to_string());
    }

    // Limit to 20 pinned directories
    if pinned_dirs.len() >= 20 {
        return Err("Maximum number of pinned directories reached (20)".to_string());
    }

    let dir_name = name.unwrap_or_else(|| {
        path_obj
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unknown")
            .to_string()
    });

    let new_pin = PinnedDirectory {
        name: dir_name,
        path: expanded_path_str,
        pinned_at: Utc::now(),
    };

    pinned_dirs.push(new_pin.clone());
    save_pinned_directories(&pinned_dirs)?;

    Ok(new_pin)
}

#[command]
pub fn remove_pinned_directory(path: String) -> Result<bool, String> {
    let expanded_path = expand_path(&path)?;
    let expanded_path_str = expanded_path.to_string_lossy().to_string();
    let mut pinned_dirs = get_pinned_directories()?;

    let initial_len = pinned_dirs.len();
    pinned_dirs.retain(|p| p.path != expanded_path_str);

    if pinned_dirs.len() < initial_len {
        save_pinned_directories(&pinned_dirs)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

#[command]
pub fn reorder_pinned_directories(paths: Vec<String>) -> Result<(), String> {
    let current_pins = get_pinned_directories()?;
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

fn save_pinned_directories(pinned_dirs: &[PinnedDirectory]) -> Result<(), String> {
    let path = pinned_directories_path()?;
    let json = serde_json::to_string_pretty(pinned_dirs)
        .map_err(|e| format!("Failed to serialize pinned directories: {}", e))?;

    let mut file = fs::File::create(&path)
        .map_err(|e| format!("Failed to create pinned directories file: {}", e))?;

    file.write_all(json.as_bytes())
        .map_err(|e| format!("Failed to write pinned directories: {}", e))?;

    Ok(())
}
