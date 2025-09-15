# Drag & Drop Implementation Status

## 🎯 Feature Requirements

**Goal**: Allow users to drag directories from the file browser to the sidebar to pin them for quick access, while maintaining the ability to drag directories to external applications.

**Expected Behavior**:
1. User drags a directory from FileGrid or FileList
2. Directory can be dragged to external applications (like Finder, Terminal, etc.)
3. Sidebar shows visual feedback (highlight/ring effect) when dragged over
4. User drops directory on sidebar to pin it
5. Directory is added to "Pinned" section in sidebar
6. Directory persists across app restarts

## 📊 Current Status: **❌ NOT WORKING**

Despite multiple implementation attempts, the drag and drop functionality for pinning directories to the sidebar is still not functioning properly.

## 🔍 Research Findings

### Tauri Drag & Drop Limitations Discovered

1. **HTML5 Drag Events in Tauri WebView**:
   - `dragstart` events fire properly
   - `dragenter`, `dragover`, and `drop` events **do NOT fire reliably between elements**
   - This is a known limitation of Tauri's WebView implementation
   - Setting `dragDropEnabled: false` in window config can help on Windows but breaks native drag

2. **Native Drag (`start_native_drag`)**:
   - Successfully allows dragging files/directories to external applications
   - **Takes over mouse control completely** - browser doesn't receive mouse events during drag
   - Returns only after drag completes (blocking operation)
   - Cannot easily detect drops within the same application

