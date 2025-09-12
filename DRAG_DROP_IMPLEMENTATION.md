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

### Attempt 5: Correct View/Coords + Legacy Only + Multi-Select UI (2025-09-12)

Implementation overview:
- Use the window’s deepest NSView under the mouse via `hitTest:` as the drag source (likely the WKWebView), instead of the window `contentView`.
- Convert `locationInWindow` into that source view’s coordinates before positioning the drag image.
- Upscale tiny system icons by calling `setSize:` (fallback to 128x128) to avoid the tiny/ugly icon.
- Populate only legacy `NSFilenamesPboardType` on `NSDragPboard` to avoid duplicate drops (remove parallel `public.file-url`).
- In the web layer, call `dataTransfer.setDragImage(canvas, ...)` with our composed canvas and apply dimming to all selected items.

Why this helps:
- Using the correct source NSView plus proper coordinate conversion ensures the native drag image actually renders at the cursor instead of defaulting to the small generic ghost.
- Scaling icons + using our composed canvas yields a clean, high‑res preview even when the system icon defaults to 32px.
- Avoiding parallel `public.file-url` entries prevents some apps from duplicating items on drop.

Status: Implemented in `src-tauri/src/drag.rs`, `src/components/FileGrid.tsx`, and `src/components/FileList.tsx`.

Open questions / risks:
- Some apps might still prefer only one of the types; we might need per‑app quirks (rare).
- If a true Finder-like lozenge/stack visual is desired, we likely need `NSDraggingSession` with `NSDraggingItem` composition.

### Attempt 6: Offset Semantics + Signed Offset (2025-09-12)

Implementation tweaks:
- Use `dragImage:at:offset:…` offset parameter instead of baking the shift into the “at” point.
- Signed offsets based on `isFlipped` and syncing HTML5 hotspot with native offset.

Result:
- ⚠ Reduced but not eliminated “flying in” reports; still inconsistent placement (above cursor on some drags).

Assessment:
- The deprecated API is highly sensitive to view flipping and event coordinates; despite pairing offsets and hotspots, visuals remain inconsistent in edge cases.

### Plugin Plan: macOS‑First Native Drag Session (Electron‑style)

Goal: Use modern `beginDraggingSessionWithItems:event:source:` for robust, beautiful Finder‑style visuals while preserving drop compatibility.

Design (mirrors Electron’s `drag_util_mac.mm`):
- Build `NSDraggingItem` per file with `NSURL fileURLWithPath:` as pasteboard writer.
- Synthesize an `NSEvent` (type `NSEventTypeLeftMouseDragged`) using `mouseLocationOutsideOfEventStream` and `NSApp.currentEvent.timestamp`.
- Use the view under the cursor (via `hitTest:`) as the source; set each item’s frame so the preview sits under the cursor.
- Provide `NSDraggingSource` that returns `NSDragOperationCopy` for outside‑app context and `Every` inside.
- For legacy apps, augment the session (future step) with `NSFilenamesPboardType` representations per item to avoid duplication.

Status:
- ✅ Initial scaffold implemented as `start_native_drag_session` (macOS only) in `src-tauri/src/native_drag.rs`.
- 🧪 Not yet wired by default; kept behind a separate command for A/B testing.
 - 🔧 Frontend feature flag: set `localStorage.setItem('enableNativeDragSession','1')` (or `window.MARLIN_NATIVE_DRAG = true`) to route FileGrid/FileList to the native session command. Use `'0'` or delete the key to disable.

### Attempt 7: Native Session Default + HTML5 Drag Suppressed (2025-09-12)

Implementation:
- Switched FileGrid/FileList to call `start_native_drag_session` first; on failure, auto‑fallback to legacy `start_file_drag`.
- Suppressed HTML5 drag handling: call `e.preventDefault()` in `onDragStart` handlers and removed all `dataTransfer`/`setDragImage` logic to avoid the web ghost overlay.
- Added minimal `NSDraggingSource` (Objective‑C) that returns `Copy` outside the app and `Every` inside the app.
- Fixed native crash by using a valid `backingAlignedRect:options:` bitmask (NSAlignAllEdgesOutward).

