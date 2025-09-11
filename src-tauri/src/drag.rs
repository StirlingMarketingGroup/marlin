#![allow(unexpected_cfgs)]

#[cfg(target_os = "macos")]
use cocoa::base::{id, nil, YES};
use base64::Engine as _;
#[cfg(target_os = "macos")]
use cocoa::foundation::{NSAutoreleasePool, NSString, NSPoint, NSSize};
#[cfg(target_os = "macos")]
use objc::{class, msg_send, sel, sel_impl};

pub fn start_file_drag(paths: Vec<String>, drag_image_png: Option<String>) -> Result<(), String> {
  #[cfg(target_os = "macos")]
  unsafe {
    if paths.is_empty() { return Err("No paths provided".into()); }
    let _pool: id = NSAutoreleasePool::new(nil);

    // Get key window and its content view as the drag source
    let ns_app: id = msg_send![class!(NSApplication), sharedApplication];
    if ns_app == nil { return Err("NSApplication unavailable".into()); }
    let window: id = msg_send![ns_app, keyWindow];
    if window == nil { return Err("No key window".into()); }
    let view: id = msg_send![window, contentView];
    if view == nil { return Err("No content view".into()); }

    // Get current event for starting the drag
    let ev: id = msg_send![ns_app, currentEvent];
    if ev == nil { return Err("No current event".into()); }

    // Create NSDragPboard for drag operation
    let drag_pboard_name: id = NSString::alloc(nil).init_str("NSDragPboard");
    let pb: id = msg_send![class!(NSPasteboard), pasteboardWithName: drag_pboard_name];
    if pb == nil { return Err("Failed to create NSDragPboard".into()); }

    // Clear the pasteboard and declare types
    let _: () = msg_send![pb, declareTypes: nil owner: nil];
    
    // Create NSArray with file paths
    let paths_array: id = msg_send![class!(NSMutableArray), array];
    for path in paths.iter() {
      let path_nsstring: id = NSString::alloc(nil).init_str(path);
      let _: () = msg_send![paths_array, addObject: path_nsstring];
    }

    // Set NSFilenamesPboardType with file paths array
    let nsfilenames_type: id = NSString::alloc(nil).init_str("NSFilenamesPboardType");
    let types_array: id = msg_send![class!(NSArray), arrayWithObject: nsfilenames_type];
    let _: () = msg_send![pb, declareTypes: types_array owner: nil];
    let legacy_set: bool = msg_send![pb, setPropertyList: paths_array forType: nsfilenames_type];
    
    if !legacy_set {
      return Err("Failed to set NSFilenamesPboardType".into());
    }

    // Create drag image
    let drag_img: id = if let Some(ref data_url) = drag_image_png {
      // Use custom image if provided
      let b64 = if let Some(idx) = data_url.find(",") { &data_url[(idx+1)..] } else { data_url.as_str() };
      let bytes = match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(v) => v,
        Err(_) => Vec::new(),
      };
      if !bytes.is_empty() {
        let data: id = msg_send![class!(NSData), alloc];
        let data: id = msg_send![data, initWithBytes: bytes.as_ptr() length: bytes.len()];
        let img: id = msg_send![class!(NSImage), alloc];
        let img: id = msg_send![img, initWithData: data];
        if img != nil { img } else {
          // Fallback to file icon
          let path_nsstring: id = NSString::alloc(nil).init_str(&paths[0]);
          let ws: id = msg_send![class!(NSWorkspace), sharedWorkspace];
          msg_send![ws, iconForFile: path_nsstring]
        }
      } else {
        // Fallback to file icon
        let path_nsstring: id = NSString::alloc(nil).init_str(&paths[0]);
        let ws: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        msg_send![ws, iconForFile: path_nsstring]
      }
    } else {
      // Use file icon from system
      let path_nsstring: id = NSString::alloc(nil).init_str(&paths[0]);
      let ws: id = msg_send![class!(NSWorkspace), sharedWorkspace];
      msg_send![ws, iconForFile: path_nsstring]
    };

    if drag_img == nil {
      return Err("Failed to create drag image".into());
    }

    // Get drag image size and mouse location
    let image_size: NSSize = msg_send![drag_img, size];
    let mouse_location: NSPoint = msg_send![ev, locationInWindow];
    
    // Calculate drag position (center the image at mouse location)
    let drag_position = NSPoint::new(
      mouse_location.x - image_size.width / 2.0,
      mouse_location.y - image_size.height / 2.0
    );

    // Use deprecated but working dragImage method
    let _: () = msg_send![view,
      dragImage: drag_img
      at: drag_position
      offset: NSSize::new(0.0, 0.0)
      event: ev
      pasteboard: pb
      source: view
      slideBack: YES
    ];

    Ok(())
  }

  #[cfg(not(target_os = "macos"))]
  {
    Err("Native file drag-out is only implemented on macOS currently".into())
  }
}