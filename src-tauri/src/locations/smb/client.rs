//! SMB sidecar client for IPC communication.
//!
//! This module handles spawning and communicating with the marlin-smb sidecar process.
//! The sidecar is a separate binary that handles all SMB operations, isolating
//! libsmbclient from the main app.

use once_cell::sync::Lazy;
use serde::{de::DeserializeOwned, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Default timeout for RPC calls (30 seconds).
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Extended timeout for file downloads (5 minutes).
#[allow(dead_code)]
pub const DOWNLOAD_TIMEOUT_MS: u64 = 300_000;

/// Maximum number of sidecar restart attempts.
const MAX_RESTART_ATTEMPTS: u32 = 3;

/// JSON-RPC request ID counter.
static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Sidecar availability status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidecarStatus {
    /// Sidecar is available and working.
    Available,
    /// Sidecar binary not found in app bundle.
    NotInstalled,
    /// libsmbclient library is missing (dyld failure).
    LibraryMissing,
    /// Sidecar failed to start for another reason.
    StartFailed(String),
}

impl SidecarStatus {
    /// Returns a user-friendly error message.
    pub fn error_message(&self) -> Option<String> {
        match self {
            SidecarStatus::Available => None,
            SidecarStatus::NotInstalled => {
                Some("SMB support not available. The marlin-smb helper is missing.".to_string())
            }
            SidecarStatus::LibraryMissing => {
                Some("SMB support requires Samba.\nmacOS: brew install samba\nLinux: sudo apt install libsmbclient-dev".to_string())
            }
            SidecarStatus::StartFailed(reason) => Some(format!("SMB sidecar failed to start: {}", reason)),
        }
    }
}

/// Sidecar process state.
struct SidecarProcess {
    #[allow(dead_code)]
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// Global sidecar state.
struct SidecarState {
    process: Option<SidecarProcess>,
    status: SidecarStatus,
    restart_attempts: u32,
}

static SIDECAR: Lazy<Mutex<SidecarState>> = Lazy::new(|| {
    Mutex::new(SidecarState {
        process: None,
        status: SidecarStatus::NotInstalled,
        restart_attempts: 0,
    })
});

/// Initialize the sidecar if not already running.
/// Returns the current status.
pub fn initialize() -> SidecarStatus {
    let mut state = SIDECAR.lock().expect("Sidecar mutex poisoned");

    // If already running and available, return success
    if state.process.is_some() && state.status == SidecarStatus::Available {
        return SidecarStatus::Available;
    }

    // Try to start the sidecar
    state.status = start_sidecar(&mut state);
    state.status.clone()
}

/// Get the current sidecar status without starting it.
#[allow(dead_code)]
pub fn get_status() -> SidecarStatus {
    let state = SIDECAR.lock().expect("Sidecar mutex poisoned");
    state.status.clone()
}

/// Check if the sidecar is available.
pub fn is_available() -> bool {
    let state = SIDECAR.lock().expect("Sidecar mutex poisoned");
    state.status == SidecarStatus::Available && state.process.is_some()
}

/// Call a method on the sidecar with the default timeout.
pub fn call_method<P: Serialize, R: DeserializeOwned>(
    method: &str,
    params: P,
) -> Result<R, String> {
    call_method_with_timeout(method, params, DEFAULT_TIMEOUT_MS)
}

/// Call a method on the sidecar with a custom timeout.
pub fn call_method_with_timeout<P: Serialize, R: DeserializeOwned>(
    method: &str,
    params: P,
    _timeout_ms: u64,
) -> Result<R, String> {
    let mut state = SIDECAR.lock().expect("Sidecar mutex poisoned");

    // Ensure sidecar is running
    if state.process.is_none() || state.status != SidecarStatus::Available {
        // Try to restart
        if state.restart_attempts >= MAX_RESTART_ATTEMPTS {
            return Err(state.status.error_message().unwrap_or_else(|| {
                "SMB sidecar is not available".to_string()
            }));
        }
        state.status = start_sidecar(&mut state);
        if state.status != SidecarStatus::Available {
            return Err(state.status.error_message().unwrap_or_else(|| {
                "SMB sidecar failed to start".to_string()
            }));
        }
    }

    let process = state.process.as_mut().ok_or("Sidecar not running")?;

    // Build request
    let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": serde_json::to_value(&params).map_err(|e| format!("Failed to serialize params: {}", e))?
    });

    // Write request
    let request_line = serde_json::to_string(&request)
        .map_err(|e| format!("Failed to serialize request: {}", e))?;

    if let Err(e) = writeln!(process.stdin, "{}", request_line) {
        // Sidecar might have crashed, mark as unavailable and try to restart
        log::warn!("Failed to write to sidecar stdin: {}", e);
        state.process = None;
        state.restart_attempts += 1;
        return Err("SMB connection lost. Please try again.".to_string());
    }

    if let Err(e) = process.stdin.flush() {
        log::warn!("Failed to flush sidecar stdin: {}", e);
        state.process = None;
        state.restart_attempts += 1;
        return Err("SMB connection lost. Please try again.".to_string());
    }

    // Read response
    // TODO: Implement actual timeout using select/poll or a separate thread
    let mut response_line = String::new();
    if let Err(e) = process.stdout.read_line(&mut response_line) {
        log::warn!("Failed to read from sidecar stdout: {}", e);
        state.process = None;
        state.restart_attempts += 1;
        return Err("SMB connection lost. Please try again.".to_string());
    }

    // Parse response
    let response: serde_json::Value = serde_json::from_str(&response_line)
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    // Validate response ID matches request ID
    let response_id = response
        .get("id")
        .and_then(|id| id.as_u64())
        .ok_or("Response missing or invalid id field")?;
    if response_id != id {
        log::error!(
            "Response ID mismatch: expected {}, got {}",
            id,
            response_id
        );
        return Err("SMB protocol error: response ID mismatch".to_string());
    }

    // Check for error
    if let Some(error) = response.get("error") {
        let message = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown error");
        return Err(message.to_string());
    }

    // Extract result
    let result = response
        .get("result")
        .ok_or("Response missing result field")?
        .clone();

    serde_json::from_value(result).map_err(|e| format!("Failed to parse result: {}", e))
}