Code pointers:
- `src-tauri/src/native_drag.rs` — native session (beginDraggingSessionWithItems:event:source:), rect and offsets.
- `src-tauri/src/drag_source.rs` — minimal NSDraggingSource provider (Copy vs Every mask).
- `src/components/FileGrid.tsx`, `src/components/FileList.tsx` — preventDefault() in dragstart, native‑first with legacy fallback.

Result:
- ❌ Still not seeing the macOS native drag preview; behavior appears as non‑native drag.
- ✅ No crash on session start after bitmask fix.
- ✅ No duplicate files when dropping multi‑selection (legacy path).
- ⚠ It is likely the fallback path is taking over in some scenarios, or WKWebView’s internal drag handling is still interfering despite preventDefault.

Hypotheses for failure:
- The native session may be getting rejected inside the dragstart event; Electron synthesizes a drag event and begins the session without relying on the HTML5 event lifecycle, potentially on the next runloop tick.
- The source object might need additional NSDraggingSource selectors implemented for WebKit contexts (e.g., `draggingSourceOperationMaskForLocal:` on older OS versions), or an explicit no‑op for `draggingSession:endedAtPoint:operation:`.
- WKWebView might still perform an internal drag if any element is `draggable=true`; we may need to set `draggable=false` and trigger the native drag from mousedown instead of dragstart.
- The native path could be completing immediately (no visible session) due to misaligned coordinates or frame; we should verify the synthesized NSEvent and frame placement against Electron’s exact code path.

Next steps (high‑confidence):
- Start native drag on mousedown (primary button) with a short `requestAnimationFrame` deferral instead of `dragstart`, and set all items `draggable=false` to prevent web drag entirely.
- Port Electron’s NSEvent synthesis more literally: use `mouseLocationOutsideOfEventStream` and currentEvent timestamp and ensure we pass a valid windowNumber; we already do a version of this, but we should clone their sequencing.
- Expand NSDraggingSource: add `draggingSourceOperationMaskForLocal:` and `draggingSession:endedAtPoint:operation:` stubs to ensure Cocoa routes correctly.
- If native session still fails to show, instrument with temporary logging to detect which path executed (native vs legacy), and log NSEvent, view/class names, and hitTest target to confirm the right NSView is used.
- If certain external apps need legacy pasteboard types, attach `NSFilenamesPboardType` as per‑item representation on NSDraggingItems (not global pasteboard) to avoid duplicates.

Current Status Summary (2025‑09‑12):
- Functionality: Drops work via legacy fallback; no duplicate multi‑file drops.
- Visual: Not reliably seeing the native macOS drag preview yet.
- Stability: No more native crash; native path safely falls back to legacy.

Next plugin steps:
- Add a tiny Objective‑C `NSDraggingSource` object (instead of passing the view) to control allowed operations.
- Provide multi‑file stacking formation and per‑item icons or per‑item previews.
- Attach `NSFilenamesPboardType` per item (not globally) to avoid duplicates while keeping legacy apps working.
- Feature‑flag in the frontend (`enableNativeDragSession`) and document A/B test procedure.

### Proposal: Native Drag Plugin (macOS-first)

Goal: Recreate Electron’s `webContents.startDrag` ergonomics in a Tauri plugin to achieve the nicest native visuals while preserving drop compatibility.

Key points (based on Electron’s `drag_util_mac.mm`):
- Use `beginDraggingSessionWithItems:event:source:` with an array of `NSDraggingItem` built from `NSURL fileURLWithPath:` writers.
- Synthesize an `NSEvent` of type `NSEventTypeLeftMouseDragged` using `mouseLocationOutsideOfEventStream` and `NSApp.currentEvent.timestamp` to reliably start the session (don’t depend on `currentEvent`).
- Provide a minimal Objective‑C `NSDraggingSource` implementation returning `NSDragOperationCopy` for outside‑app and `Every` inside‑app contexts.
- Set the dragging frame and preview per item; optionally apply stacking formation for multi‑file drags.
- For maximum compatibility with older apps, also set `NSFilenamesPboardType` on `NSDragPboard` in parallel (as we do now) right before/after starting the session.

Frontend API:
- `start_native_drag({ paths: string[], previewPng?: string })` that mirrors our current command.
- Optionally support a list of (path, displayName, preview) to control image composition per item.

Fallbacks:
- If `beginDraggingSession…` is unavailable or fails, fall back to our working `dragImage:…` path with both pasteboard types set.

