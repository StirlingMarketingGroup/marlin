//! Manual SMB integration tests (not run in CI).
//!
//! These tests require:
//! - Building with `--features smb`
//! - `libsmbclient` installed (macOS: `brew install samba`, Linux: `apt install libsmbclient-dev`)
//! - SMB server credentials configured in Marlin (via the UI)
//!
//! Run (examples):
//! - `MARLIN_SMB_TEST_SERVER=storage cargo test -p marlin --features smb --test smb_integration -- --ignored --nocapture`
//! - `MARLIN_SMB_TEST_SERVER=storage MARLIN_SMB_TEST_SHARE=Public cargo test -p marlin --features smb --test smb_integration -- --ignored --nocapture`
//! - `MARLIN_SMB_TEST_SERVER=storage MARLIN_SMB_TEST_SHARE=Public MARLIN_SMB_TEST_PATH=/DOD cargo test -p marlin --features smb --test smb_integration -- --ignored --nocapture`
#![cfg(feature = "smb")]

use app_lib::locations::smb::SmbProvider;
use app_lib::locations::{Location, LocationProvider};

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("Missing required env var: {name}"))
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

#[test]
#[ignore]
fn smb_can_list_shares() {
    let server = env("MARLIN_SMB_TEST_SERVER").expect("Set MARLIN_SMB_TEST_SERVER to run this test");
    let provider = SmbProvider::default();
    let location = Location::parse(&format!("smb://{server}/")).expect("Invalid SMB URL");

    let result = tauri::async_runtime::block_on(async { provider.read_directory(&location).await });
    match result {
        Ok(entries) => {
            assert!(
                !entries.entries.is_empty(),
                "Expected at least one share for smb://{server}/"
            );
        }
        Err(err) => panic!("Failed to list shares for smb://{server}/: {err}"),
    }
}

#[test]
#[ignore]
fn smb_can_list_directory() {
    let server = env("MARLIN_SMB_TEST_SERVER").expect("Set MARLIN_SMB_TEST_SERVER to run this test");
    let share = env("MARLIN_SMB_TEST_SHARE").expect("Set MARLIN_SMB_TEST_SHARE to run this test");
    let path = env_or("MARLIN_SMB_TEST_PATH", "/");

    let provider = SmbProvider::default();
    let url = if path == "/" {
        format!("smb://{server}/{share}/")
    } else {
        let trimmed = path.strip_prefix('/').unwrap_or(&path);
        format!("smb://{server}/{share}/{trimmed}")
    };
    let location = Location::parse(&url).expect("Invalid SMB URL");

    let result = tauri::async_runtime::block_on(async { provider.read_directory(&location).await });
    match result {
        Ok(entries) => {
            // We don’t assert on exact counts; just ensure the read worked.
            // This provides a quick “can we talk to SMB?” signal for local debugging.
            assert!(
                entries.location.raw.starts_with("smb://"),
                "Expected SMB location summary"
            );
        }
        Err(err) => panic!("Failed to list directory for {url}: {err}"),
    }
}

