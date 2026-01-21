//! Clipboard operations for copy/cut/paste of files.
//!
//! This module provides cross-platform clipboard integration:
//! - macOS: Uses NSPasteboard with NSFilenamesPboardType
//! - Windows: Uses CF_HDROP with Preferred DropEffect for cut/copy intent
//! - Linux: Uses x-special/gnome-copied-files format

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Information about clipboard contents
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardInfo {
    pub has_files: bool,
    pub has_image: bool,
    pub file_paths: Vec<String>,
    /// Whether the clipboard contains a "cut" operation (move intent)
    pub is_cut: bool,
}

/// Result of a paste operation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasteResult {
    pub pasted_paths: Vec<String>,
    pub skipped_count: usize,
    pub error_message: Option<String>,
}

/// Result of pasting an image
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasteImageResult {
    pub path: String,
}

// ============================================================================
// macOS Implementation
// ============================================================================
#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use cocoa::base::{id, nil, NO};
    use cocoa::foundation::NSString;
    use objc::{class, msg_send, runtime::BOOL, sel, sel_impl};

    /// Copy file paths to the system clipboard (macOS)
    pub fn copy_to_clipboard(paths: &[String], is_cut: bool) -> Result<(), String> {
        if paths.is_empty() {
            return Err("No paths provided".into());
        }

        unsafe {
            let pb: id = msg_send![class!(NSPasteboard), generalPasteboard];
            if pb == nil {
                return Err("Failed to get general pasteboard".into());
            }

            // Clear existing contents
            let _: i64 = msg_send![pb, clearContents];

            // Declare types
            let nsfilenames_type: id = NSString::alloc(nil).init_str("NSFilenamesPboardType");
            let types_array: id = msg_send![class!(NSArray), arrayWithObject: nsfilenames_type];
            let _: BOOL = msg_send![pb, declareTypes: types_array owner: nil];

            // Create NSArray of file paths
            let paths_array: id = msg_send![class!(NSMutableArray), array];
            for path in paths {
                let path_nsstring: id = NSString::alloc(nil).init_str(path);
                let _: () = msg_send![paths_array, addObject: path_nsstring];
            }

            // Set the paths on the pasteboard
            let success: BOOL = msg_send![pb, setPropertyList: paths_array forType: nsfilenames_type];
            if success == NO {
                return Err("Failed to set file paths on pasteboard".into());
            }

            // For cut operations, we also set a custom type to indicate cut intent
            // macOS Finder uses "com.apple.pasteboard.promised-file-content-type" for some operations
            // but we'll use a simple marker that we can detect later
            if is_cut {
                let cut_type: id = NSString::alloc(nil).init_str("com.marlin.cut-operation");
                // Add the cut type to the pasteboard
                let types_with_cut: id = msg_send![class!(NSMutableArray), arrayWithCapacity: 2usize];
                let _: () = msg_send![types_with_cut, addObject: nsfilenames_type];
                let _: () = msg_send![types_with_cut, addObject: cut_type];
                let _: BOOL = msg_send![pb, declareTypes: types_with_cut owner: nil];
                let _: BOOL = msg_send![pb, setPropertyList: paths_array forType: nsfilenames_type];
                let marker: id = NSString::alloc(nil).init_str("1");
                let _: BOOL = msg_send![pb, setString: marker forType: cut_type];
            }

            Ok(())
        }
    }

    /// Get clipboard contents information (macOS)
    pub fn get_clipboard_contents() -> Result<ClipboardInfo, String> {
        unsafe {
            let pb: id = msg_send![class!(NSPasteboard), generalPasteboard];
            if pb == nil {
                return Err("Failed to get general pasteboard".into());
            }

            // Check for files
            let nsfilenames_type: id = NSString::alloc(nil).init_str("NSFilenamesPboardType");
            let file_paths_obj: id = msg_send![pb, propertyListForType: nsfilenames_type];

            let mut file_paths = Vec::new();
            let has_files = file_paths_obj != nil;

            if has_files {
                let count: usize = msg_send![file_paths_obj, count];
                for i in 0..count {
                    let path_obj: id = msg_send![file_paths_obj, objectAtIndex: i];
                    if path_obj != nil {
                        let path_cstr: *const i8 = msg_send![path_obj, UTF8String];
                        if !path_cstr.is_null() {
                            let path = std::ffi::CStr::from_ptr(path_cstr)
                                .to_string_lossy()
                                .into_owned();
                            file_paths.push(path);
                        }
                    }
                }
            }

            // Check for cut marker
            let cut_type: id = NSString::alloc(nil).init_str("com.marlin.cut-operation");
            let cut_marker: id = msg_send![pb, stringForType: cut_type];
            let is_cut = cut_marker != nil;

            // Check for image data
            let tiff_type: id = NSString::alloc(nil).init_str("public.tiff");
            let png_type: id = NSString::alloc(nil).init_str("public.png");
            let tiff_data: id = msg_send![pb, dataForType: tiff_type];
            let png_data: id = msg_send![pb, dataForType: png_type];
            let has_image = tiff_data != nil || png_data != nil;

            Ok(ClipboardInfo {
                has_files,
                has_image,
                file_paths,
                is_cut,
            })
        }
    }

    /// Get image data from clipboard as PNG bytes (macOS)
    pub fn get_clipboard_image() -> Result<Vec<u8>, String> {
        unsafe {
            let pb: id = msg_send![class!(NSPasteboard), generalPasteboard];
            if pb == nil {
                return Err("Failed to get general pasteboard".into());
            }

            // Try PNG first, then TIFF
            let png_type: id = NSString::alloc(nil).init_str("public.png");
            let png_data: id = msg_send![pb, dataForType: png_type];

            if png_data != nil {
                let length: usize = msg_send![png_data, length];
                let bytes: *const u8 = msg_send![png_data, bytes];
                if !bytes.is_null() && length > 0 {
                    let slice = std::slice::from_raw_parts(bytes, length);
                    return Ok(slice.to_vec());
                }
            }

            // Try TIFF and convert to PNG
            let tiff_type: id = NSString::alloc(nil).init_str("public.tiff");
            let tiff_data: id = msg_send![pb, dataForType: tiff_type];

            if tiff_data != nil {
                // Create NSImage from TIFF data
                let ns_image: id = msg_send![class!(NSImage), alloc];
                let ns_image: id = msg_send![ns_image, initWithData: tiff_data];

                if ns_image != nil {
                    // Get PNG representation
                    let tiff_rep: id = msg_send![ns_image, TIFFRepresentation];
                    if tiff_rep != nil {
                        let bitmap_rep: id = msg_send![class!(NSBitmapImageRep), imageRepWithData: tiff_rep];
                        if bitmap_rep != nil {
                            let png_type_num: u64 = 4; // NSBitmapImageFileTypePNG
                            let props: id = msg_send![class!(NSDictionary), dictionary];
                            let png_data: id = msg_send![bitmap_rep, representationUsingType: png_type_num properties: props];

                            if png_data != nil {
                                let length: usize = msg_send![png_data, length];
                                let bytes: *const u8 = msg_send![png_data, bytes];
                                if !bytes.is_null() && length > 0 {
                                    let slice = std::slice::from_raw_parts(bytes, length);
                                    return Ok(slice.to_vec());
                                }
                            }
                        }
                    }
                }
            }

            Err("No image data in clipboard".into())
        }
    }
}

