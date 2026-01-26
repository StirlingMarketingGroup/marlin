//! Marlin SMB Sidecar
//!
//! This is the entry point for the SMB sidecar binary.
//! It isolates all libsmbclient/pavao usage from the main Marlin app,
//! allowing the main app to start even if libsmbclient is not installed.
//!
//! Communication with the main app uses JSON-RPC 2.0 over stdin/stdout.

fn main() {
    app_lib::smb_sidecar::run();
}