Compatibility plan:
- Test: Finder, VSCode, Bambu Studio, Teams, Slack, Chrome/Safari tabs, Adobe apps.
- If any app rejects the drag when only `public.file-url` is present, ensure the parallel `NSFilenamesPboardType` path is active.

### Attempt 8: Complete Electron-Style Refactor (2025-09-12)

**Goal**: Implement a clean, Electron-style native drag system by removing all legacy implementations and building a single robust solution.

**Implementation**:
- **Removed all existing drag code**: Deleted `drag.rs`, `native_drag.rs`, `drag_source.rs` and old command functions
- **Created unified `native_drag.rs`**: Single implementation using modern `beginDraggingSessionWithItems:event:source:`
- **Proper NSDraggingSource**: Full Objective-C protocol implementation with operation masks for inside/outside app
- **Event synthesis**: Uses `mouseLocationOutsideOfEventStream` + current timestamp like Electron's approach  
- **View selection**: Hit-test to find deepest view under cursor (WKWebView) for proper coordinate handling
- **Multi-file support**: Creates NSDraggingItem per file with stacking formation
- **Frontend simplification**: Removed HTML5 drag, set `draggable=false`, trigger on `mousedown`

**Code Structure**:
- `src-tauri/src/native_drag.rs` - Main implementation with `start_native_drag()` function
- `src-tauri/src/commands.rs` - Single command `start_native_drag` (macOS only)  
- `src/components/FileGrid.tsx` - Updated to use new API, drag on mousedown
- `src/components/FileList.tsx` - Updated to use new API, drag on mousedown

**Initial Testing Results**:
- ✅ **Compilation**: All code compiles successfully (Rust + TypeScript)
- ❌ **Runtime Crash**: App crashes with `NSInvalidArgumentException` when trying to add legacy pasteboard types
- **Root Cause**: `setPropertyList:forType:` called on NSURL object instead of NSPasteboardItem

**Crash Fix Applied**:
- Removed `add_legacy_pasteboard_types_to_item()` function entirely
- Uses only modern NSURL pasteboard writers (no legacy NSFilenamesPboardType)
- Should be compatible with most modern applications

**Current Status** (2025-09-12):
- ✅ No more crashes - app runs successfully
- ✅ **Drag functionality works** - files can be dragged to external applications
- ⚠️ **Visual issues remain** - drag preview animation needs refinement
- ⚠️ May need legacy pasteboard support for older applications

**Animation Issues Identified**:
- **Problem**: Drag preview has a "flying in from above" animation instead of appearing immediately at cursor
- **Behavior**: Preview flashes briefly at cursor position, then animates down from above
- **Attempts Made**:
  - Disabled `setAnimatesToStartingPositionsOnCancelOrFail` (only affects cancel/fail, not initial animation)
  - Adjusted frame positioning to center exactly on cursor
  - Tried different frame sizes and positions
- **Current State**: macOS seems to override our frame position with its own animation logic

**Technical Insights**:
- `NSDraggingItem.setDraggingFrame` position gets overridden by the system
- macOS appears to have built-in logic for where drag animations should originate
- The frame position we set shows briefly but then gets replaced by system animation
- May need different approach entirely (different event timing, different API, etc.)

---

## Next Steps

### Animation Fixes (High Priority)
1. **Investigate macOS drag animation behavior**:
   - Research why macOS overrides our frame position
   - Look into whether we need different timing (delay before starting session?)
   - Check if event synthesis is causing the override
2. **Try alternative approaches**:
   - Consider returning to deprecated `dragImage:at:offset:` method (worked better visually?)
   - Investigate using different source view or coordinate system
   - Try setting frame after session starts rather than before
3. **Deep dive into Electron's implementation**:
   - Find the actual Electron source code for drag_util_mac.mm
   - See exactly how they handle frame positioning and event synthesis

### Compatibility Testing
1. **Test current functionality** with target applications (Finder, VSCode, Bambu Studio, Teams)
2. **Document compatibility results** - which apps work vs fail with NSURL-only approach
3. **Add legacy support if needed** using proper NSPasteboardItem creation for older apps

### Performance & Polish
1. **Performance testing** with large file selections
2. **Multi-file drag improvements** - ensure stacking looks good
3. **Error handling** improvements

---

Last updated: 2025-09-12
