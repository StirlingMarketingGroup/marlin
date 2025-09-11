#![allow(unexpected_cfgs)]

#[cfg(target_os = "macos")]
use cocoa::base::{id, nil};
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

    // Prepare pasteboard with both legacy and modern formats
    let pb: id = msg_send![class!(NSPasteboard), generalPasteboard];
    if pb == nil { return Err("No general pasteboard".into()); }
    
    // Clear pasteboard before writing new content
    let _: () = msg_send![pb, clearContents];
    
    // Create array of file paths for legacy NSFilenamesPboardType
    let paths_array: id = msg_send![class!(NSMutableArray), array];
    for p in paths.iter() {
      let nsstr: id = NSString::alloc(nil).init_str(p);
      let _: () = msg_send![paths_array, addObject: nsstr];
    }
    
    // Set legacy NSFilenamesPboardType (required by many apps including Bambu Studio)
    let nsfilenames_type: id = NSString::alloc(nil).init_str("NSFilenamesPboardType");
    let legacy_set: bool = msg_send![pb, setPropertyList: paths_array forType: nsfilenames_type];
    if !legacy_set { return Err("Failed to set legacy pasteboard type".into()); }

    // Create drag image (custom if provided, else tiny transparent)
    let img: id = if let Some(ref data_url) = drag_image_png {
      // Expect data:image/png;base64,.... or raw base64
      let b64 = if let Some(idx) = data_url.find(",") { &data_url[(idx+1)..] } else { data_url.as_str() };
      let bytes = match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(v) => v,
        Err(_) => Vec::new(),
      };
      if !bytes.is_empty() {
        let nsdata: id = msg_send![class!(NSData), dataWithBytes: bytes.as_ptr() length: bytes.len()];
        let img: id = msg_send![class!(NSImage), alloc];
        let img: id = msg_send![img, initWithData: nsdata];
        img
      } else {
        let size = NSSize::new(1.0, 1.0);
        let img: id = msg_send![class!(NSImage), alloc];
        let img: id = msg_send![img, initWithSize: size];
        img
      }
    } else {
      let size = NSSize::new(1.0, 1.0);
      let img: id = msg_send![class!(NSImage), alloc];
      let img: id = msg_send![img, initWithSize: size];
      img
    };

    // Get current mouse location in window coords from the current event
    let ev: id = msg_send![ns_app, currentEvent];
    let mut at: NSPoint = NSPoint::new(10.0, 10.0);
    if ev != nil {
      at = msg_send![ev, locationInWindow];
    }

    // Start a legacy drag session from the view
    let zero = NSSize::new(0.0, 0.0);
    let _: () = msg_send![view,
      dragImage: img
      at: at
      offset: zero
      event: ev
      pasteboard: pb
      source: view
      slideBack: true
    ];

    Ok(())
  }

  #[cfg(not(target_os = "macos"))]
  {
    Err("Native file drag-out is only implemented on macOS currently".into())
  }
}