3. **Platform-Specific Issues**:
   - Linux/WebKit has known issues with drag and drop (GitHub issue #6695)
   - Windows requires disabling native drag/drop to use HTML5 drag/drop
   - macOS native drag preview animations have issues (see `DRAG_DROP_IMPLEMENTATION.md` in git history)

## 🛠️ Implementation History

### ✅ Backend Implementation (COMPLETE)

**Files**: `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`

**Features Implemented**:
- `get_pinned_directories()` - Load pinned directories from JSON file
- `add_pinned_directory(path, name)` - Add new pinned directory with duplicate checking
- `remove_pinned_directory(path)` - Remove pinned directory
- `reorder_pinned_directories(paths)` - Reorder pinned directories
- Persistent storage in `~/.config/Marlin/pinned_directories.json`

**Status**: ✅ Working - Backend commands function correctly

### ✅ Store Integration (COMPLETE)

**Files**: `src/store/useAppStore.ts`, `src/types/index.ts`

**Features Implemented**:
- `pinnedDirectories` state array
- `loadPinnedDirectories()` - Load from backend on app start
- `addPinnedDirectory()` - Add and update local state
- `removePinnedDirectory()` - Remove and update local state
- TypeScript types for `PinnedDirectory`

**Status**: ✅ Working - Store methods function correctly

### ✅ Sidebar UI (PARTIALLY WORKING)

**File**: `src/components/Sidebar.tsx`

**Features Implemented**:
- Display pinned directories in "Pinned" section ✅
- Unpin functionality with trash button ✅
- Navigation to pinned directories ✅
- Visual feedback during drag ❌ (not showing)
- Drop zone detection ❌ (not working)

**Status**: ⚠️ Display works, but drop detection fails

## 📝 Attempted Solutions

### Attempt 1: Manual Drag Implementation (Initial Commit)
**Approach**: Created a complete manual drag system using mouse events
- Tracked drag state in `useDragStore`
- Created `DragPreview` component for visual feedback
- Used mouse position tracking for drop detection

**Result**: 
- ✅ Pinning to sidebar worked
- ❌ **Broke dragging to external applications**
- ❌ Complex implementation with many edge cases

### Attempt 2: Restore Native Drag (Current Attempt #1)
**Approach**: Use native drag for everything, track directories for pinning
- Restored native drag for directories
- Added lightweight tracking in `useDragStore`
- Tried to detect drops via mouse position during native drag

**Result**:
- ✅ External application dragging works
- ❌ **No drop detection** - mouse events blocked during native drag
- ❌ **No visual feedback** - sidebar doesn't highlight

### Attempt 3: Global Mouse Tracking (Current Attempt #2)
**Approach**: Track mouse position globally, check after native drag completes
- Added global mouse position tracking in App.tsx
- Check sidebar bounds after `start_native_drag` returns
- Poll mouse position for visual feedback

**Implementation**:
```typescript
// In App.tsx - track mouse globally
useEffect(() => {
  const handleMouseMove = (e: MouseEvent) => {
    (window as any).lastMouseX = e.clientX;
    (window as any).lastMouseY = e.clientY;
  }
  document.addEventListener('mousemove', handleMouseMove)
}, [])

// In FileGrid/FileList - check after drag
await invoke('start_native_drag', {...})
// Check if mouse is over sidebar
const sidebar = document.querySelector('[data-sidebar="true"]')
if (sidebar && mouseOverSidebar) {
  await addPinnedDirectory(file.path)
}
```

**Result**:
- ✅ External application dragging still works
- ❌ **Drop detection still doesn't work** - mouse position not reliable after native drag
- ❌ **No visual feedback** - polling during native drag doesn't work

## 🐛 Root Cause Analysis

### Why Current Implementation Fails

1. **Native Drag Blocks Everything**:
   - When `start_native_drag` is called, it takes complete control
   - No browser events fire until drag completes
   - Mouse position tracking is unreliable during native drag
   - The final mouse position after drag may not reflect drop location

2. **HTML5 Drag is Broken in Tauri**:
   - Even with `draggable={true}`, drop events don't fire between elements
   - This appears to be a fundamental Tauri WebView limitation
   - Cannot use HTML5 drag for internal drops

3. **No Hybrid Solution Works**:
   - Can't use HTML5 for internal + native for external (events conflict)
   - Can't detect drops during native drag (no events)
   - Can't reliably detect drops after native drag (position unreliable)

## 🚧 Current Blockers

1. **Fundamental Limitation**: Tauri doesn't support detecting drops within the app during native drag
2. **No Mouse Events**: Can't track mouse or detect hover during native drag
3. **No HTML5 Alternative**: HTML5 drag events broken in Tauri WebView
4. **Position Unreliable**: Final mouse position after native drag doesn't indicate drop location

## 💡 Potential Solutions to Explore

### Option 1: Two-Phase Drag
- On mouse down, show a "mode selector" 
- User chooses "Pin to Sidebar" or "Drag to App"
- Different drag behavior based on selection
- **Pros**: Would work reliably
- **Cons**: Poor UX, extra step for users

### Option 2: Modifier Keys
- Hold Cmd/Ctrl while dragging to pin to sidebar (uses manual drag)
- Normal drag for external apps (uses native drag)
- **Pros**: Power user friendly
- **Cons**: Not discoverable, requires documentation

### Option 3: Right-Click Menu
- Add "Pin to Sidebar" to context menu
- Keep drag only for external apps
- **Pros**: Simple, reliable
- **Cons**: Not as intuitive as drag and drop

### Option 4: Custom Tauri Plugin
- Write a Rust plugin to detect drops at the OS level
- Bridge native drop detection back to JavaScript
- **Pros**: Could provide full functionality
- **Cons**: Complex, platform-specific, maintenance burden

### Option 5: Drag Handle/Zone
- Add a specific drag handle/icon that uses manual drag (for pinning)
- Rest of the item uses native drag (for external)
- **Pros**: Both features available
- **Cons**: UI complexity, user confusion

## 📊 Comparison with Native File Managers

| Feature | Finder (macOS) | Explorer (Windows) | Our App |
|---------|---------------|-------------------|---------|
| Drag to external apps | ✅ | ✅ | ✅ |
| Drag to sidebar | ✅ | ✅ | ❌ |
| Visual feedback | ✅ | ✅ | ❌ |
| Drop zones | ✅ | ✅ | ❌ |

## 🎯 Recommendation

Given the technical limitations discovered, I recommend:

### Primary Solution: Custom Tauri Plugin (In Progress)
Implement **Option 4** - Build a native drag detection plugin

**Status**: Plugin foundation created and compiling. Needs platform-specific implementation completion.

### Alternative Solutions (Not Preferred)
- **Context Menu**: Would work but we want to keep the context menu clean and minimal
- **Modifier Keys**: Not discoverable enough for users
- **Two-Phase Drag**: Poor UX with extra steps

### Current Focus: Complete the Custom Plugin
Investigate **Option 4** - Build a native drag detection plugin

**Technical Approach**:
1. **Create Rust Plugin** that hooks into OS-level drag events:
   - macOS: Use `NSDraggingDestination` protocol to detect drops
   - Windows: Use `IDropTarget` COM interface
   - Linux: Use GTK drag-and-drop signals

2. **Bridge to JavaScript**:
   ```rust
   // Rust side - detect drop and emit event
   fn handle_native_drop(window: Window, paths: Vec<String>, drop_location: DropLocation) {
     window.emit("native-drop", DropPayload {
       paths,
       target: drop_location, // e.g., "sidebar", "file-list", etc.
     });
   }
   ```

3. **Handle in Frontend**:
   ```typescript
   // JavaScript side - listen for native drops
   listen('native-drop', (event) => {
     if (event.payload.target === 'sidebar') {
       addPinnedDirectory(event.payload.paths[0]);
     }
   });
   ```

**Benefits**:
- Full native drag and drop support
- Works with external apps AND internal drops
- Proper visual feedback and drop zones
- Native performance and reliability

**Considerations**:
- Requires platform-specific implementations
- Adds maintenance complexity
- Could be contributed back to Tauri core
- Would benefit entire Tauri community

**Resources**:
- [Tauri Plugin Development Guide](https://tauri.app/v1/guides/features/plugin)
- [macOS NSDraggingDestination](https://developer.apple.com/documentation/appkit/nsdraggingdestination)
- [Windows IDropTarget](https://docs.microsoft.com/en-us/windows/win32/api/oleidl/nn-oleidl-idroptarget)
- [GTK Drag and Drop](https://docs.gtk.org/gtk3/drag-and-drop.html)

## 📝 Lessons Learned

1. **Tauri Limitations**: Not all web APIs work as expected in Tauri
2. **Native vs Web**: Can't easily mix native and web drag behaviors
3. **Research First**: Should have investigated Tauri limitations before implementing
4. **Test Early**: Simple prototypes would have revealed issues sooner
5. **Document Everything**: This document is valuable for future reference

## 🔗 Related Documentation

- `DRAG_DROP_IMPLEMENTATION.md` (in git history) - Detailed native drag implementation attempts
- [Tauri GitHub Issue #6695](https://github.com/tauri-apps/tauri/issues/6695) - Linux drag/drop issues
- [Tauri Window Config](https://tauri.app/v1/api/config/#windowconfig) - `dragDropEnabled` setting

---

**Last Updated**: 2024-12-19
**Status**: Blocked by Tauri limitations
**Next Steps**: Implement context menu workaround or investigate custom plugin