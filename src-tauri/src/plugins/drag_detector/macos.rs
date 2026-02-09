use objc2::class;
use objc2::ffi::NSUInteger;
use objc2::msg_send;
use objc2::rc::autoreleasepool;
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::sel;
use objc2_app_kit::NSAutoresizingMaskOptions;
use objc2_foundation::{NSPoint, NSRect, NSString};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::ffi::{c_void, CStr};
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Runtime, Window};

use super::{DragDropEvent, DragEventType, DropLocation, DropZoneConfig, DragModifiers};

type DragHandler = dyn Fn(DragDropEvent) + 'static;

type NSDragOperation = NSUInteger;
const NS_DRAG_OPERATION_NONE: NSDragOperation = 0;
const NS_DRAG_OPERATION_COPY: NSDragOperation = 1;

const NS_EVENT_MODIFIER_FLAG_OPTION: NSUInteger = 1 << 19;
const NS_EVENT_MODIFIER_FLAG_COMMAND: NSUInteger = 1 << 20;

type Id = *mut AnyObject;

// Static CStr for ivar name to avoid repeated parsing on hot path
static IVAR_NAME: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"event_handler_ptr\0") };

#[derive(Debug, Clone, Default)]
struct DropZoneState {
    enabled: bool,
    width: Option<f64>,
}

static DROP_ZONES: Lazy<Mutex<HashMap<String, DropZoneState>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static INITIALIZED_WINDOWS: Lazy<Mutex<HashMap<String, bool>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

static DRAG_OVERLAY_CLASS: Lazy<&'static AnyClass> = Lazy::new(|| {
    let superclass = class!(NSView);
    let name = CStr::from_bytes_with_nul(b"MarlinDragOverlayView\0")
        .expect("valid overlay class name");
    let mut decl = ClassBuilder::new(name, superclass).expect("create overlay class");

    unsafe {
        decl.add_ivar::<*mut c_void>(IVAR_NAME);

        let dragging_entered_fn: extern "C-unwind" fn(_, _, _) -> NSDragOperation =
            dragging_entered;
        decl.add_method(sel!(draggingEntered:), dragging_entered_fn);

        let dragging_updated_fn: extern "C-unwind" fn(_, _, _) -> NSDragOperation =
            dragging_updated;
        decl.add_method(sel!(draggingUpdated:), dragging_updated_fn);

        let dragging_exited_fn: extern "C-unwind" fn(_, _, _) = dragging_exited;
        decl.add_method(sel!(draggingExited:), dragging_exited_fn);

        let perform_drag_operation_fn: extern "C-unwind" fn(_, _, _) -> Bool =
            perform_drag_operation;
        decl.add_method(sel!(performDragOperation:), perform_drag_operation_fn);

        let prepare_for_drag_operation_fn: extern "C-unwind" fn(_, _, _) -> Bool =
            prepare_for_drag_operation;
        decl.add_method(sel!(prepareForDragOperation:), prepare_for_drag_operation_fn);

        // Ensure normal pointer events pass through to the webview
        let hit_test_fn: extern "C-unwind" fn(_, _, _) -> Id = hit_test;
        decl.add_method(sel!(hitTest:), hit_test_fn);

        let wants_periodic_dragging_updates_fn: extern "C-unwind" fn(_, _) -> Bool =
            wants_periodic_dragging_updates;
        decl.add_method(sel!(wantsPeriodicDraggingUpdates), wants_periodic_dragging_updates_fn);

        // Add dealloc to clean up the event handler and prevent memory leaks
        let dealloc_fn: extern "C-unwind" fn(_, _) = overlay_dealloc;
        decl.add_method(sel!(dealloc), dealloc_fn);
    }

    decl.register()
});

/// Dealloc method to free the event handler when the overlay is deallocated
extern "C-unwind" fn overlay_dealloc(this: Id, _sel: Sel) {
    unsafe {
        if let Some(ivar) = (*this).class().instance_variable(IVAR_NAME) {
            let handler_ptr = *ivar.load::<*mut c_void>(&*this);
            if !handler_ptr.is_null() {
                // Reclaim the Box to drop the Arc<DragHandler>
                let _ = Box::from_raw(handler_ptr as *mut Arc<DragHandler>);
            }
        }
        // Call superclass dealloc
        let superclass = class!(NSView);
        let _: () = msg_send![super(this, superclass), dealloc];
    }
}

extern "C-unwind" fn dragging_entered(this: Id, _sel: Sel, sender: Id) -> NSDragOperation {
    match catch_unwind(AssertUnwindSafe(|| {
        autoreleasepool(|_| unsafe {
            let (event, target_id) = compose_event(sender, DragEventType::DragEnter);
            emit_event(&*this, event);
            drag_operation_for_target(&target_id)
        })
    })) {
        Ok(op) => op,
        Err(_) => {
            log::error!("dragging_entered panicked");
            NS_DRAG_OPERATION_NONE
        }
    }
}

extern "C-unwind" fn dragging_updated(this: Id, _sel: Sel, sender: Id) -> NSDragOperation {
    match catch_unwind(AssertUnwindSafe(|| {
        autoreleasepool(|_| unsafe {
            let (event, target_id) = compose_event(sender, DragEventType::DragOver);
            emit_event(&*this, event);
            drag_operation_for_target(&target_id)
        })
    })) {
        Ok(op) => op,
        Err(_) => {
            log::error!("dragging_updated panicked");
            NS_DRAG_OPERATION_NONE
        }
    }
}

extern "C-unwind" fn dragging_exited(this: Id, _sel: Sel, sender: Id) {
    if let Err(_) = catch_unwind(AssertUnwindSafe(|| {
        autoreleasepool(|_| unsafe {
            let (event, _) = compose_event(sender, DragEventType::DragLeave);
            emit_event(&*this, event);
        })
    })) {
        log::error!("dragging_exited panicked");
    }
}