/// Start the sidecar process.
fn start_sidecar(state: &mut SidecarState) -> SidecarStatus {
    // Find the sidecar binary
    let binary_path = match find_sidecar_binary() {
        Some(path) => path,
        None => {
            log::warn!("SMB sidecar binary not found");
            return SidecarStatus::NotInstalled;
        }
    };

    log::info!("Starting SMB sidecar from: {}", binary_path.display());

    // Ensure the binary has execute permissions (may be stripped during bundling or updates)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(&binary_path) {
            let mode = metadata.permissions().mode();
            if mode & 0o100 == 0 {
                let mut new_perms = metadata.permissions();
                new_perms.set_mode(mode | 0o111);
                match std::fs::set_permissions(&binary_path, new_perms) {
                    Ok(()) => log::warn!(
                        "Sidecar binary lacked execute permission (mode {:o}), fixed to {:o}",
                        mode,
                        mode | 0o111
                    ),
                    Err(e) => log::warn!("Failed to set execute permission on sidecar: {}", e),
                }
            }
        }
    }

    // Spawn the sidecar
    let mut child = match Command::new(&binary_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log::error!("Failed to spawn sidecar: {}", e);
            return SidecarStatus::StartFailed(e.to_string());
        }
    };

    // Give the sidecar a moment to start or fail
    std::thread::sleep(Duration::from_millis(100));

    // Check if it's still running
    match child.try_wait() {
        Ok(Some(status)) => {
            // Process exited immediately - likely a dyld failure
            let stderr = child.stderr.take().map(|mut s| {
                let mut buf = String::new();
                use std::io::Read;
                let _ = s.read_to_string(&mut buf);
                buf
            }).unwrap_or_default();

            if stderr.contains("dyld") || stderr.contains("Library not loaded") || stderr.contains("libsmb") {
                log::warn!("Sidecar failed with dyld error: {}", stderr);
                return SidecarStatus::LibraryMissing;
            }

            log::error!("Sidecar exited immediately with status {:?}: {}", status, stderr);
            return SidecarStatus::StartFailed(format!("Exited with status: {:?}", status));
        }
        Ok(None) => {
            // Still running, good
        }
        Err(e) => {
            log::error!("Failed to check sidecar status: {}", e);
            return SidecarStatus::StartFailed(e.to_string());
        }
    }

    let stdin = match child.stdin.take() {
        Some(s) => s,
        None => {
            log::error!("Failed to get sidecar stdin");
            let _ = child.kill();
            return SidecarStatus::StartFailed("Failed to get stdin".to_string());
        }
    };

    let stdout = match child.stdout.take() {
        Some(s) => BufReader::new(s),
        None => {
            log::error!("Failed to get sidecar stdout");
            let _ = child.kill();
            return SidecarStatus::StartFailed("Failed to get stdout".to_string());
        }
    };

    state.process = Some(SidecarProcess {
        child,
        stdin,
        stdout,
    });
    state.restart_attempts = 0;

    // Note: We can't verify with a ping here because we'd need to release the lock
    // and re-acquire it to avoid deadlock. The verification will happen on the first real call.

    SidecarStatus::Available
}

