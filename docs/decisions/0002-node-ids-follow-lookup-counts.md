# 0002: A node ID lasts while the OS holds the node

**Status:** proposed
**Date:** 2026-11-11
**Issue:** #370 (epic #368)

This record replaces one row of [0001](0001-mount-architecture.md): the "Identity" row in "What a mount promises on every OS". Everything else in 0001 stands.

## Decision

The core counts who holds each node, the way the FUSE kernel protocol does.

- A node ID stays the same while the OS holds a lookup on the node or a handle is open on it.
- When the OS has forgotten every lookup and closed every handle, the core drops the node.
- If the same item is seen again later, it gets a new node ID. An ID is never reused in a mount.
- A handle's hold is counted apart from the OS's lookups, so a forget can never drop a node that has an open file.

## Why

**0001 promised more than the core can keep.** It said a node ID stays the same for the life of a mount. To keep that, the core must remember every item it has ever shown, for as long as the mount lives. A thumbnailer or a search indexer that walks a large Google Drive would grow that table without limit. Issue #370 requires that the node table is bounded.

**The kernel already works this way.** FUSE tells a filesystem when it forgets a node. 0001's own table lists "Node IDs with lookup and forget counts" as the lookup model on Linux and macOS. The core now keeps what the OS still refers to and nothing more.

**What an app can see.** A file's inode number can change between two looks, but only if the OS dropped the file from its cache in between and nothing had it open. Untested: which apps notice. Tools that compare inode numbers inside one run (`rsync -H`, `tar`, `find`) hold what they compare for a short time, so the risk looks low.

## Rejected

- **Keep every node ID for the life of the mount.** The table has no bound.
- **Derive the node ID from a hash of the provider's item ID.** The core still needs a table to get from a node ID back to the item, so nothing is saved. Two items can also hash to one ID.
- **Cap the table and drop the least recently used node.** The OS may still hold a node the core dropped. Its next call on a live file would fail with a stale-handle error.

## What follows

- **Each adapter must pair every lookup with a forget.** The Linux and macOS adapters (#374, #373) pass the kernel's forget calls through. WinFsp works on paths and never forgets, so the Windows adapter (#375) forgets each node it walked through when the call ends, and keeps only the node of an open file through its handle.
- **A path-based provider gets less.** A provider's item ID should stay with the item when it is renamed. Google Drive's file ID does. SFTP only has paths, so a rename there looks like one item removed and another added.
