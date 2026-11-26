# Decisions

A decision record captures one choice that later work depends on: what we chose, why, and what we rejected. Write one when a choice is expensive to reverse, or when the next person would otherwise argue it again.

A wiki page says what the code does today. A decision record says why it is that way. Link from the page to the record.

## Rules

- One file per decision, named `NNNN-short-title.md`, numbered in order from `0001`.
- A record is not edited after it is accepted, except to change its status. A new decision that replaces it gets a new number and says which record it supersedes.
- Add the record to the list below and to [`../index.md`](../index.md) in the same branch.
- Mark anything you could not test as untested. A record never claims more than the evidence shows.

## Shape

```markdown
# NNNN: Title

**Status:** proposed | accepted | superseded by NNNN
**Date:** YYYY-MM-DD
**Issue:** #N

## Decision

What we chose, in a few sentences. Lead with this.

## Why

The problem, the constraints, and the evidence.

## Rejected

Each alternative, and the reason it lost.

## What follows

What this makes easy, what it makes hard, and what work it unblocks.
```

## Records

| Record                                                                                                                     | Decides                                                                                                                                                                                                                     |
| -------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [0001: Mount architecture](0001-mount-architecture.md) (accepted, #369)                                                    | How remotes become native mounts: the portable core, the adapter per OS, the helper process, and the risks.                                                                                                                 |
| [0002: Node IDs follow lookup counts](0002-node-ids-follow-lookup-counts.md) (accepted, #370)                              | How long a mount's node ID lasts. Replaces the Identity row of 0001.                                                                                                                                                        |
| [0003: Google Drive link files](0003-gdrive-link-files.md) (accepted, #527)                                                | How a Google Drive mount shows Google editor files and shortcuts: as small web pages that open them.                                                                                                                        |
| [0004: Real-app tests](0004-real-app-tests.md) (accepted, #497)                                                            | How a test drives the real app: a WebDriver server built into a test build, on macOS and Linux.                                                                                                                             |
| [0005: Mount writes](0005-mount-writes.md) (accepted, #376)                                                                | How a mount takes writes: name changes at once, file bytes through a local spool, uploads after the last close. Replaces the "Extended attributes" row of 0001.                                                             |
| [0006: SMB mounts](0006-smb-mounts.md) (accepted, #612)                                                                    | What an SMB mount is and what it is named, and how the helper runs `marlin-smb` for it: its own processes, up to four, each ended when a call passes its limit.                                                             |
| [0007: Permanent delete](0007-permanent-delete.md) (accepted, #48)                                                         | How a permanent delete runs: four shared threads, a walk that never follows a link and leaves another drive in place, and the open-file limit it needs.                                                                     |
| [0008: Read-only files](0008-read-only-files.md) (accepted, #631)                                                          | How a mount shows a file the account may not change, what the core refuses, and why a refused upload waits. Replaces choice 8 of 0005 for a refused upload.                                                                 |
| [0009: Window on mount paths](0009-window-on-mount-paths.md) (accepted, #377)                                              | How Marlin's own window uses mounts: mount paths in the window, provider addresses as input and the stored form, every account mounted at launch, and no fallback. Replaces the end of the "Rename" row and risk 3 of 0001. |
| [0010: SMB uploads run on their own processes](0010-smb-uploads-run-on-their-own-processes.md) (accepted, #604)            | An SMB mount runs up to four call processes, as 0006 says, plus up to four processes for the two byte calls. Extends choice 4 of 0006.                                                                                      |
| [0011: Read-only folders](0011-read-only-folders.md) (accepted, #639)                                                      | How a mount shows a folder the account may not add to, and what the core refuses there. Extends choice 1 of 0008 to folders.                                                                                                |
| [0012: Empty new files wait for the mount's writers](0012-empty-new-files-wait-for-the-mounts-writers.md) (accepted, #629) | When a new file that holds no bytes uploads: once nothing in the mount has been open for writing for the settle time, so the placeholders Finder removes during a copy stay on this computer. Extends choice 4 of 0005.     |
