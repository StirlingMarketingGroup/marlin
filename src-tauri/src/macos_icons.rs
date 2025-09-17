#![allow(unexpected_cfgs)]

#[cfg(target_os = "macos")]
use crate::macos_security;
#[cfg(target_os = "macos")]
use base64::Engine as _; // bring encode() into scope
#[cfg(target_os = "macos")]
use cocoa::base::{id, nil};
#[cfg(target_os = "macos")]
use cocoa::foundation::{NSAutoreleasePool, NSSize, NSString};
#[cfg(target_os = "macos")]
use dirs;
#[cfg(target_os = "macos")]
use log::warn;
#[cfg(target_os = "macos")]
use objc::{class, msg_send, sel, sel_impl};
#[cfg(target_os = "macos")]
use std::collections::hash_map::DefaultHasher;
#[cfg(target_os = "macos")]
use std::hash::{Hash, Hasher};
#[cfg(target_os = "macos")]
use std::{fs, path::Path, time::UNIX_EPOCH};

#[cfg(target_os = "macos")]
pub fn app_icon_png_base64(path: &str, size: u32) -> Result<String, String> {
    unsafe {
        let _pool: id = NSAutoreleasePool::new(nil);

        let path_obj = Path::new(path);
        let _scope_guard = macos_security::retain_access(path_obj)?;

        // Build a persistent cache file path under user's cache dir, keyed by (path,size,mtime,len)
        let meta = fs::metadata(path).map_err(|e| format!("metadata failed: {}", e))?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        size.hash(&mut hasher);
        meta.len().hash(&mut hasher);
        mtime.hash(&mut hasher);
        let key = format!("{:016x}.png", hasher.finish());
        let mut cache_dir = dirs::cache_dir().unwrap_or(std::env::temp_dir());
        cache_dir.push("marlin_icons_native");
        let _ = fs::create_dir_all(&cache_dir);
        let cache_file = cache_dir.join(key);

        if let Ok(bytes) = fs::read(&cache_file) {
            let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
            return Ok(format!("data:image/png;base64,{}", encoded));
        }

        // NSWorkspace.sharedWorkspace.iconForFile(path)
        let ns_path: id = NSString::alloc(nil).init_str(path);
        let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        if workspace == nil {
            return Err("NSWorkspace unavailable".into());
        }
        let image: id = msg_send![workspace, iconForFile: ns_path];
        if image == nil {
            return Err("iconForFile returned nil".into());
        }

        // Resize to requested size (logical points)
        let sz = NSSize::new(size as f64, size as f64);
        let _: () = msg_send![image, setSize: sz];

        // Convert NSImage -> TIFF -> NSBitmapImageRep -> PNG (NSData)
        let tiff: id = msg_send![image, TIFFRepresentation];
        if tiff == nil {
            return Err("TIFFRepresentation is nil".into());
        }
        let rep_class = class!(NSBitmapImageRep);
        let rep: id = msg_send![rep_class, imageRepWithData: tiff];
        if rep == nil {
            return Err("imageRepWithData returned nil".into());
        }

        // NSPNGFileType is 4 (NSBitmapImageFileType enum)
        let dict_class = class!(NSDictionary);
        let empty_dict: id = msg_send![dict_class, dictionary];
        let png_data: id = msg_send![rep, representationUsingType: 4u64 properties: empty_dict];
        if png_data == nil {
            return Err("PNG conversion failed".into());
        }

        // Extract bytes from NSData and return as base64 data URL
        let len: usize = msg_send![png_data, length];
        let bytes_ptr: *const std::os::raw::c_void = msg_send![png_data, bytes];
        if bytes_ptr.is_null() || len == 0 {
            return Err("PNG data is empty".into());
        }
        let bytes = std::slice::from_raw_parts(bytes_ptr as *const u8, len);
        // Persist to cache for reuse across restarts
        let _ = fs::write(&cache_file, bytes);
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        let result = Ok(format!("data:image/png;base64,{}", encoded));

        if let Err(err) = macos_security::store_bookmark_if_needed(path_obj) {
            warn!(
                "Failed to persist security bookmark while reading {}: {}",
                path, err
            );
        }

        result
    }
}
