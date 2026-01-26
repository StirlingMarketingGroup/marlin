#![allow(unexpected_cfgs)]

use base64::Engine as _;
use objc2::class;
use objc2::msg_send;
use objc2::rc::autoreleasepool;
use objc2::runtime::{AnyObject, Bool};
use objc2_foundation::{NSArray, NSPoint, NSSize, NSString};

pub fn start_native_drag(
    paths: Vec<String>,
    preview_image: Option<String>,
    _drag_offset_y: Option<f64>,
) -> Result<(), String> {
    if paths.is_empty() {
        return Err("No paths provided".into());
    }

    autoreleasepool(|_| unsafe {
        // Get NSApplication and key window
        let ns_app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        if ns_app.is_null() {
            return Err("NSApplication unavailable".into());
        }

        let window: *mut AnyObject = msg_send![ns_app, keyWindow];
        if window.is_null() {
            return Err("No key window".into());
        }

        // Get content view and find the deepest view under the cursor
        let content_view: *mut AnyObject = msg_send![window, contentView];
        if content_view.is_null() {
            return Err("No content view".into());
        }

        // Use mouseLocationOutsideOfEventStream for current mouse position
        let mouse_location_window: NSPoint = msg_send![window, mouseLocationOutsideOfEventStream];
        let mouse_in_content: NSPoint = msg_send![
            content_view,
            convertPoint: mouse_location_window,
            fromView: std::ptr::null_mut::<AnyObject>()
        ];

        // Hit test to find the actual view under the cursor (likely WKWebView)
        let hit_view: *mut AnyObject = msg_send![content_view, hitTest: mouse_in_content];
        let source_view: *mut AnyObject = if !hit_view.is_null() {
            hit_view
        } else {
            content_view
        };

        // Convert mouse location to source view coordinates
        let mouse_in_source: NSPoint = msg_send![
            source_view,
            convertPoint: mouse_location_window,
            fromView: std::ptr::null_mut::<AnyObject>()
        ];

        // Create drag image
        let drag_img: *mut AnyObject = if let Some(ref data_url) = preview_image {
            match create_drag_image_from_data_url(data_url) {
                Ok(img) => img,
                Err(_) => create_drag_image_from_file_icon(&paths[0])?,
            }
        } else {
            create_drag_image_from_file_icon(&paths[0])?
        };

        let image_size: NSSize = msg_send![drag_img, size];

        // Ensure image has a reasonable size and handle zero-size images
        let final_image_size = if image_size.width <= 0.0
            || image_size.height <= 0.0
            || image_size.width < 64.0
            || image_size.height < 64.0
        {
            let new_size = NSSize::new(128.0, 128.0);
            let _: () = msg_send![drag_img, setSize: new_size];
            new_size
        } else {
            image_size
        };

        // Use the deprecated but more reliable dragImage method for better positioning control
        // Create NSDragPboard with file paths
        let drag_pboard_name = NSString::from_str("NSDragPboard");
        let pb: *mut AnyObject = msg_send![class!(NSPasteboard), pasteboardWithName: &*drag_pboard_name];

        if pb.is_null() {
            return Err("Failed to create NSDragPboard".into());
        }

        // Clear and set pasteboard types
        let _: () = msg_send![pb, clearContents];
        let nsfilenames_type = NSString::from_str("NSFilenamesPboardType");
        let types_array = NSArray::from_retained_slice(&[nsfilenames_type.clone()]);
        let _: i64 = msg_send![
            pb,
            declareTypes: &*types_array,
            owner: std::ptr::null_mut::<AnyObject>()
        ];

        // Create NSArray of file paths
        let path_strings: Vec<_> = paths.iter().map(|path| NSString::from_str(path)).collect();
        let paths_array = NSArray::from_retained_slice(&path_strings);

        // Set the paths on the pasteboard
        let success: Bool = msg_send![
            pb,
            setPropertyList: &*paths_array,
            forType: &*nsfilenames_type
        ];
        if success.is_false() {
            return Err("Failed to set file paths on pasteboard".into());
        }

        // Get current event for the drag
        let current_event: *mut AnyObject = msg_send![ns_app, currentEvent];

        // Calculate drag position - center the image exactly on the mouse cursor
        let drag_point = NSPoint::new(
            mouse_in_source.x - (final_image_size.width / 2.0),
            mouse_in_source.y + (final_image_size.height / 2.0), // Adjust for flipped coordinates
        );

        // Use the deprecated but working dragImage method
        let slide_back = Bool::YES;
        let _: () = msg_send![
            source_view,
            dragImage: drag_img,
            at: drag_point,
            offset: NSPoint::new(0.0, 0.0),
            event: current_event,
            pasteboard: pb,
            source: source_view,
            slideBack: slide_back
        ];

        Ok(())
    })
}

fn create_drag_image_from_data_url(data_url: &str) -> Result<*mut AnyObject, String> {
    unsafe {
        // Extract base64 data
        let b64 = if let Some(idx) = data_url.find(',') {
            &data_url[(idx + 1)..]
        } else {
            data_url
        };

        // Add size limit to prevent DoS from huge payloads (max 50MB)
        const MAX_ENCODED_SIZE: usize = 50 * 1024 * 1024;
        if b64.len() > MAX_ENCODED_SIZE {
            return Err("Image data too large".to_string());
        }

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|_| "Failed to decode base64 image data".to_string())?;

        if bytes.is_empty() {
            return Err("Empty image data".to_string());
        }

        // Create NSData
        // Note: These objects are autoreleased within the surrounding autoreleasepool,
        // which is fine since the drag operation completes within that scope.
        let data: *mut AnyObject = msg_send![class!(NSData), alloc];
        if data.is_null() {
            return Err("Failed to allocate NSData".to_string());
        }
        let data: *mut AnyObject =
            msg_send![data, initWithBytes: bytes.as_ptr(), length: bytes.len()];
        if data.is_null() {
            return Err("Failed to initialize NSData".to_string());
        }

        // Create NSImage
        let img: *mut AnyObject = msg_send![class!(NSImage), alloc];
        if img.is_null() {
            return Err("Failed to allocate NSImage".to_string());
        }
        let img: *mut AnyObject = msg_send![img, initWithData: data];

        if img.is_null() {
            Err("Failed to create NSImage from data".to_string())
        } else {
            Ok(img)
        }
    }
}

fn create_drag_image_from_file_icon(path: &str) -> Result<*mut AnyObject, String> {
    unsafe {
        let path_nsstring = NSString::from_str(path);
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        let icon: *mut AnyObject = msg_send![workspace, iconForFile: &*path_nsstring];

        if icon.is_null() {
            Err("Failed to get file icon".to_string())
        } else {
            Ok(icon)
        }
    }
}