/// Find the sidecar binary.
/// SECURITY: Only look in the app bundle, never in PATH.
fn find_sidecar_binary() -> Option<PathBuf> {
    // Determine the target triple suffix for Tauri sidecar naming
    let target_triple = get_target_triple();

    // First, check next to the current executable (development/bundled)
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            // Try with target triple suffix (Tauri convention)
            let sidecar_with_suffix = exe_dir.join(format!("marlin-smb-{}", target_triple));
            if sidecar_with_suffix.is_file() {
                return Some(sidecar_with_suffix);
            }

            // Try without suffix (development)
            let sidecar_plain = exe_dir.join("marlin-smb");
            if sidecar_plain.is_file() {
                return Some(sidecar_plain);
            }

            // On macOS, check in Resources (for bundled apps)
            #[cfg(target_os = "macos")]
            {
                // exe is in Marlin.app/Contents/MacOS/Marlin
                // sidecar might be in Marlin.app/Contents/MacOS/ or Marlin.app/Contents/Resources/
                if let Some(macos_dir) = exe_dir.parent() {
                    let resources_dir = macos_dir.join("Resources");
                    let sidecar_resources = resources_dir.join(format!("marlin-smb-{}", target_triple));
                    if sidecar_resources.is_file() {
                        return Some(sidecar_resources);
                    }
                }
            }
        }
    }

    // Check in the target/debug or target/release directory (development)
    #[cfg(debug_assertions)]
    {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let debug_path = PathBuf::from(manifest_dir)
            .join("target")
            .join("debug")
            .join("marlin-smb");
        if debug_path.is_file() {
            return Some(debug_path);
        }
    }

    None
}

/// Get the target triple for the current platform.
fn get_target_triple() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "aarch64-apple-darwin"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "x86_64-apple-darwin"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "x86_64-unknown-linux-gnu"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "aarch64-unknown-linux-gnu"
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
    )))]
    {
        "unknown-unknown-unknown"
    }
}

/// Shutdown the sidecar when the app exits.
#[allow(dead_code)]
pub fn shutdown() {
    let mut state = SIDECAR.lock().expect("Sidecar mutex poisoned");
    if let Some(mut process) = state.process.take() {
        log::info!("Shutting down SMB sidecar");
        // Close stdin to signal the sidecar to exit
        drop(process.stdin);
        // Give it a moment to exit gracefully
        std::thread::sleep(Duration::from_millis(100));
        // Force kill if still running
        let _ = process.child.kill();
    }
}
