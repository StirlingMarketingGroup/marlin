# Drag & Drop Implementation Log

## Problem Statement

We need drag-and-drop functionality that provides both:
1. **Native macOS drag preview** - The beautiful system drag handle that looks perfect
2. **External app compatibility** - Ability to drop files into VSCode, Finder, Bambu Studio, etc.

## Attempted Solutions

### Attempt 1: CrabNebula tauri-plugin-drag (2025-09-11)

**Implementation:**
- Installed `@crabnebula/tauri-plugin-drag` (v2.1.0)
- Added Rust crate `tauri-plugin-drag = "2.1.0"`
- Initialized plugin in `lib.rs`
- Used `startDrag({ item: selected.map(f => f.path) })` in components

**Results:**
- ✅ **Visual:** Perfect native macOS drag preview
- ❌ **Functionality:** Files cannot be dropped into external applications
- ❌ **Issue:** Plugin requires `icon` parameter that we didn't provide
- ❌ **Root Cause:** Plugin likely doesn't set legacy `NSFilenamesPboardType` that external apps expect

**Key Findings:**
- Plugin focuses on modern drag APIs
- Many external apps still require legacy pasteboard types
- Plugin is primarily designed for visual drag operations, not full file transfer

### Attempt 2: Restored Custom Implementation with NSDragPboard (2025-09-11)

**Implementation:**
- Restored custom `drag.rs` with modifications
- Changed from `generalPasteboard` to `pasteboardWithName:NSDragPboard`
- Set both legacy `NSFilenamesPboardType` and modern `public.file-url` types
- Used custom drag image from our existing `createDragImageForSelection`
- Integrated with both FileGrid and FileList components

**Key Technical Changes:**
```rust
// Use NSDragPboard instead of generalPasteboard
let drag_pboard_name: id = NSString::alloc(nil).init_str("NSDragPboard");
let pb: id = msg_send![class!(NSPasteboard), pasteboardWithName: drag_pboard_name];

// Set legacy format (required by many external apps)
let nsfilenames_type: id = NSString::alloc(nil).init_str("NSFilenamesPboardType");
let legacy_set: bool = msg_send![pb, setPropertyList: paths_array forType: nsfilenames_type];

// Also set modern format for compatibility
let file_url_type: id = NSString::alloc(nil).init_str("public.file-url");
```

**Status:** Implemented and tested

**Results:** 
- ✅ **Functionality:** Files can now be dropped into external applications (VSCode, Finder, etc.)
- ❌ **Visual:** Lost native macOS drag preview - no visible drag handle appears
- ❌ **Multi-file:** Multi-file selection drag stopped working properly
- **Issue:** Using `NSDragPboard` directly bypasses the visual drag system

### Attempt 3: Modern NSDraggingSession API (2025-09-11)

**Plan:**
- Replace deprecated `dragImage:at:offset:event:pasteboard:source:slideBack:`
- Use modern `beginDraggingSessionWithItems:event:source:`
- Create proper NSPasteboardItem instances with file data
- Create NSDraggingItem with proper frame and content
- Should provide both native drag preview AND external app functionality

**Implementation Details:**
- Use `NSPasteboardItem` for each file with proper pasteboard types
- Set both `NSFilenamesPboardType` and `public.file-url` types
- Create `NSDraggingItem` with proper frame information
- Use `beginDraggingSessionWithItems` to start modern drag session

**Status:** Implemented and ready for testing

**Key Technical Changes:**
- Replaced deprecated `dragImage:at:offset:event:pasteboard:source:slideBack:`
- Uses modern `beginDraggingSessionWithItems:event:source:`
- Creates individual NSPasteboardItem for each file with multiple format support
- Creates NSDraggingItem with proper drag frame positioning
- Uses NSWorkspace to get file icons for drag preview
- Sets dragging formation for multi-file arrangements

**Results:**
- ❌ **Initial Crash:** App crashed with two errors:
  - `setDraggingFormation:animated:` is not a valid selector (should be property `draggingFormation`)
  - 'NSFilenamesPboardType' is not a valid UTI string (deprecated in modern macOS)
- ✅ **Crash Fixed:** Resolved by:
  - Removing `animated:` parameter from `setDraggingFormation` call
  - Replacing manual NSPasteboardItem creation with NSURL as pasteboard writer
  - Using `msg_send![class!(NSURL), fileURLWithPath: path_nsstring]` directly
- ✅ **Compilation:** App compiles and runs without crashes
- ❌ **Visual Preview:** Drag shows only tiny icon, not proper macOS drag visuals
- ❌ **Functionality:** Files don't drop properly into external apps:
  - Bambu Studio: No response
  - Teams: Drops text only instead of files
- **Issue:** Modern NSDraggingSession with NSURL pasteboard writers doesn't provide legacy compatibility

### Attempt 4: Return to Deprecated dragImage Method (2025-09-11)

**Plan:**
- Use deprecated but functional `dragImage:at:offset:event:pasteboard:source:slideBack:`
- Create proper NSDragPboard with NSFilenamesPboardType
- Set array of file paths using `setPropertyList:forType:`
- Use custom drag image for beautiful macOS drag preview
- Should provide both native visuals AND external app functionality

**Implementation Details:**
- Switch from `beginDraggingSessionWithItems` to `dragImage:at:offset:event:pasteboard:source:slideBack:`
- Use `pasteboardWithName:NSDragPboard` for proper drag pasteboard
- Declare `NSFilenamesPboardType` and set paths array with `setPropertyList`
- Create drag image from provided base64 data or file icon
- Set proper drag position and offset

**Status:** Implemented and tested

**Results:**
- ✅ **Functionality:** Dragging works! Files can be dropped into external applications
- ❌ **Visual:** Still showing ugly/small icon, not the nice macOS drag preview  
- **Partial Success:** Achieved functionality but still missing beautiful visuals
- **Issue:** Even with deprecated dragImage method, we're not getting the native macOS drag aesthetics

## Key Technical Insights

1. **Legacy Compatibility Required:** Many external apps (Bambu Studio, older versions of apps) require `NSFilenamesPboardType`
2. **Pasteboard Selection Matters:** Using drag-specific pasteboard vs general pasteboard affects behavior
3. **Plugin Limitations:** Third-party plugins may prioritize modern APIs over legacy compatibility
4. **Native vs Web:** HTML5 drag works for web compatibility but loses native system preview

## Next Steps

1. Implement modified drag.rs
2. Test with multiple external applications
3. Document specific compatibility results
4. Consider hybrid approaches if needed

---

*Last updated: 2025-09-11*