// ============================================================================
// Windows Implementation
// ============================================================================
#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use clipboard_win::{formats, get_clipboard, set_clipboard, Clipboard, Getter, Setter};
    use std::mem;

    const DROPEFFECT_COPY: u32 = 1;
    const DROPEFFECT_MOVE: u32 = 2;

    /// Copy file paths to the system clipboard (Windows)
    pub fn copy_to_clipboard(paths: &[String], is_cut: bool) -> Result<(), String> {
        if paths.is_empty() {
            return Err("No paths provided".into());
        }

        let _clip = Clipboard::new_attempts(10).map_err(|e| format!("Failed to open clipboard: {}", e))?;

        // Build DROPFILES structure
        // Format: DROPFILES header followed by double-null-terminated wide string list
        let wide_paths: Vec<Vec<u16>> = paths
            .iter()
            .map(|p| {
                let mut w: Vec<u16> = p.encode_utf16().collect();
                w.push(0); // null terminator
                w
            })
            .collect();

        let total_chars: usize = wide_paths.iter().map(|w| w.len()).sum::<usize>() + 1; // extra null
        let header_size = mem::size_of::<DROPFILES>();
        let data_size = header_size + total_chars * 2;

        let mut buffer = vec![0u8; data_size];

        // DROPFILES structure
        #[repr(C)]
        #[allow(non_snake_case)]
        struct DROPFILES {
            pFiles: u32,
            pt_x: i32,
            pt_y: i32,
            fNC: i32,
            fWide: i32,
        }

        let header = DROPFILES {
            pFiles: header_size as u32,
            pt_x: 0,
            pt_y: 0,
            fNC: 0,
            fWide: 1, // Unicode
        };

        // Copy header
        unsafe {
            std::ptr::copy_nonoverlapping(
                &header as *const DROPFILES as *const u8,
                buffer.as_mut_ptr(),
                header_size,
            );
        }

        // Copy paths
        let mut offset = header_size;
        for wide in &wide_paths {
            let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
            buffer[offset..offset + bytes.len()].copy_from_slice(bytes);
            offset += bytes.len();
        }
        // Final double-null already zeroed

        // Set CF_HDROP
        formats::CF_HDROP
            .write_clipboard(&buffer)
            .map_err(|e| format!("Failed to write CF_HDROP: {}", e))?;

        // Set Preferred DropEffect
        let effect: u32 = if is_cut { DROPEFFECT_MOVE } else { DROPEFFECT_COPY };
        let effect_bytes = effect.to_le_bytes();

        // Register and set "Preferred DropEffect" format
        let format_name = "Preferred DropEffect\0";
        let format_id = unsafe {
            windows::Win32::System::DataExchange::RegisterClipboardFormatA(
                windows::core::PCSTR::from_raw(format_name.as_ptr()),
            )
        };

        if format_id != 0 {
            // Use raw clipboard API to set the custom format
            use windows::Win32::System::DataExchange::*;
            use windows::Win32::System::Memory::*;

            unsafe {
                let hmem = GlobalAlloc(GMEM_MOVEABLE, 4).ok();
                if let Some(hmem) = hmem {
                    let ptr = GlobalLock(hmem);
                    if !ptr.is_null() {
                        std::ptr::copy_nonoverlapping(effect_bytes.as_ptr(), ptr as *mut u8, 4);
                        GlobalUnlock(hmem);
                        if SetClipboardData(format_id, windows::Win32::Foundation::HANDLE(hmem.0 as _))
                            .is_err()
                        {
                            // `SetClipboardData` takes ownership on success; on failure we must free.
                            let _ = GlobalFree(hmem);
                        }
                    } else {
                        let _ = GlobalFree(hmem);
                    }
                }
            }
        }

        Ok(())
    }

    /// Get clipboard contents information (Windows)
    pub fn get_clipboard_contents() -> Result<ClipboardInfo, String> {
        let _clip = Clipboard::new_attempts(10).map_err(|e| format!("Failed to open clipboard: {}", e))?;

        let mut file_paths = Vec::new();
        let mut has_files = false;
        let mut is_cut = false;

        // Try to get CF_HDROP
        if let Ok(data) = formats::CF_HDROP.read_clipboard::<Vec<u8>>() {
            if data.len() > mem::size_of::<DROPFILES>() {
                has_files = true;

                #[repr(C)]
                #[allow(non_snake_case)]
                struct DROPFILES {
                    pFiles: u32,
                    pt_x: i32,
                    pt_y: i32,
                    fNC: i32,
                    fWide: i32,
                }

                let header: DROPFILES =
                    unsafe { std::ptr::read_unaligned(data.as_ptr() as *const DROPFILES) };
                let is_unicode = header.fWide != 0;
                let start = header.pFiles as usize;

                if is_unicode {
                    // Parse Unicode paths
                    let wide_data = &data[start..];
                    let wide_slice = unsafe {
                        std::slice::from_raw_parts(
                            wide_data.as_ptr() as *const u16,
                            wide_data.len() / 2,
                        )
                    };

                    let mut current_start = 0;
                    for (i, &c) in wide_slice.iter().enumerate() {
                        if c == 0 {
                            if i > current_start {
                                let path = String::from_utf16_lossy(&wide_slice[current_start..i]);
                                if !path.is_empty() {
                                    file_paths.push(path);
                                }
                            } else {
                                // Double null = end
                                break;
                            }
                            current_start = i + 1;
                        }
                    }
                }
            }
        }

        // Check Preferred DropEffect
        let format_name = "Preferred DropEffect\0";
        let format_id = unsafe {
            windows::Win32::System::DataExchange::RegisterClipboardFormatA(
                windows::core::PCSTR::from_raw(format_name.as_ptr()),
            )
        };

        if format_id != 0 {
            use windows::Win32::System::DataExchange::*;
            use windows::Win32::System::Memory::*;

            unsafe {
                let handle = GetClipboardData(format_id);
                if let Ok(h) = handle {
                    let ptr = GlobalLock(windows::Win32::Foundation::HGLOBAL(h.0 as _));
                    if !ptr.is_null() {
                        let effect = *(ptr as *const u32);
                        is_cut = effect == DROPEFFECT_MOVE;
                        GlobalUnlock(windows::Win32::Foundation::HGLOBAL(h.0 as _));
                    }
                }
            }
        }

        // Check for image (CF_DIB which we use for extraction)
        let has_image = formats::CF_DIB.is_available();

        Ok(ClipboardInfo {
            has_files,
            has_image,
            file_paths,
            is_cut,
        })
    }

    /// Get image data from clipboard as PNG bytes (Windows)
    pub fn get_clipboard_image() -> Result<Vec<u8>, String> {
        let _clip = Clipboard::new_attempts(10).map_err(|e| format!("Failed to open clipboard: {}", e))?;

        // Get DIB data
        if let Ok(dib_data) = formats::CF_DIB.read_clipboard::<Vec<u8>>() {
            // Convert DIB to PNG using the image crate
            // DIB format: BITMAPINFOHEADER followed by pixel data
            if dib_data.len() > 40 {
                // Try to decode as BMP (add BMP file header)
                let mut bmp_data = vec![0u8; 14 + dib_data.len()];
                bmp_data[0] = b'B';
                bmp_data[1] = b'M';
                let file_size = (14 + dib_data.len()) as u32;
                bmp_data[2..6].copy_from_slice(&file_size.to_le_bytes());
                // Reserved
                bmp_data[6..10].copy_from_slice(&[0, 0, 0, 0]);
                // Pixel data offset (after headers)
                let header_size = u32::from_le_bytes([dib_data[0], dib_data[1], dib_data[2], dib_data[3]]);
                let pixel_offset = 14 + header_size;
                bmp_data[10..14].copy_from_slice(&pixel_offset.to_le_bytes());
                bmp_data[14..].copy_from_slice(&dib_data);

                // Use image crate to convert
                if let Ok(img) = image::load_from_memory(&bmp_data) {
                    let mut png_data = Vec::new();
                    if img
                        .write_to(&mut std::io::Cursor::new(&mut png_data), image::ImageFormat::Png)
                        .is_ok()
                    {
                        return Ok(png_data);
                    }
                }
            }
        }

        Err("No image data in clipboard".into())
    }
}

