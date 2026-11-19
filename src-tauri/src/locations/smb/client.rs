//! The app's one `marlin-smb` process: this module finds the binary and picks each
//! method's time limit. `marlin-smb-client` starts the process and talks to it.

use marlin_smb_client::protocol::methods;
use once_cell::sync::Lazy;
use serde::{de::DeserializeOwned, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

static SIDECAR: Lazy<marlin_smb_client::Sidecar> =
    Lazy::new(|| marlin_smb_client::Sidecar::new(find_sidecar_binary()));

/// Starts the sidecar when it is not running.
pub fn start() -> Result<(), marlin_smb_client::StartError> {
    SIDECAR.start()
}

/// Call a method on the sidecar.
///
/// A download, an upload, and a copy have no time limit. A partial download has
/// 5 minutes. Every other method has 30 seconds.
pub fn call_method<P: Serialize, R: DeserializeOwned>(
    method: &str,
    params: P,
) -> Result<R, String> {
    let limit = match method {
        methods::DOWNLOAD_FILE | methods::UPLOAD_FILE | methods::COPY => None,
        methods::DOWNLOAD_PARTIAL => Some(Duration::from_secs(5 * 60)),
        _ => Some(Duration::from_secs(30)),
    };
    SIDECAR
        .call(method, params, limit)
        .map_err(|err| err.to_string())
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
            if is_installed(&sidecar_with_suffix) {
                return Some(sidecar_with_suffix);
            }

            // Try without suffix (development)
            let sidecar_plain = exe_dir.join("marlin-smb");
            if is_installed(&sidecar_plain) {
                return Some(sidecar_plain);
            }

            // On macOS, check in Resources (for bundled apps)
            #[cfg(target_os = "macos")]
            {
                // exe is in Marlin.app/Contents/MacOS/Marlin
                // sidecar might be in Marlin.app/Contents/MacOS/ or Marlin.app/Contents/Resources/
                if let Some(macos_dir) = exe_dir.parent() {
                    let resources_dir = macos_dir.join("Resources");
                    let sidecar_resources =
                        resources_dir.join(format!("marlin-smb-{}", target_triple));
                    if is_installed(&sidecar_resources) {
                        return Some(sidecar_resources);
                    }
                }
            }
        }
    }

    // A dev build also looks in target/debug, where `cargo build -p marlin-smb` puts it.
    #[cfg(debug_assertions)]
    {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let debug_path = PathBuf::from(manifest_dir)
            .join("target")
            .join("debug")
            .join("marlin-smb");
        if is_installed(&debug_path) {
            return Some(debug_path);
        }
    }

    None
}

/// A release without SMB (the Intel Mac) and a fresh clone ship an empty placeholder
/// file (`build.rs`), which is not a helper.
fn is_installed(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|file| file.is_file() && file.len() > 0)
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
