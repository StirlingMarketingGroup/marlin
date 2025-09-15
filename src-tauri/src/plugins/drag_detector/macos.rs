use cocoa::base::{id, nil, YES, NO};
use cocoa::foundation::{NSArray, NSString, NSPoint, NSAutoreleasePool};
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel, BOOL};
use objc::class;
use objc::{msg_send, sel, sel_impl};
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::{Arc, Mutex};
use tauri::{Runtime, Window};
use once_cell::sync::Lazy;

use super::{DragDropEvent, DragEventType, DropLocation};

static DROP_ZONES: Lazy<Arc<Mutex<HashMap<String, bool>>>> = 
    Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));

static DRAG_DELEGATE_CLASS: Lazy<&'static Class> = Lazy::new(|| {
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("MarlinDragDelegate", superclass).unwrap();
    
    unsafe {
        decl.add_ivar::<*mut c_char>("window_ptr");
        
        decl.add_method(
            sel!(draggingEntered:),
            dragging_entered as extern "C" fn(&Object, Sel, id) -> BOOL,
        );
        
        decl.add_method(
            sel!(draggingUpdated:),
            dragging_updated as extern "C" fn(&Object, Sel, id) -> BOOL,
        );
        
        decl.add_method(
            sel!(draggingExited:),
            dragging_exited as extern "C" fn(&Object, Sel, id),
        );
        
        decl.add_method(
            sel!(performDragOperation:),
            perform_drag_operation as extern "C" fn(&Object, Sel, id) -> BOOL,
        );
        
        decl.add_method(
            sel!(prepareForDragOperation:),
            prepare_for_drag_operation as extern "C" fn(&Object, Sel, id) -> BOOL,
        );
    }
    
    decl.register()
});

extern "C" fn dragging_entered(this: &Object, _sel: Sel, sender: id) -> BOOL {
    unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        
        let point = get_drag_location(sender);
        let paths = get_dragged_paths(sender);
        
        if let Some(window) = get_window_from_delegate(this) {
            let event = DragDropEvent {
                paths,
                location: DropLocation {
                    x: point.x,
                    y: point.y,
                    target_id: get_target_at_point(point),
                },
                event_type: DragEventType::DragEnter,
            };
            
            emit_to_window(window, event);
        }
        
        YES
    }
}

extern "C" fn dragging_updated(this: &Object, _sel: Sel, sender: id) -> BOOL {
    unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        
        let point = get_drag_location(sender);
        let paths = get_dragged_paths(sender);
        
        if let Some(window) = get_window_from_delegate(this) {
            let event = DragDropEvent {
                paths,
                location: DropLocation {
                    x: point.x,
                    y: point.y,
                    target_id: get_target_at_point(point),
                },
                event_type: DragEventType::DragOver,
            };
            
            emit_to_window(window, event);
        }
        
        YES
    }
}

extern "C" fn dragging_exited(this: &Object, _sel: Sel, sender: id) {
    unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        
        let point = get_drag_location(sender);
        let paths = get_dragged_paths(sender);
        
        if let Some(window) = get_window_from_delegate(this) {
            let event = DragDropEvent {
                paths,
                location: DropLocation {
                    x: point.x,
                    y: point.y,
                    target_id: None,
                },
                event_type: DragEventType::DragLeave,
            };
            
            emit_to_window(window, event);
        }
    }
}

extern "C" fn prepare_for_drag_operation(_this: &Object, _sel: Sel, _sender: id) -> BOOL {
    YES
}

extern "C" fn perform_drag_operation(this: &Object, _sel: Sel, sender: id) -> BOOL {
    unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        
        let point = get_drag_location(sender);
        let paths = get_dragged_paths(sender);
        
        if let Some(window) = get_window_from_delegate(this) {
            let target_id = get_target_at_point(point);
            
            if let Some(ref target) = target_id {
                let zones = DROP_ZONES.lock().unwrap();
                if !zones.get(target).unwrap_or(&false) {
                    return NO;
                }
            }
            
            let event = DragDropEvent {
                paths,
                location: DropLocation {
                    x: point.x,
                    y: point.y,
                    target_id,
                },
                event_type: DragEventType::Drop,
            };
            
            emit_to_window(window, event);
            YES
        } else {
            NO
        }
    }
}

unsafe fn get_drag_location(sender: id) -> NSPoint {
    let window: id = msg_send![sender, draggingDestinationWindow];
    let screen_point: NSPoint = msg_send![sender, draggingLocation];
    let window_point: NSPoint = msg_send![window, convertPointFromScreen:screen_point];
    
    let content_view: id = msg_send![window, contentView];
    let view_point: NSPoint = msg_send![content_view, convertPoint:window_point fromView:nil];
    
    view_point
}

unsafe fn get_dragged_paths(sender: id) -> Vec<String> {
    let pasteboard: id = msg_send![sender, draggingPasteboard];
    let types: id = msg_send![class!(NSArray), arrayWithObject:NSString::alloc(nil).init_str("public.file-url")];
    let urls: id = msg_send![pasteboard, readObjectsForClasses:types options:nil];
    
    let mut paths = Vec::new();
    
    if urls != nil {
        let count: usize = msg_send![urls, count];
        for i in 0..count {
            let url: id = msg_send![urls, objectAtIndex:i];
            let path: id = msg_send![url, path];
            let c_str: *const c_char = msg_send![path, UTF8String];
            if !c_str.is_null() {
                if let Ok(path_str) = CStr::from_ptr(c_str).to_str() {
                    paths.push(path_str.to_string());
                }
            }
        }
    }
    
    paths
}

fn get_target_at_point(point: NSPoint) -> Option<String> {
    if point.x < 250.0 {
        Some("sidebar".to_string())
    } else {
        Some("file-grid".to_string())
    }
}

unsafe fn get_window_from_delegate(delegate: &Object) -> Option<id> {
    let window_ptr: *mut c_char = *delegate.get_ivar("window_ptr");
    if window_ptr.is_null() {
        None
    } else {
        Some(window_ptr as id)
    }
}

fn emit_to_window(window: id, event: DragDropEvent) {
    println!("Emitting drag event: {:?}", event);
}

pub fn setup_drag_handlers<R: Runtime>(window: &Window<R>) -> Result<(), String> {
    // For now, just return Ok - full implementation would require
    // more complex window handle management
    Ok(())
}

pub fn set_drop_zone(zone_id: &str, enabled: bool) {
    let mut zones = DROP_ZONES.lock().unwrap();
    zones.insert(zone_id.to_string(), enabled);
}