// ============================================================================
// Linux Implementation
// ============================================================================
#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::process::Command;

    /// Copy file paths to the system clipboard (Linux)
    /// Uses xclip to set multiple clipboard formats for maximum compatibility
    pub fn copy_to_clipboard(paths: &[String], is_cut: bool) -> Result<(), String> {
        if paths.is_empty() {
            return Err("No paths provided".into());
        }

        // Format for x-special/gnome-copied-files
        // Format: "copy\nfile:///path1\nfile:///path2" or "cut\nfile:///path1\n..."
        let prefix = if is_cut { "cut" } else { "copy" };
        let uri_list: String = paths
            .iter()
            .map(|p| {
                let encoded = urlencoding::encode(p);
                format!("file://{}", encoded.replace("%2F", "/"))
            })
            .collect::<Vec<_>>()
            .join("\n");

        let gnome_format = format!("{}\n{}", prefix, uri_list);

        // Try xclip first (most common)
        let xclip_result = Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "x-special/gnome-copied-files"])
            .stdin(std::process::Stdio::piped())
            .spawn();

        if let Ok(mut child) = xclip_result {
            if let Some(stdin) = child.stdin.as_mut() {
                use std::io::Write;
                let _ = stdin.write_all(gnome_format.as_bytes());
            }
            let _ = child.wait();

            // Also set text/uri-list for broader compatibility
            if let Ok(mut child2) = Command::new("xclip")
                .args(["-selection", "clipboard", "-t", "text/uri-list"])
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(stdin) = child2.stdin.as_mut() {
                    use std::io::Write;
                    let _ = stdin.write_all(uri_list.as_bytes());
                }
                let _ = child2.wait();
            }

            return Ok(());
        }

        // Fallback to xsel
        let xsel_result = Command::new("xsel")
            .args(["--clipboard", "--input"])
            .stdin(std::process::Stdio::piped())
            .spawn();

        if let Ok(mut child) = xsel_result {
            if let Some(stdin) = child.stdin.as_mut() {
                use std::io::Write;
                // xsel doesn't support MIME types directly, use plain paths
                let plain = paths.join("\n");
                let _ = stdin.write_all(plain.as_bytes());
            }
            let _ = child.wait();
            return Ok(());
        }

        Err("No clipboard tool available (install xclip or xsel)".into())
    }

    /// Get clipboard contents information (Linux)
    pub fn get_clipboard_contents() -> Result<ClipboardInfo, String> {
        let mut file_paths = Vec::new();
        let mut is_cut = false;
        let mut has_files = false;

        // Try x-special/gnome-copied-files first
        let gnome_result = Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "x-special/gnome-copied-files", "-o"])
            .output();

        if let Ok(output) = gnome_result {
            if output.status.success() {
                let content = String::from_utf8_lossy(&output.stdout);
                let lines: Vec<&str> = content.lines().collect();

                if !lines.is_empty() {
                    // First line is "copy" or "cut"
                    is_cut = lines[0].trim().eq_ignore_ascii_case("cut");

                    // Remaining lines are file URIs
                    for line in lines.iter().skip(1) {
                        let trimmed = line.trim();
                        if let Some(path) = trimmed.strip_prefix("file://") {
                            // URL decode the path
                            if let Ok(decoded) = urlencoding::decode(path) {
                                file_paths.push(decoded.into_owned());
                                has_files = true;
                            }
                        }
                    }
                }
            }
        }

        // Fallback to text/uri-list if no gnome format
        if !has_files {
            let uri_result = Command::new("xclip")
                .args(["-selection", "clipboard", "-t", "text/uri-list", "-o"])
                .output();

            if let Ok(output) = uri_result {
                if output.status.success() {
                    let content = String::from_utf8_lossy(&output.stdout);
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if let Some(path) = trimmed.strip_prefix("file://") {
                            if let Ok(decoded) = urlencoding::decode(path) {
                                file_paths.push(decoded.into_owned());
                                has_files = true;
                            }
                        }
                    }
                }
            }
        }

        // Fallback to xsel for plain text paths if xclip didn't work
        if !has_files {
            let xsel_result = Command::new("xsel")
                .args(["--clipboard", "--output"])
                .output();

            if let Ok(output) = xsel_result {
                if output.status.success() {
                    let content = String::from_utf8_lossy(&output.stdout);
                    for line in content.lines() {
                        let trimmed = line.trim();
                        // Check if it's a file:// URI
                        if let Some(path) = trimmed.strip_prefix("file://") {
                            if let Ok(decoded) = urlencoding::decode(path) {
                                file_paths.push(decoded.into_owned());
                                has_files = true;
                            }
                        } else if trimmed.starts_with('/') {
                            // Plain absolute path
                            file_paths.push(trimmed.to_string());
                            has_files = true;
                        }
                    }
                }
            }
        }

        // Check for image
        let image_result = Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "TARGETS", "-o"])
            .output();

        let has_image = if let Ok(output) = image_result {
            let targets = String::from_utf8_lossy(&output.stdout);
            targets.contains("image/png") || targets.contains("image/jpeg")
        } else {
            false
        };

        Ok(ClipboardInfo {
            has_files,
            has_image,
            file_paths,
            is_cut,
        })
    }

    /// Get image data from clipboard as PNG bytes (Linux)
    pub fn get_clipboard_image() -> Result<Vec<u8>, String> {
        // Try PNG first
        let png_result = Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "image/png", "-o"])
            .output();

        if let Ok(output) = png_result {
            if output.status.success() && !output.stdout.is_empty() {
                return Ok(output.stdout);
            }
        }

        // Try JPEG and convert
        let jpeg_result = Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "image/jpeg", "-o"])
            .output();

        if let Ok(output) = jpeg_result {
            if output.status.success() && !output.stdout.is_empty() {
                // Convert JPEG to PNG using image crate
                if let Ok(img) = image::load_from_memory(&output.stdout) {
                    let mut png_data = Vec::new();
                    if img
                        .write_to(&mut std::io::Cursor::new(&mut png_data), image::ImageFormat::Png)
                        .is_ok()
                    {
                        return Ok(png_data);
                    }
                }
            }
        }

        Err("No image data in clipboard".into())
    }
}

