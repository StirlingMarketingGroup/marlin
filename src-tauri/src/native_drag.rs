#![allow(unexpected_cfgs)]

use cocoa::base::{id, nil, YES, NO};
use cocoa::foundation::{NSAutoreleasePool, NSString, NSPoint, NSRect, NSSize};
use objc::{class, declare::ClassDecl, runtime::{Class, Object, Sel, BOOL}, sel, sel_impl, msg_send};
use base64::Engine as _;

// NSDraggingSource implementation
extern "C" fn dragging_source_operation_mask_for_dragging_context(
    _this: &Object, 
    _sel: Sel, 
    _session: id, 
    context: u64
) -> u64 {
    // NSDraggingContext: 0 = OutsideApplication, 1 = WithinApplication  
    // Return Copy (1) for outside, Every (!0) for inside
    if context == 0 { 1 } else { !0 }
}

extern "C" fn ignore_modifier_keys_while_dragging(_this: &Object, _sel: Sel) -> BOOL {
    YES // Ignore modifier keys for consistent operation
}

extern "C" fn dragging_session_ended_at_point_operation(
    _this: &Object,
    _sel: Sel,
    _session: id,
    _screen_point: NSPoint,
    _operation: u64,
) {
    // Optional cleanup - no-op for our use case
}

fn ensure_drag_source_class() -> *const Class {
    use std::sync::Once;
    static mut CLS: *const Class = std::ptr::null();
    static INIT: Once = Once::new();
    
    INIT.call_once(|| unsafe {
        let superclass = class!(NSObject);
        let mut decl = ClassDecl::new("MarlinNativeDragSource", superclass)
            .expect("Failed to create MarlinNativeDragSource class");
        
        decl.add_method(
            sel!(draggingSession:sourceOperationMaskForDraggingContext:),
            dragging_source_operation_mask_for_dragging_context as extern "C" fn(&Object, Sel, id, u64) -> u64
        );
        decl.add_method(
            sel!(ignoreModifierKeysWhileDragging),
            ignore_modifier_keys_while_dragging as extern "C" fn(&Object, Sel) -> BOOL
        );
        decl.add_method(
            sel!(draggingSession:endedAtPoint:operation:),
            dragging_session_ended_at_point_operation as extern "C" fn(&Object, Sel, id, NSPoint, u64)
        );
        
        let cls = decl.register();
        CLS = cls;
    });
    
    unsafe { CLS }
}

fn create_drag_source() -> id {
    let cls = ensure_drag_source_class();
    unsafe { msg_send![cls, new] }
}

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
            create_drag_image_from_data_url(data_url)?
        } else {
            create_drag_image_from_file_icon(&paths[0])?
        };
        
        let image_size: NSSize = msg_send![drag_img, size];
        
        // Ensure image has a reasonable size for visibility
        let final_image_size = if image_size.width < 64.0 || image_size.height < 64.0 {
            let new_size = NSSize::new(128.0, 128.0);
            let _: () = msg_send![drag_img, setSize: new_size];
            new_size
        } else {
            image_size
        };
        
        // Create NSMutableArray for dragging items
        let dragging_items: id = msg_send![class!(NSMutableArray), array];
        
        for (index, path) in paths.iter().enumerate() {
            // Create NSURL for file
            let path_nsstring: id = NSString::alloc(nil).init_str(path);
            let file_url: id = msg_send![class!(NSURL), fileURLWithPath: path_nsstring];
            
            if file_url == nil {
                continue; // Skip invalid paths
            }
            
            // Create NSDraggingItem with the URL as pasteboard writer
            let dragging_item: id = msg_send![class!(NSDraggingItem), alloc];
            let dragging_item: id = msg_send![dragging_item, initWithPasteboardWriter: file_url];
            
            // Position frame exactly at cursor with proper image size
            // This should minimize or eliminate the flying animation
            let stack_offset = index as f64 * 4.0;
            let frame = NSRect {
                origin: NSPoint::new(
                    mouse_in_source.x - final_image_size.width / 2.0 + stack_offset,
                    mouse_in_source.y - final_image_size.height / 2.0 + stack_offset
                ),
                size: final_image_size,
            };
            
            // Set dragging frame and contents (image)
            let _: () = msg_send![dragging_item, setDraggingFrame: frame contents: drag_img];
            
            // Note: Legacy pasteboard types removed due to API incompatibility  
            // Modern NSURL pasteboard writers should be compatible with most apps
            
            // Add to items array
            let _: () = msg_send![dragging_items, addObject: dragging_item];
        }
        
        // Synthesize NSEvent (like Electron does)
        let current_event: id = msg_send![ns_app, currentEvent];
        let timestamp: f64 = if current_event != nil {
            msg_send![current_event, timestamp]
        } else {
            0.0
        };
        
        let window_number: i64 = msg_send![window, windowNumber];
        
        // Create NSEventTypeLeftMouseDragged (6)
        let drag_event: id = msg_send![
            class!(NSEvent),
            mouseEventWithType: 6u64
            location: mouse_in_source
            modifierFlags: 0u64
            timestamp: timestamp
            windowNumber: window_number
            context: nil
            eventNumber: 0
            clickCount: 1
            pressure: 1.0f64
        ];
        
        if drag_event == nil {
            return Err("Failed to synthesize drag event".into());
        }
        
        // Create drag source
        let drag_source = create_drag_source();
        
        // Begin dragging session
        let session: id = msg_send![
            source_view,
            beginDraggingSessionWithItems: dragging_items
            event: drag_event
            source: drag_source
        ];
        
        // Disable animations to prevent "flying in" effect
        let _: () = msg_send![session, setAnimatesToStartingPositionsOnCancelOrFail: NO];
        
        // Set dragging formation for multiple files (stack formation looks nice)
        if paths.len() > 1 {
            // NSDraggingFormationStack = 1
            let _: () = msg_send![session, setDraggingFormation: 1u64];
        }
        
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

