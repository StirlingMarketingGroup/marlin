//! Manual Google Drive tests (not run in CI).
//!
//! These tests call the live Drive API as the first account Marlin lists. That is the service
//! account when `GOOGLE_SERVICE_ACCOUNT_KEY_FILE` is set, and otherwise the first Google account
//! connected in Marlin. They read that account's sign-in from the OS keychain, so macOS may ask
//! to allow it.
//!
//! Start Marlin once before the first run. If the accounts file still holds tokens from an older
//! version, the app should be the one that moves them into the keychain, not the test binary.
//!
//! Run:
//! - `cargo test -p marlin --test gdrive_integration -- --ignored --nocapture`
//! - The two file ID tests also need `MARLIN_GDRIVE_TEST_FILE_ID`, the ID of a folder the account can open.

use app_lib::locations::gdrive::provider::resolve_file_id_to_path;
use app_lib::locations::gdrive::{get_google_accounts, GoogleDriveProvider};
use app_lib::locations::{Location, LocationProvider};

fn account_email() -> String {
    app_lib::ensure_rustls_crypto_provider();
    get_google_accounts()
        .expect("Failed to read the Google accounts")
        .first()
        .expect("Connect a Google account in Marlin to run this test")
        .email
        .clone()
}

fn file_id() -> String {
    std::env::var("MARLIN_GDRIVE_TEST_FILE_ID")
        .expect("Set MARLIN_GDRIVE_TEST_FILE_ID to run this test")
}

fn location(raw: &str) -> Location {
    Location::parse(raw).expect("Invalid Google Drive URL")
}

#[tokio::test]
#[ignore]
async fn gdrive_can_list_the_account_root() {
    let email = account_email();
    let provider = GoogleDriveProvider::default();

    let entries = provider
        .read_directory(&location(&format!("gdrive://{email}/")))
        .await
        .expect("Failed to list the account root");

    let names: Vec<_> = entries.entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"My Drive"), "{names:?}");
    assert!(names.contains(&"Shared with me"), "{names:?}");
}

#[tokio::test]
#[ignore]
async fn gdrive_can_list_my_drive() {
    let email = account_email();
    let provider = GoogleDriveProvider::default();

    provider
        .read_directory(&location(&format!("gdrive://{email}/My Drive")))
        .await
        .expect("Failed to list My Drive");
}

#[tokio::test]
#[ignore]
async fn gdrive_can_open_a_folder_shared_with_me() {
    let email = account_email();
    let provider = GoogleDriveProvider::default();

    let shared = provider
        .read_directory(&location(&format!("gdrive://{email}/Shared with me")))
        .await
        .expect("Failed to list Shared with me");

    let Some(folder) = shared.entries.iter().find(|e| e.is_directory) else {
        println!("No folder in Shared with me, so there is nothing to open");
        return;
    };

    provider
        .read_directory(&location(&folder.path))
        .await
        .unwrap_or_else(|err| panic!("Failed to open {}: {err}", folder.path));
}

#[tokio::test]
#[ignore]
async fn gdrive_can_resolve_a_file_id_to_a_path() {
    app_lib::ensure_rustls_crypto_provider();
    let provider = GoogleDriveProvider::default();

    let (_email, path) = resolve_file_id_to_path(&file_id())
        .await
        .expect("Failed to resolve the file ID");

    provider
        .read_directory(&location(&path))
        .await
        .unwrap_or_else(|err| panic!("Failed to list {path}: {err}"));
}

#[tokio::test]
#[ignore]
async fn gdrive_can_open_a_folder_by_id() {
    let email = account_email();
    let provider = GoogleDriveProvider::default();

    provider
        .read_directory(&location(&format!("gdrive://{email}/id/{}", file_id())))
        .await
        .expect("Failed to open the folder by its ID");
}
