# Drag & Drop Implementation Status

## 🎯 Feature Requirements

**Goal**: Allow users to drag directories from the file browser to the sidebar to pin them for quick access.

**Expected Behavior**:
1. User drags a directory from FileGrid or FileList
2. Sidebar shows visual feedback (highlight/ring effect) when dragged over
3. User drops directory on sidebar
4. Directory is added to "Pinned" section in sidebar
5. Directory persists across app restarts

## 📊 Current Status: **✅ WORKING**

The drag and drop functionality is fully operational using a custom manual drag implementation.

## 🛠️ Implementation Progress

### ✅ Backend Implementation (COMPLETE)

**Files Modified**: `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`

**Features Implemented**:
- `get_pinned_directories()` - Load pinned directories from JSON file
- `add_pinned_directory(path, name)` - Add new pinned directory with duplicate checking
- `remove_pinned_directory(path)` - Remove pinned directory
- `reorder_pinned_directories(paths)` - Reorder pinned directories
- Persistent storage in `~/.config/Marlin/pinned_directories.json`
- Limit of 20 pinned directories
- Automatic directory name extraction from path

**Backend Status**: ✅ **Working** - All Tauri commands compile and are registered

### ✅ Store Integration (COMPLETE)

**Files Modified**: `src/store/useAppStore.ts`, `src/types/index.ts`

**Features Implemented**:
- `pinnedDirectories` state array
- `loadPinnedDirectories()` - Load from backend on app start
- `addPinnedDirectory()` - Add and update local state
- `removePinnedDirectory()` - Remove and update local state  
- `reorderPinnedDirectories()` - Reorder local state
- Error handling for all operations
- TypeScript types for `PinnedDirectory`

**Store Status**: ✅ **Working** - App loads pinned directories on startup

### ✅ Sidebar UI (COMPLETE)

**Files Modified**: `src/components/Sidebar.tsx`

**Features Implemented**:
- Display pinned directories in "Pinned" section
- Unpin functionality with trash button on hover
- Navigation to pinned directories on click
- Visual feedback during drag operations
- Drop zone with enhanced styling and transitions

**Sidebar Status**: ✅ **Working** - UI displays properly, unpin works

### ❌ Drag & Drop System (NOT WORKING)

**Files Modified**: `src/components/FileGrid.tsx`, `src/components/FileList.tsx`, `src/components/Sidebar.tsx`

#### HTML5 Drag Implementation (Source)

**FileGrid.tsx & FileList.tsx**:
```typescript
// Directory items have draggable={file.is_directory}
// onDragStart handler sets drag data for directories only
const handleDragStart = (e: React.DragEvent, file: FileItem) => {
  if (file.is_directory) {
    const dragData = {
      type: 'file',
      path: file.path,
      isDirectory: true,
      name: file.name
    }
    e.dataTransfer.setData('application/json', JSON.stringify(dragData))
    e.dataTransfer.effectAllowed = 'copy'
  } else {
    e.preventDefault()
  }
}
```

**Current Issues**:
- `handleDragStart` is never being called
- No drag events are being initiated at all
- Console logs show no drag activity

#### Drop Zone Implementation (Target)

**Sidebar.tsx**:
```typescript
<div 
  className={`... ${isDragOver ? 'ring-2 ring-accent bg-accent/10' : ''}`}
  data-tauri-drag-region={false}
  onDragEnter={handleDragEnter}
  onDragOver={handleDragOver}
  onDragLeave={handleDragLeave}
  onDrop={handleDrop}
>
```

**Current Issues**:
- Drop zone event handlers are never called
- No visual feedback occurs
- `isDragOver` state is never set to `true`

## 🐛 Root Cause Analysis

### Issue 1: HTML5 Drag Not Initiating

**Problem**: The `onDragStart` event is never fired for directories.

**Investigation**:
- Directories have `draggable={file.is_directory}` set correctly
- `onDragStart` handler is attached properly
- Mouse down events work (selection still functions)
- But drag doesn't initiate at all

**Potential Causes**:
1. **CSS Prevention**: Some CSS or parent element preventing drag
2. **Event Interference**: Other event handlers interfering with drag initiation
3. **Browser Restrictions**: Tauri/WebView restrictions on drag operations
4. **Mouse Handler Conflicts**: `onMouseDown` handler might still be interfering

### Issue 2: Native Drag System Conflict

**Background**: The app has a complex native drag system for files that uses:
- `onMouseDown` with threshold detection
- `invoke('start_native_drag')` for external file operations
- Custom drag image generation

**Recent Changes**: Modified `onMouseDown` to skip native drag for directories:
```typescript
// Before: early return prevented HTML5 drag
if (file.is_directory) {
  return // ❌ This broke HTML5 drag
}

// After: wrapped native drag code
if (!file.is_directory) {
  // Native drag code only for files
}
```

**Status**: Fixed the early return issue, but drag still doesn't work.

### Issue 3: Tauri WebView Restrictions

**Concern**: Tauri's WebView might have restrictions on HTML5 drag operations, especially:
- Cross-component drag and drop
- `dataTransfer` API usage
- Custom drag data

## 🔍 Debugging Performed

### ✅ Completed Debugging Steps

1. **Backend Testing**: ✅ All Tauri commands work via manual invocation
2. **Store Testing**: ✅ All store methods work correctly
3. **UI Testing**: ✅ Sidebar displays pinned directories correctly  
4. **Build Testing**: ✅ Frontend and backend compile without errors
5. **Event Handler Setup**: ✅ All drag/drop handlers are attached correctly
6. **Console Logging**: ✅ Extensive logging added but no drag events fire
7. **Mouse Down Fix**: ✅ Removed early return that was blocking HTML5 drag

