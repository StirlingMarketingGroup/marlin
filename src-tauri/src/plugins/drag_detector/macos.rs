use cocoa::appkit::{NSViewHeightSizable, NSViewWidthSizable, NSWindowOrderingMode};
use cocoa::base::{id, nil, NO, YES};
use cocoa::foundation::{NSArray, NSAutoreleasePool, NSPoint, NSRect, NSString};
use objc::class;
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel, BOOL};
use objc::{msg_send, sel, sel_impl};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::ffi::{c_void, CStr};
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Runtime, Window};

use super::{DragDropEvent, DragEventType, DropLocation, DropZoneConfig};

type DragHandler = dyn Fn(DragDropEvent) + 'static;

type NSDragOperation = u64;
const NS_DRAG_OPERATION_NONE: NSDragOperation = 0;
const NS_DRAG_OPERATION_COPY: NSDragOperation = 1;

#[derive(Debug, Clone, Default)]
struct DropZoneState {
    enabled: bool,
    width: Option<f64>,
}

static DROP_ZONES: Lazy<Mutex<HashMap<String, DropZoneState>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static INITIALIZED_WINDOWS: Lazy<Mutex<HashMap<String, bool>>> = Lazy::new(|| Mutex::new(HashMap::new()));

static DRAG_OVERLAY_CLASS: Lazy<&'static Class> = Lazy::new(|| {
    let superclass = class!(NSView);
    let mut decl = ClassDecl::new("MarlinDragOverlayView", superclass).expect("create overlay class");

    unsafe {
        decl.add_ivar::<*mut c_void>("event_handler_ptr");

        decl.add_method(
            sel!(draggingEntered:),
            dragging_entered as extern "C" fn(&Object, Sel, id) -> NSDragOperation,
        );

        decl.add_method(
            sel!(draggingUpdated:),
            dragging_updated as extern "C" fn(&Object, Sel, id) -> NSDragOperation,
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

        // Ensure normal pointer events pass through to the webview
        decl.add_method(
            sel!(hitTest:),
            hit_test as extern "C" fn(&Object, Sel, NSPoint) -> id,
        );

        decl.add_method(
            sel!(wantsPeriodicDraggingUpdates),
            wants_periodic_dragging_updates as extern "C" fn(&Object, Sel) -> BOOL,
        );
    }

    decl.register()
});

extern "C" fn dragging_entered(this: &Object, _sel: Sel, sender: id) -> NSDragOperation {
    match catch_unwind(AssertUnwindSafe(|| unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        let (event, target_id) = compose_event(sender, DragEventType::DragEnter);
        emit_event(this, event);
        drag_operation_for_target(&target_id)
    })) {
        Ok(op) => op,
        Err(_) => {
            log::error!("dragging_entered panicked");
            NS_DRAG_OPERATION_NONE
        }
    }
}

extern "C" fn dragging_updated(this: &Object, _sel: Sel, sender: id) -> NSDragOperation {
    match catch_unwind(AssertUnwindSafe(|| unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        let (event, target_id) = compose_event(sender, DragEventType::DragOver);
        emit_event(this, event);
        drag_operation_for_target(&target_id)
    })) {
        Ok(op) => op,
        Err(_) => {
            log::error!("dragging_updated panicked");
            NS_DRAG_OPERATION_NONE
        }
    }
}

extern "C" fn dragging_exited(this: &Object, _sel: Sel, sender: id) {
    if let Err(_) = catch_unwind(AssertUnwindSafe(|| unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        let (event, _) = compose_event(sender, DragEventType::DragLeave);
        emit_event(this, event);
    })) {
        log::error!("dragging_exited panicked");
    }
}

extern "C" fn prepare_for_drag_operation(_this: &Object, _sel: Sel, _sender: id) -> BOOL {
    YES
}

extern "C" fn perform_drag_operation(this: &Object, _sel: Sel, sender: id) -> BOOL {
    match catch_unwind(AssertUnwindSafe(|| unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        let (event, target_id) = compose_event(sender, DragEventType::Drop);

        if !is_target_enabled(&target_id) {
            return NO;
        }

        emit_event(this, event);
        YES
    })) {
        Ok(result) => result,
        Err(_) => {
            log::error!("perform_drag_operation panicked");
            NO
        }
    }
}

extern "C" fn hit_test(_this: &Object, _sel: Sel, _point: NSPoint) -> id {
    // Allow underlying views to handle normal events while still receiving drag callbacks
    nil
}

extern "C" fn wants_periodic_dragging_updates(_this: &Object, _sel: Sel) -> BOOL {
    YES
}

unsafe fn get_drag_location(sender: id) -> NSPoint {
    msg_send![sender, draggingLocation]
}

