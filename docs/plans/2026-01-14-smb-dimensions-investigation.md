# SMB Thumbnail Dimensions Investigation

## Problem Summary

Two issues with SMB directories:

1. **Dimensions not displaying** - SMB thumbnails load correctly, but dimensions (e.g., "1920×1080") don't appear under images in grid view
2. **Auto-grid not triggering** - Media-heavy SMB folders should auto-switch to grid view (75%+ media files), but all SMB dirs open in list view

## What Works

- Google Drive dimensions work correctly (fixed in previous session)
- E2E tests pass with mocks - frontend code is correct
- SMB thumbnails DO load and display
- Local file dimensions work correctly

## Investigation Done

### Debug Logging Added

**`src/hooks/useThumbnail.ts`** - Added logging at lines 339-364:

- `[THUMB RESPONSE]` - Logs raw response from backend for remote files
- `[THUMB] Calling updateFileDimensions` - Logs when dimensions are passed to store
- `[THUMB] NO DIMENSIONS in response` - Logs when response has null dimensions

**`src/store/useAppStore.ts`** - Added logging in `updateFileDimensions`:

- `[STORE] updateFileDimensions: FILE NOT FOUND` - If path doesn't match any file in state
- `[STORE] updateFileDimensions: UPDATING` - When dimensions successfully update

### E2E Tests Created

- `e2e/dimensions.spec.ts` - 4 tests for dimension persistence, all pass with mocks
- `e2e/tauri-mocks.ts` - Updated with `request_thumbnail` mock that returns dimensions

## Where to Look Next

### Most Likely Causes

1. **Backend not returning dimensions for SMB** - Check if SMB thumbnail generation path differs from local files
   - Look at: `src-tauri/src/thumbnails/generators/images.rs`
   - Look at: `src-tauri/src/thumbnails/cache.rs` - cached items might not return dimensions

2. **Path mismatch** - The path in `updateFileDimensions` might not match files in state
   - SMB paths use `smb://server/share/path` format
   - Check if the path from thumbnail response matches the path in file list

3. **Cache returning stale data** - Cached thumbnails might not include dimensions
   - Check `CacheEntry` struct in cache.rs
   - Verify dimensions are stored AND returned when cache hits

### Files to Investigate

```
src-tauri/src/thumbnails/
├── mod.rs           - ThumbnailResponse struct definition
├── cache.rs         - Check if dimensions stored/returned for cached items
├── generators/
│   └── images.rs    - ImageGenerator::generate returns dimensions correctly
└── service.rs       - Orchestrates generation, might have SMB-specific path
```

### For Auto-Grid Issue

Check `src/App.tsx` lines 170-183:

- Auto-grid logic checks if 75%+ files are media
- For SMB, uses `listing.location.path` instead of `listing.location.raw`
- May need to verify SMB listings trigger this code path

## How to Debug

1. Run the app and navigate to an SMB directory with images
2. Open browser DevTools console
3. Look for these log patterns:
   - `[THUMB RESPONSE]` shows `width: null, height: null` → Backend issue
   - `[THUMB] NO DIMENSIONS in response` appears → Confirm backend issue
   - `[STORE] updateFileDimensions: FILE NOT FOUND` → Path mismatch issue
   - No logs at all → Effect not running or thumbnailUrl path being used

## Quick Commands

```bash
# Run the app
npm run tauri dev

# Run e2e tests
npm run test:e2e

# Check Rust builds
cd src-tauri && cargo build
```