extern "C-unwind" fn prepare_for_drag_operation(
    _this: Id,
    _sel: Sel,
    _sender: Id,
) -> Bool {
    Bool::YES
}

extern "C-unwind" fn perform_drag_operation(
    this: Id,
    _sel: Sel,
    sender: Id,
) -> Bool {
    match catch_unwind(AssertUnwindSafe(|| {
        autoreleasepool(|_| unsafe {
            let (event, target_id) = compose_event(sender, DragEventType::Drop);

            if !is_target_enabled(&target_id) {
                return Bool::NO;
            }

            emit_event(&*this, event);
            Bool::YES
        })
    })) {
        Ok(result) => result,
        Err(_) => {
            log::error!("perform_drag_operation panicked");
            Bool::NO
        }
    }
}

extern "C-unwind" fn hit_test(_this: Id, _sel: Sel, _point: NSPoint) -> Id {
    // Allow underlying views to handle normal events while still receiving drag callbacks
    std::ptr::null_mut()
}

extern "C-unwind" fn wants_periodic_dragging_updates(_this: Id, _sel: Sel) -> Bool {
    Bool::YES
}

unsafe fn get_drag_location(sender: Id) -> NSPoint {
    msg_send![sender, draggingLocation]
}

unsafe fn get_dragged_paths(sender: Id) -> Vec<String> {
    let pasteboard: Id = msg_send![sender, draggingPasteboard];
    if pasteboard.is_null() {
        return Vec::new();
    }

    let url_class_ptr: &AnyClass = class!(NSURL);
    let class_array: Id = msg_send![class!(NSArray), arrayWithObject: url_class_ptr];
    let urls: Id = msg_send![
        pasteboard,
        readObjectsForClasses: class_array,
        options: std::ptr::null_mut::<AnyObject>()
    ];

    let mut paths = Vec::new();

    if !urls.is_null() {
        let count: usize = msg_send![urls, count];
        for i in 0..count {
            let url: Id = msg_send![urls, objectAtIndex: i];
            let path: Id = msg_send![url, path];
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

unsafe fn get_drag_modifiers() -> DragModifiers {
    let app: Id = msg_send![class!(NSApplication), sharedApplication];
    if app.is_null() {
        return DragModifiers {
            option_alt: false,
            cmd_ctrl: false,
        };
    }
    let event: Id = msg_send![app, currentEvent];
    if event.is_null() {
        return DragModifiers {
            option_alt: false,
            cmd_ctrl: false,
        };
    }
    let flags: NSUInteger = msg_send![event, modifierFlags];

    DragModifiers {
        option_alt: (flags & NS_EVENT_MODIFIER_FLAG_OPTION) != 0,
        cmd_ctrl: (flags & NS_EVENT_MODIFIER_FLAG_COMMAND) != 0,
    }
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

    if let Some(zone) = zones.get("file-panel") {
        if zone.enabled {
            return Some("file-panel".to_string());
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

fn emit_event(this: &AnyObject, event: DragDropEvent) {
    unsafe {
        let ivar = this
            .class()
            .instance_variable(IVAR_NAME)
            .expect("overlay ivar missing");
        let handler_ptr = *ivar.load::<*mut c_void>(this);
        if handler_ptr.is_null() {
            return;
        }

        let handler_arc_ptr = handler_ptr as *mut Arc<DragHandler>;
        let handler = &*handler_arc_ptr;
        handler(event);
    }
}

unsafe fn compose_event(sender: Id, event_type: DragEventType) -> (DragDropEvent, Option<String>) {
    let point = get_drag_location(sender);
    let paths = get_dragged_paths(sender);
    let target_id = match event_type {
        DragEventType::DragLeave => None,
        _ => resolve_target(point),
    };
    let modifiers = get_drag_modifiers();

    (
        DragDropEvent {
            paths,
            location: DropLocation {
                x: point.x,
                y: point.y,
                target_id: target_id.clone(),
            },
            event_type,
            modifiers,
        },
        target_id,
    )
}

fn install_overlay<R: Runtime>(window: Window<R>) -> Result<(), String> {
    unsafe {
        let ns_window_ptr = window.ns_window().map_err(|e| e.to_string())?;
        let ns_window: Id = ns_window_ptr as Id;
        let content_view: Id = msg_send![ns_window, contentView];
        if content_view.is_null() {
            return Err("No content view available".into());
        }

        let bounds: NSRect = msg_send![content_view, bounds];

        let overlay: Id = msg_send![*DRAG_OVERLAY_CLASS, alloc];
        let overlay: Id = msg_send![overlay, initWithFrame: bounds];
        let _: () = msg_send![overlay, setAlphaValue: 0.0];
        let _: () = msg_send![overlay, setHidden: Bool::NO];
        let _: () =
            msg_send![overlay, setAutoresizingMask: NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable];

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
        let overlay_obj = overlay as *mut AnyObject;
        let ivar = (*overlay_obj)
            .class()
            .instance_variable(IVAR_NAME)
            .expect("overlay ivar missing");
        let slot = ivar.load_mut::<*mut c_void>(&mut *overlay_obj);
        *slot = handler_ptr;

        let types: Id = msg_send![
            class!(NSArray),
            arrayWithObject: &*NSString::from_str("public.file-url")
        ];
        let _: () = msg_send![overlay, registerForDraggedTypes: types];

        let _: () = msg_send![
            content_view,
            addSubview: overlay,
            positioned: 1i64,
            relativeTo: std::ptr::null_mut::<AnyObject>()
        ];
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