// ============================================================================
// Public API (platform dispatch)
// ============================================================================

/// Copy file paths to the system clipboard
#[cfg(target_os = "macos")]
pub fn copy_to_clipboard(paths: &[String], is_cut: bool) -> Result<(), String> {
    macos::copy_to_clipboard(paths, is_cut)
}

#[cfg(target_os = "windows")]
pub fn copy_to_clipboard(paths: &[String], is_cut: bool) -> Result<(), String> {
    windows::copy_to_clipboard(paths, is_cut)
}

#[cfg(target_os = "linux")]
pub fn copy_to_clipboard(paths: &[String], is_cut: bool) -> Result<(), String> {
    linux::copy_to_clipboard(paths, is_cut)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub fn copy_to_clipboard(_paths: &[String], _is_cut: bool) -> Result<(), String> {
    Err("Clipboard not supported on this platform".into())
}

/// Get clipboard contents information
#[cfg(target_os = "macos")]
pub fn get_clipboard_contents() -> Result<ClipboardInfo, String> {
    macos::get_clipboard_contents()
}

#[cfg(target_os = "windows")]
pub fn get_clipboard_contents() -> Result<ClipboardInfo, String> {
    windows::get_clipboard_contents()
}

#[cfg(target_os = "linux")]
pub fn get_clipboard_contents() -> Result<ClipboardInfo, String> {
    linux::get_clipboard_contents()
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub fn get_clipboard_contents() -> Result<ClipboardInfo, String> {
    Ok(ClipboardInfo {
        has_files: false,
        has_image: false,
        file_paths: Vec::new(),
        is_cut: false,
    })
}

/// Get image data from clipboard as PNG bytes
#[cfg(target_os = "macos")]
pub fn get_clipboard_image() -> Result<Vec<u8>, String> {
    macos::get_clipboard_image()
}

#[cfg(target_os = "windows")]
pub fn get_clipboard_image() -> Result<Vec<u8>, String> {
    windows::get_clipboard_image()
}

#[cfg(target_os = "linux")]
pub fn get_clipboard_image() -> Result<Vec<u8>, String> {
    linux::get_clipboard_image()
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub fn get_clipboard_image() -> Result<Vec<u8>, String> {
    Err("Clipboard not supported on this platform".into())
}

// ============================================================================
// File Operations
// ============================================================================

/// Generate a unique filename by appending (2), (3), etc. if the file exists
fn generate_unique_path(base_path: &Path) -> std::path::PathBuf {
    if !base_path.exists() {
        return base_path.to_path_buf();
    }

    let parent = base_path.parent().unwrap_or(Path::new(""));
    let stem = base_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let ext = base_path.extension().and_then(|e| e.to_str());

    for i in 2..1000 {
        let new_name = if let Some(e) = ext {
            format!("{} ({}).{}", stem, i, e)
        } else {
            format!("{} ({})", stem, i)
        };

        let new_path = parent.join(&new_name);
        if !new_path.exists() {
            return new_path;
        }
    }

    // Fallback with high-resolution timestamp + counter
    use std::sync::atomic::{AtomicU64, Ordering};
    static FALLBACK_COUNTER: AtomicU64 = AtomicU64::new(0);

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = FALLBACK_COUNTER.fetch_add(1, Ordering::Relaxed);

    let new_name = if let Some(e) = ext {
        format!("{}_{}_{}.{}", stem, timestamp, counter, e)
    } else {
        format!("{}_{}_{}", stem, timestamp, counter)
    };

    parent.join(&new_name)
}

/// Paste files from clipboard to destination directory
pub fn paste_files(destination: &str, is_cut: bool) -> Result<PasteResult, String> {
    let clipboard_info = get_clipboard_contents()?;

    if clipboard_info.file_paths.is_empty() {
        return Err("No files in clipboard".into());
    }

    let dest_path = Path::new(destination);
    if !dest_path.exists() {
        return Err(format!("Destination does not exist: {}", destination));
    }
    if !dest_path.is_dir() {
        return Err(format!("Destination is not a directory: {}", destination));
    }

    let mut pasted_paths = Vec::new();
    let mut skipped_count = 0;
    let mut last_error: Option<String> = None;

    for source_path_str in &clipboard_info.file_paths {
        let source_path = Path::new(source_path_str);

        // Skip if source doesn't exist (for cut operations, file may have been moved/deleted)
        if !source_path.exists() {
            skipped_count += 1;
            continue;
        }

        // Skip directories (v1 limitation)
        if source_path.is_dir() {
            skipped_count += 1;
            continue;
        }

        let file_name = source_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unnamed");

        let target_path = dest_path.join(file_name);
        let final_target = generate_unique_path(&target_path);

        // Use actual clipboard is_cut if available, otherwise use parameter
        let should_move = is_cut || clipboard_info.is_cut;

        let result = if should_move {
            // Move operation
            std::fs::rename(source_path, &final_target)
                .or_else(|_| {
                    // If rename fails (cross-device), fall back to copy + delete
                    std::fs::copy(source_path, &final_target)?;
                    std::fs::remove_file(source_path)?;
                    Ok::<_, std::io::Error>(())
                })
        } else {
            // Copy operation
            std::fs::copy(source_path, &final_target).map(|_| ())
        };

        match result {
            Ok(()) => {
                pasted_paths.push(final_target.to_string_lossy().into_owned());
            }
            Err(e) => {
                skipped_count += 1;
                last_error = Some(format!("Failed to paste {}: {}", file_name, e));
            }
        }
    }

    Ok(PasteResult {
        pasted_paths,
        skipped_count,
        error_message: last_error,
    })
}

/// Paste an image from clipboard to a file
pub fn paste_image(destination: &str, filename: Option<&str>) -> Result<PasteImageResult, String> {
    let image_data = get_clipboard_image()?;

    let dest_path = Path::new(destination);
    if !dest_path.exists() {
        return Err(format!("Destination does not exist: {}", destination));
    }
    if !dest_path.is_dir() {
        return Err(format!("Destination is not a directory: {}", destination));
    }

    // Generate filename with timestamp
    let file_name = if let Some(name) = filename {
        name.to_string()
    } else {
        let now = chrono::Local::now();
        // Use dashes instead of colons for Windows compatibility
        format!("Screenshot {}.png", now.format("%Y-%m-%d at %H-%M-%S"))
    };

    let target_path = dest_path.join(&file_name);
    let final_target = generate_unique_path(&target_path);

    std::fs::write(&final_target, &image_data)
        .map_err(|e| format!("Failed to write image: {}", e))?;

    Ok(PasteImageResult {
        path: final_target.to_string_lossy().into_owned(),
    })
}

// ============================================================================
// Tauri Commands
// ============================================================================

/// Copy file paths to the system clipboard
#[tauri::command]
pub async fn clipboard_copy_files(paths: Vec<String>, is_cut: bool) -> Result<(), String> {
    tokio::task::spawn_blocking(move || copy_to_clipboard(&paths, is_cut))
        .await
        .map_err(|e| format!("Task failed: {}", e))?
}

/// Get clipboard contents information
#[tauri::command]
pub async fn clipboard_get_contents() -> Result<ClipboardInfo, String> {
    tokio::task::spawn_blocking(get_clipboard_contents)
        .await
        .map_err(|e| format!("Task failed: {}", e))?
}

/// Paste files from clipboard to destination
#[tauri::command]
pub async fn clipboard_paste_files(destination: String, is_cut: bool) -> Result<PasteResult, String> {
    tokio::task::spawn_blocking(move || paste_files(&destination, is_cut))
        .await
        .map_err(|e| format!("Task failed: {}", e))?
}

/// Paste an image from clipboard to destination
#[tauri::command]
pub async fn clipboard_paste_image(
    destination: String,
    filename: Option<String>,
) -> Result<PasteImageResult, String> {
    tokio::task::spawn_blocking(move || paste_image(&destination, filename.as_deref()))
        .await
        .map_err(|e| format!("Task failed: {}", e))?
}
