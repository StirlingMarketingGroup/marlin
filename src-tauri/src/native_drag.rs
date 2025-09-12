#![allow(unexpected_cfgs)]

use cocoa::base::{id, nil, YES, NO};
use cocoa::foundation::{NSAutoreleasePool, NSString, NSPoint, NSSize};
use objc::{class, msg_send, runtime::BOOL, sel, sel_impl};
use base64::Engine as _;


pub fn start_native_drag(paths: Vec<String>, preview_image: Option<String>, _drag_offset_y: Option<f64>) -> Result<(), String> {
    unsafe {
        if paths.is_empty() {
            return Err("No paths provided".into());
        }
        
        
        let _pool: id = NSAutoreleasePool::new(nil);

        // Get NSApplication and key window
        let ns_app: id = msg_send![class!(NSApplication), sharedApplication];
        if ns_app == nil {
            return Err("NSApplication unavailable".into());
        }
        
        let window: id = msg_send![ns_app, keyWindow];
        if window == nil {
            return Err("No key window".into());
        }
        
        // Get content view and find the deepest view under the cursor
        let content_view: id = msg_send![window, contentView];
        if content_view == nil {
            return Err("No content view".into());
        }
        
        // Use mouseLocationOutsideOfEventStream for current mouse position
        let mouse_location_window: NSPoint = msg_send![window, mouseLocationOutsideOfEventStream];
        let mouse_in_content: NSPoint = msg_send![content_view, convertPoint: mouse_location_window fromView: nil];
        
        // Hit test to find the actual view under the cursor (likely WKWebView)
        let hit_view: id = msg_send![content_view, hitTest: mouse_in_content];
        let source_view: id = if hit_view != nil { hit_view } else { content_view };
        
        // Convert mouse location to source view coordinates  
        let mouse_in_source: NSPoint = msg_send![source_view, convertPoint: mouse_location_window fromView: nil];
        
        // Create drag image
        let drag_img: id = if let Some(ref data_url) = preview_image {
            match create_drag_image_from_data_url(data_url) {
                Ok(img) => img,
                Err(_) => create_drag_image_from_file_icon(&paths[0])?
            }
        } else {
            create_drag_image_from_file_icon(&paths[0])?
        };
        
        let image_size: NSSize = msg_send![drag_img, size];
        
        // Ensure image has a reasonable size and handle zero-size images
        let final_image_size = if image_size.width <= 0.0 || image_size.height <= 0.0 || image_size.width < 64.0 || image_size.height < 64.0 {
            let new_size = NSSize::new(128.0, 128.0);
            let _: () = msg_send![drag_img, setSize: new_size];
            new_size
        } else {
            image_size
        };
        
        // Use the deprecated but more reliable dragImage method for better positioning control
        // Create NSDragPboard with file paths
        let drag_pboard_name: id = NSString::alloc(nil).init_str("NSDragPboard");
        let pb: id = msg_send![class!(NSPasteboard), pasteboardWithName: drag_pboard_name];
        
        if pb == nil {
            return Err("Failed to create NSDragPboard".into());
        }
        
        // Clear and set pasteboard types
        let _: () = msg_send![pb, clearContents];
        let nsfilenames_type: id = NSString::alloc(nil).init_str("NSFilenamesPboardType");
        let types_array: id = msg_send![class!(NSArray), arrayWithObject: nsfilenames_type];
        let _: BOOL = msg_send![pb, declareTypes: types_array owner: nil];
        
        // Create NSArray of file paths
        let paths_array: id = msg_send![class!(NSMutableArray), array];
        for path in &paths {
            let path_nsstring: id = NSString::alloc(nil).init_str(path);
            let _: () = msg_send![paths_array, addObject: path_nsstring];
        }
        
        // Set the paths on the pasteboard
        let success: BOOL = msg_send![pb, setPropertyList: paths_array forType: nsfilenames_type];
        if success == NO {
            return Err("Failed to set file paths on pasteboard".into());
        }
        
        // Get current event for the drag
        let current_event: id = msg_send![ns_app, currentEvent];
        
        // Calculate drag position - center the image exactly on the mouse cursor
        let drag_point = NSPoint::new(
            mouse_in_source.x - (final_image_size.width / 2.0),
            mouse_in_source.y + (final_image_size.height / 2.0)  // Adjust for flipped coordinates
        );
        
        // Use the deprecated but working dragImage method
        let slide_back: BOOL = YES;
        let _: () = msg_send![
            source_view,
            dragImage: drag_img
            at: drag_point
            offset: NSPoint::new(0.0, 0.0)
            event: current_event
            pasteboard: pb
            source: source_view
            slideBack: slide_back
        ];
        
        Ok(())
    }
}

fn create_drag_image_from_data_url(data_url: &str) -> Result<id, String> {
    unsafe {
        // Extract base64 data
        let b64 = if let Some(idx) = data_url.find(',') {
            &data_url[(idx + 1)..]
        } else {
            data_url
        };
        
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|_| "Failed to decode base64 image data".to_string())?;
        
        if bytes.is_empty() {
            return Err("Empty image data".to_string());
        }
        
        // Create NSData
        let data: id = msg_send![class!(NSData), alloc];
        let data: id = msg_send![data, initWithBytes: bytes.as_ptr() length: bytes.len()];
        
        // Create NSImage
        let img: id = msg_send![class!(NSImage), alloc];
        let img: id = msg_send![img, initWithData: data];
        
        if img == nil {
            Err("Failed to create NSImage from data".to_string())
        } else {
            Ok(img)
        }
    }
}

fn create_drag_image_from_file_icon(path: &str) -> Result<id, String> {
    unsafe {
        let path_nsstring: id = NSString::alloc(nil).init_str(path);
        let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let icon: id = msg_send![workspace, iconForFile: path_nsstring];
        
        if icon == nil {
            Err("Failed to get file icon".to_string())
        } else {
            Ok(icon)
        }
    }
}

