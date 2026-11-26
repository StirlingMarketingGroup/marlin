# 0010: SMB uploads run on their own processes

**Status:** accepted
**Date:** 2026-11-25
**Issue:** #604

## Decision

An SMB mount runs up to `PROCESSES` (4) `marlin-smb` processes for its calls, as [0006](0006-smb-mounts.md) says, plus up to `UPLOAD_PROCESSES` (4) processes for the two byte calls, so up to eight. The provider gives both numbers to the core (`max_calls` and `max_uploads`). An upload has no time limit ([0005](0005-mount-writes.md) choice 4) and ends its process when the core stops it.

## Why

One `marlin-smb` process runs one call at a time. The core takes no call slot for an upload, so uploads on the call processes would leave a list or a read waiting behind a long upload. A slow upload on one share must not block calls on another (#604).

## Rejected

- **One shared pool of four.** Uploads would starve calls.
- **An upload pool sized from the helper's default `max_uploads`.** The provider's process count and the core's upload limit would be two numbers that can drift apart. An upload waiting for a process sends no progress, and the core ends it as stalled.

## What follows

A mount runs up to eight `marlin-smb` processes. Changing either number needs no new record.