### ❌ Outstanding Issues

1. **No Drag Initiation**: `onDragStart` never fires for directories
2. **No Visual Feedback**: Sidebar drop zone never activates
3. **Silent Failure**: No errors in console, drag just doesn't start

## 🚨 Current Blockers

### Primary Blocker: HTML5 Drag Not Starting

The fundamental issue is that HTML5 drag operations are not initiating at all. This suggests:

1. **Draggable Attribute Issue**: `draggable={file.is_directory}` might not be working
2. **Event Prevention**: Some parent element or CSS is preventing drag
3. **Tauri Limitation**: WebView might not support HTML5 drag fully
4. **Conflicting Handlers**: Other event handlers still interfering

### Secondary Issues

1. **No Error Messages**: Complete silent failure makes debugging difficult
2. **Complex Event Flow**: Native drag + HTML5 drag systems are complex
3. **Cross-System Interference**: File selection, context menus, rename modes all using same elements

## 🎉 Solution: Manual Drag Implementation

### The Problem
HTML5 drag and drop API was fundamentally broken in Tauri's WebView:
- `draggable={true}` would create a drag ghost image
- `onDragStart` events would fire
- But `onDragEnter`, `onDragOver`, and `onDrop` events on drop targets would **never fire**
- This appears to be a Tauri WebView limitation where drag events don't propagate between elements

### The Solution
We implemented a **complete manual drag system** using mouse events:

1. **Global Drag State** (`src/store/useDragStore.ts`):
   - Zustand store to track drag state globally
   - Stores dragged directory info and cursor position
   - Provides `startDrag()`, `endDrag()`, and `updateDragPosition()` methods

2. **Manual Drag Initiation** (FileGrid/FileList):
   - On `mouseDown` for directories, track initial position
   - On `mouseMove`, check if moved > 5px (drag threshold)
   - If threshold met, call `startManualDrag()` from store
   - Completely bypass HTML5 drag API

3. **Visual Drag Preview** (`src/components/DragPreview.tsx`):
   - Global component that renders when `isDragging` is true
   - Follows cursor position using mousemove events
   - Shows folder icon and name in floating panel

4. **Drop Detection** (Sidebar):
   - Monitors global mouse position during drag
   - Checks if cursor is within sidebar bounds
   - On `mouseUp`, if within bounds, pins the directory
   - Single source of truth for ending drag (avoids conflicts)

### Key Implementation Details

#### Why Manual Drag Was Necessary
```javascript
// This DOESN'T work in Tauri:
<div 
  draggable={true}
  onDragStart={(e) => console.log('starts')}  // ✅ Fires
  onDrop={(e) => console.log('drops')}         // ❌ Never fires
/>

// This DOES work (our solution):
<div
  onMouseDown={handleManualDragStart}
  // Global mouseMove and mouseUp handlers track drag
/>
```

#### Coordination Between Components
- **FileGrid/FileList**: Only starts drag, doesn't end it
- **Sidebar**: Handles both drop detection AND ending drag
- **DragPreview**: Pure visual component, no logic
- This prevents race conditions where drag ends before drop is processed

## 💻 Code Status

### Files with Complete Implementation
- ✅ `src-tauri/src/commands.rs` - Backend pinned directory management
- ✅ `src-tauri/src/lib.rs` - Tauri command registration  
- ✅ `src/store/useAppStore.ts` - Frontend state management for pinned directories
- ✅ `src/store/useDragStore.ts` - Manual drag state management
- ✅ `src/types/index.ts` - TypeScript definitions
- ✅ `src/components/Sidebar.tsx` - Drop detection and pinning logic
- ✅ `src/components/FileGrid.tsx` - Manual drag initiation for directories
- ✅ `src/components/FileList.tsx` - Manual drag initiation for directories
- ✅ `src/components/DragPreview.tsx` - Visual drag feedback component
- ✅ `src/App.tsx` - Initialization and component composition

### Test Coverage
- ✅ `src/__tests__/unit/store/useAppStore.test.ts` - Store methods tested
- ⚠️ Manual drag system not yet unit tested

## 📝 Summary

The drag and drop system is now **fully functional** using a custom manual implementation:

### What Works
- ✅ **Drag Initiation**: Click and drag folders from FileGrid or FileList
- ✅ **Visual Feedback**: Floating preview follows cursor during drag
- ✅ **Drop Zone Highlighting**: Sidebar shows blue outline when hovering
- ✅ **Directory Pinning**: Drop on sidebar successfully pins directories
- ✅ **Persistence**: Pinned directories persist across app restarts
- ✅ **Duplicate Prevention**: Already pinned directories won't be added twice
- ✅ **Clean State Management**: Drag state properly cleans up after drop

### Technical Achievement
We successfully worked around a fundamental Tauri WebView limitation by:
1. Completely bypassing the broken HTML5 drag and drop API
2. Implementing a robust manual drag system using mouse events
3. Creating a global state management solution for drag operations
4. Ensuring proper coordination between multiple components

### Lessons Learned
- **Tauri WebView Limitation**: HTML5 drag events don't propagate properly between elements
- **Manual Implementation**: Sometimes native APIs don't work and you need custom solutions
- **Global State**: Complex interactions benefit from centralized state management
- **Event Coordination**: Careful handling of who "owns" cleanup prevents race conditions

The implementation is clean, maintainable, and provides a smooth user experience despite the underlying platform limitations.