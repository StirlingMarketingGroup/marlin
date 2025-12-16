//! Google Drive Integration Tests
//!
//! These tests require a service account to be configured via
//! GOOGLE_SERVICE_ACCOUNT_KEY_FILE environment variable.
//!
//! Run with:
//!   GOOGLE_SERVICE_ACCOUNT_KEY_FILE=/path/to/key.json cargo test --test gdrive_integration_test

use std::env;

// Import from the main library
use app_lib::locations::gdrive::auth::{
    ensure_valid_token, get_google_accounts, get_service_account_email,
};
use app_lib::locations::gdrive::provider::GoogleDriveProvider;
use app_lib::locations::{Location, LocationProvider};

/// Check if service account is configured
fn service_account_configured() -> bool {
    env::var("GOOGLE_SERVICE_ACCOUNT_KEY_FILE").is_ok()
}

/// Skip test if service account is not configured
macro_rules! require_service_account {
    () => {
        if !service_account_configured() {
            eprintln!("Skipping test: GOOGLE_SERVICE_ACCOUNT_KEY_FILE not set");
            return;
        }
    };
}

#[tokio::test]
async fn test_service_account_is_detected() {
    require_service_account!();

    let email = get_service_account_email();
    assert!(email.is_some(), "Service account email should be detected");

    let email = email.unwrap();
    assert!(
        email.contains("@") && email.contains(".iam.gserviceaccount.com"),
        "Email should be a service account: {}",
        email
    );
    println!("Service account email: {}", email);
}

#[tokio::test]
async fn test_service_account_appears_in_accounts_list() {
    require_service_account!();

    let accounts = get_google_accounts().expect("Should get accounts list");
    assert!(!accounts.is_empty(), "Should have at least one account");

    let service_account = accounts
        .iter()
        .find(|a| a.email.contains(".iam.gserviceaccount.com"));
    assert!(
        service_account.is_some(),
        "Service account should be in accounts list"
    );

    println!(
        "Found {} accounts, including service account",
        accounts.len()
    );
}

#[tokio::test]
async fn test_service_account_token_fetch() {
    require_service_account!();

    let email = get_service_account_email().expect("Service account should be configured");

    let token = ensure_valid_token(&email)
        .await
        .expect("Should fetch access token");

    assert!(!token.is_empty(), "Token should not be empty");
    assert!(
        token.starts_with("ya29."),
        "Token should be a Google access token"
    );
    println!("Successfully fetched access token (length: {})", token.len());
}

#[tokio::test]
async fn test_list_drive_root() {
    require_service_account!();

    let email = get_service_account_email().expect("Service account should be configured");
    let provider = GoogleDriveProvider::default();

    // Create location for drive root
    let location = Location::parse(&format!("gdrive://{}/", email)).expect("Valid location");

    let result = provider
        .read_directory(&location)
        .await
        .expect("Should list drive root");

    println!("Drive root contains {} entries:", result.entries.len());
    for entry in &result.entries {
        println!("  - {} ({})", entry.name, if entry.is_directory { "folder" } else { "file" });
    }

    // Should have virtual folders
    let has_my_drive = result.entries.iter().any(|e| e.name == "My Drive");
    let has_shared = result.entries.iter().any(|e| e.name == "Shared with me");

    assert!(has_my_drive, "Should have 'My Drive' virtual folder");
    assert!(has_shared, "Should have 'Shared with me' virtual folder");
}

#[tokio::test]
async fn test_list_my_drive() {
    require_service_account!();

    let email = get_service_account_email().expect("Service account should be configured");
    let provider = GoogleDriveProvider::default();

    let location =
        Location::parse(&format!("gdrive://{}/My Drive", email)).expect("Valid location");

    let result = provider
        .read_directory(&location)
        .await
        .expect("Should list My Drive");

    println!("My Drive contains {} entries", result.entries.len());
    // Service account's My Drive might be empty, that's ok
}

#[tokio::test]
async fn test_list_shared_with_me() {
    require_service_account!();

    let email = get_service_account_email().expect("Service account should be configured");
    let provider = GoogleDriveProvider::default();

    let location =
        Location::parse(&format!("gdrive://{}/Shared with me", email)).expect("Valid location");

    let result = provider
        .read_directory(&location)
        .await
        .expect("Should list Shared with me");

    println!("Shared with me contains {} entries:", result.entries.len());
    for entry in &result.entries {
        println!(
            "  - {} ({})",
            entry.name,
            if entry.is_directory { "folder" } else { "file" }
        );
    }
}

#[tokio::test]
async fn test_navigate_into_shared_folder() {
    require_service_account!();

    let email = get_service_account_email().expect("Service account should be configured");
    let provider = GoogleDriveProvider::default();

    // First list shared folders
    let shared_location =
        Location::parse(&format!("gdrive://{}/Shared with me", email)).expect("Valid location");

    let shared_result = provider
        .read_directory(&shared_location)
        .await
        .expect("Should list Shared with me");

    // Find a folder to navigate into
    let folder = shared_result
        .entries
        .iter()
        .find(|e| e.is_directory);

    if let Some(folder) = folder {
        println!("Navigating into shared folder: {}", folder.name);

        let folder_location = Location::parse(&folder.path).expect("Valid folder path");
        let folder_result = provider
            .read_directory(&folder_location)
            .await
            .expect("Should list folder contents");

        println!(
            "Folder '{}' contains {} entries:",
            folder.name,
            folder_result.entries.len()
        );
        for entry in &folder_result.entries {
            println!(
                "  - {} ({})",
                entry.name,
                if entry.is_directory { "folder" } else { "file" }
            );
        }
    } else {
        println!("No shared folders to navigate into (expected if nothing is shared)");
    }
}

#[tokio::test]
async fn test_gdrive_path_scheme_preserved() {
    require_service_account!();

    let email = get_service_account_email().expect("Service account should be configured");
    let provider = GoogleDriveProvider::default();

    let location = Location::parse(&format!("gdrive://{}/", email)).expect("Valid location");

    let result = provider
        .read_directory(&location)
        .await
        .expect("Should list drive root");

    // All paths should start with gdrive://
    for entry in &result.entries {
        assert!(
            entry.path.starts_with("gdrive://"),
            "Path should start with gdrive://: {}",
            entry.path
        );
        // Should have double slashes after scheme
        assert!(
            !entry.path.contains("gdrive:/") || entry.path.contains("gdrive://"),
            "Path should have double slashes: {}",
            entry.path
        );
    }
    println!("All {} paths have correct gdrive:// scheme", result.entries.len());
}
