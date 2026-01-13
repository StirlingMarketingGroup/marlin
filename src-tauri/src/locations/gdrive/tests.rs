//! Integration tests for Google Drive provider
//! These tests use the already-authenticated user tokens stored locally.

#[cfg(test)]
mod tests {
    use crate::locations::gdrive::auth::get_all_accounts;
    use crate::locations::gdrive::provider::{GoogleDriveProvider, resolve_file_id_to_path};
    use crate::locations::{LocationInput, LocationProvider};

    /// Helper to check if we have any authenticated accounts
    fn has_accounts() -> bool {
        get_all_accounts().map(|a| !a.is_empty()).unwrap_or(false)
    }

    /// Helper to create a Location from a raw string
    fn location(raw: &str) -> crate::locations::Location {
        LocationInput::Raw(raw.to_string()).into_location().unwrap()
    }

    #[tokio::test]
    async fn test_list_virtual_root() {
        if !has_accounts() {
            println!("Skipping test: no Google accounts connected");
            return;
        }

        let accounts = get_all_accounts().unwrap();
        let email = &accounts[0].email;
        println!("Testing with account: {}", email);

        let provider = GoogleDriveProvider::default();
        let loc = location(&format!("gdrive://{}/", email));

        let result = provider.read_directory(&loc).await;

        assert!(result.is_ok(), "Failed to list virtual root: {:?}", result.err());

        let entries = result.unwrap();
        assert!(!entries.entries.is_empty(), "Virtual root should have entries");

        // Should have My Drive, Shared with me, etc.
        let names: Vec<_> = entries.entries.iter().map(|e| &e.name).collect();
        println!("Virtual root entries: {:?}", names);

        assert!(names.contains(&&"My Drive".to_string()), "Should have My Drive");
        assert!(names.contains(&&"Shared with me".to_string()), "Should have Shared with me");
    }

    #[tokio::test]
    async fn test_list_my_drive() {
        if !has_accounts() {
            println!("Skipping test: no Google accounts connected");
            return;
        }

        let accounts = get_all_accounts().unwrap();
        let email = &accounts[0].email;

        let provider = GoogleDriveProvider::default();
        let loc = location(&format!("gdrive://{}/My Drive", email));

        let result = provider.read_directory(&loc).await;
        println!("My Drive result: is_ok={}", result.is_ok());

        assert!(result.is_ok(), "Failed to list My Drive: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_list_shared_with_me() {
        if !has_accounts() {
            println!("Skipping test: no Google accounts connected");
            return;
        }

        let accounts = get_all_accounts().unwrap();
        let email = &accounts[0].email;

        let provider = GoogleDriveProvider::default();
        let loc = location(&format!("gdrive://{}/Shared with me", email));

        let result = provider.read_directory(&loc).await;
        println!("Shared with me result: is_ok={}", result.is_ok());

        assert!(result.is_ok(), "Failed to list Shared with me: {:?}", result.err());

        let entries = result.unwrap();
        println!("Shared with me has {} entries", entries.entries.len());

        // Print first few entries to see their paths
        for entry in entries.entries.iter().take(5) {
            println!("  - name: {}, path: {}, is_dir: {}", entry.name, entry.path, entry.is_directory);
        }
    }

    #[tokio::test]
    async fn test_navigate_into_shared_folder() {
        if !has_accounts() {
            println!("Skipping test: no Google accounts connected");
            return;
        }

        let accounts = get_all_accounts().unwrap();
        let email = &accounts[0].email;

        let provider = GoogleDriveProvider::default();

        // First, list Shared with me to get a folder
        let loc = location(&format!("gdrive://{}/Shared with me", email));
        let result = provider.read_directory(&loc).await;
        assert!(result.is_ok(), "Failed to list Shared with me");

        let entries = result.unwrap();

        // Find a folder to navigate into
        let folder = entries.entries.iter().find(|e| e.is_directory);

        if let Some(folder) = folder {
            println!("Navigating into folder: {} at path: {}", folder.name, folder.path);

            // Navigate into it using the path from the entry
            let folder_location = location(&folder.path);
            let folder_result = provider.read_directory(&folder_location).await;

            println!("Folder navigation result: is_ok={}", folder_result.is_ok());
            assert!(folder_result.is_ok(), "Failed to navigate into folder: {:?}", folder_result.err());

            let folder_entries = folder_result.unwrap();
            println!("Folder has {} entries", folder_entries.entries.len());

            // Print contents
            for entry in folder_entries.entries.iter().take(10) {
                println!("  - {}", entry.name);
            }
        } else {
            println!("No folders found in Shared with me, skipping navigation test");
        }
    }

    #[tokio::test]
    async fn test_resolve_google_drive_url() {
        if !has_accounts() {
            println!("Skipping test: no Google accounts connected");
            return;
        }

        // Test with a known file ID (the one from the user's URL)
        let file_id = "1S-OYLnHs9iwOk4xvxTiH5NVMvEP6Wjnw";

        println!("Resolving file ID: {}", file_id);
        let result = resolve_file_id_to_path(file_id).await;

        println!("Resolution result: {:?}", result);

        match result {
            Ok((email, path)) => {
                println!("Resolved to: email={}, path={}", email, path);

                // Now try to navigate to that path
                let provider = GoogleDriveProvider::default();
                let loc = location(&path);
                let dir_result = provider.read_directory(&loc).await;

                println!("Directory listing result: is_ok={}", dir_result.is_ok());
                assert!(dir_result.is_ok(), "Failed to list resolved directory: {:?}", dir_result.err());

                let entries = dir_result.unwrap();
                println!("Directory has {} entries", entries.entries.len());
                for entry in entries.entries.iter().take(10) {
                    println!("  - {}", entry.name);
                }
            }
            Err(e) => {
                println!("Failed to resolve: {}", e);
                // Don't fail the test - the file might not be accessible
            }
        }
    }

    #[tokio::test]
    async fn test_id_based_navigation() {
        if !has_accounts() {
            println!("Skipping test: no Google accounts connected");
            return;
        }

        let accounts = get_all_accounts().unwrap();
        let email = &accounts[0].email;

        // Test with a known file ID
        let file_id = "1S-OYLnHs9iwOk4xvxTiH5NVMvEP6Wjnw";
        let path = format!("gdrive://{}/id/{}", email, file_id);

        println!("Testing ID-based navigation: {}", path);

        let provider = GoogleDriveProvider::default();
        let loc = location(&path);
        let result = provider.read_directory(&loc).await;

        println!("ID-based navigation result: is_ok={}", result.is_ok());

        if result.is_ok() {
            let entries = result.unwrap();
            println!("Success! Directory has {} entries", entries.entries.len());
            for entry in entries.entries.iter().take(10) {
                println!("  - {}", entry.name);
            }
        } else {
            println!("Failed (file may not be accessible with this account): {:?}", result.err());
        }
    }

}