unsafe fn get_dragged_paths(sender: id) -> Vec<String> {
    let pasteboard: id = msg_send![sender, draggingPasteboard];
    if pasteboard == nil {
        return Vec::new();
    }

    let url_class_ptr: *const Class = class!(NSURL);
    let url_class_obj: id = url_class_ptr as *const _ as *mut Object;
    let class_array: id = NSArray::arrayWithObject(nil, url_class_obj);
    let urls: id = msg_send![pasteboard, readObjectsForClasses: class_array options: nil];

    let mut paths = Vec::new();

    if urls != nil {
        let count: usize = msg_send![urls, count];
        for i in 0..count {
            let url: id = msg_send![urls, objectAtIndex: i];
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

fn resolve_target(point: NSPoint) -> Option<String> {
    if point.x < 0.0 || point.y < 0.0 {
        return None;
    }

    let zones = DROP_ZONES.lock().unwrap();

    if let Some(zone) = zones.get("sidebar") {
        if zone.enabled {
            let width = zone.width.unwrap_or(280.0);
            // Allow a small tolerance to smooth minor rounding differences.
            if point.x <= width + 1.0 {
                return Some("sidebar".to_string());
            }
        }
    }

    if let Some(zone) = zones.get("file-grid") {
        if zone.enabled {
            return Some("file-grid".to_string());
        }
    }

    None
}

fn drag_operation_for_target(target: &Option<String>) -> NSDragOperation {
    if is_target_enabled(target) {
        NS_DRAG_OPERATION_COPY
    } else {
        NS_DRAG_OPERATION_NONE
    }
}

fn is_target_enabled(target: &Option<String>) -> bool {
    match target {
        Some(id) => DROP_ZONES
            .lock()
            .unwrap()
            .get(id)
            .map(|zone| zone.enabled)
            .unwrap_or(false),
        None => false,
    }
}

fn emit_event(this: &Object, event: DragDropEvent) {
    unsafe {
        let handler_ptr = *this.get_ivar::<*mut c_void>("event_handler_ptr");
        if handler_ptr.is_null() {
            return;
        }

        let handler_arc_ptr = handler_ptr as *mut Arc<DragHandler>;
        let handler = &*handler_arc_ptr;
        handler(event);
    }
}

unsafe fn compose_event(sender: id, event_type: DragEventType) -> (DragDropEvent, Option<String>) {
    let point = get_drag_location(sender);
    let paths = get_dragged_paths(sender);
    let target_id = match event_type {
        DragEventType::DragLeave => None,
        _ => resolve_target(point),
    };

    (
        DragDropEvent {
            paths,
            location: DropLocation {
                x: point.x,
                y: point.y,
                target_id: target_id.clone(),
            },
            event_type,
        },
        target_id,
    )
}

fn install_overlay<R: Runtime>(window: Window<R>) -> Result<(), String> {
    unsafe {
        let ns_window_ptr = window
            .ns_window()
            .map_err(|e| e.to_string())?;
        let ns_window: id = ns_window_ptr as id;
        let content_view: id = msg_send![ns_window, contentView];
        if content_view == nil {
            return Err("No content view available".into());
        }

        let bounds: NSRect = msg_send![content_view, bounds];

        let overlay: id = msg_send![*DRAG_OVERLAY_CLASS, alloc];
        let overlay: id = msg_send![overlay, initWithFrame: bounds];
        let _: () = msg_send![overlay, setAlphaValue: 0.0];
        let _: () = msg_send![overlay, setHidden: NO];
        let _: () = msg_send![overlay, setAutoresizingMask: NSViewWidthSizable | NSViewHeightSizable];

        let handler_arc: Arc<DragHandler> = Arc::new({
            let window_clone = window.clone();
            move |event: DragDropEvent| {
                log::debug!(
                    "drag detector event: {:?} target={:?} ({}, {})",
                    event.event_type,
                    event.location.target_id,
                    event.location.x,
                    event.location.y
                );

                let event_clone = event.clone();
                let emit_window = window_clone.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(err) = emit_window.emit("drag-drop-event", event_clone) {
                        log::warn!("Failed to emit drag-drop-event: {err}");
                    }
                });
            }
        });

        let handler_box = Box::new(handler_arc);
        let handler_ptr = Box::into_raw(handler_box) as *mut c_void;
        let overlay_obj = overlay as *mut Object;
        (*overlay_obj).set_ivar("event_handler_ptr", handler_ptr);

        let ns_array: &Class = class!(NSArray);
        let types: id = msg_send![ns_array, arrayWithObject: NSString::alloc(nil).init_str("public.file-url")];
        let _: () = msg_send![overlay, registerForDraggedTypes: types];

        let _: () = msg_send![content_view, addSubview: overlay positioned: NSWindowOrderingMode::NSWindowAbove relativeTo: nil];
    }

    Ok(())
}

pub fn setup_drag_handlers<R: Runtime>(window: &Window<R>) -> Result<(), String> {
    let label = window.label().to_string();
    {
        let mut initialized = INITIALIZED_WINDOWS.lock().unwrap();
        if initialized.get(&label).copied().unwrap_or(false) {
            return Ok(());
        }
        initialized.insert(label, true);
    }

    let result: Arc<Mutex<Result<(), String>>> = Arc::new(Mutex::new(Ok(())));
    let result_clone = Arc::clone(&result);
    let window_clone = window.clone();

    window
        .run_on_main_thread(move || {
            let install_result = install_overlay(window_clone);
            if let Ok(mut guard) = result_clone.lock() {
                *guard = install_result;
            }
        })
        .map_err(|e| e.to_string())?;

    result
        .lock()
        .map(|guard| guard.clone())
        .map_err(|_| "Failed to acquire drag detector result".to_string())?
}

pub fn set_drop_zone(zone_id: &str, enabled: bool, config: Option<DropZoneConfig>) {
    let mut zones = DROP_ZONES.lock().unwrap();
    let entry = zones.entry(zone_id.to_string()).or_default();
    entry.enabled = enabled;
    if let Some(cfg) = config {
        entry.width = cfg.width;
    }
}
