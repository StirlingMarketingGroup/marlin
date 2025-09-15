# Alternative Drag & Drop Solution

Since the full native plugin implementation is complex, here's a simpler interim solution that works TODAY:

## Immediate Solution: Context Menu

Instead of complex drag detection, let's add a "Pin to Sidebar" option to the context menu:

### Implementation

1. **Add to Context Menu** (in FileGrid/FileList):
```typescript
// When right-clicking a directory
if (file.is_directory) {
  menuItems.push({
    label: 'Pin to Sidebar',
    icon: <PushPin />,
    action: () => addPinnedDirectory(file.path)
  })
}
```

2. **Keyboard Shortcut** (optional):
- Select directory
- Press Cmd+P (or Ctrl+P) to pin

3. **Drag Handle Alternative**:
- Add a small pin icon next to each directory
- Click the pin to add to sidebar
- Visual and discoverable

## Benefits
- Works immediately without plugin complexity
- Cross-platform compatible
- No Tauri limitations
- Simple to implement and maintain

## Future Enhancement
The drag detector plugin structure is in place and compiles. To fully implement it:

1. **Complete macOS implementation**: 
   - Properly register window as drop target
   - Handle coordinate conversions
   - Emit events to JavaScript

2. **Add Windows support**:
   - Use IDropTarget COM interface
   - Register window for OLE drag/drop

3. **Add Linux support**:
   - Use GTK drag-and-drop signals
   - Handle X11/Wayland differences

## Recommendation
Use the context menu approach NOW while the plugin is refined in the background. This gives users the functionality immediately while avoiding the complexity of native drag detection.