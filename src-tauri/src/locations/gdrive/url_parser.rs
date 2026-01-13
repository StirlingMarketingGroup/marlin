use url::Url;

/// Parsed Google Drive URL information
#[derive(Debug, Clone)]
pub struct GoogleDriveUrlInfo {
    /// The file or folder ID extracted from the URL
    pub id: String,
    /// Whether this is a folder (vs a file)
    pub is_folder: bool,
}

/// Parse a Google Drive URL and extract the file/folder ID
///
/// Supported formats:
/// - https://drive.google.com/open?id=XXXXX
/// - https://drive.google.com/file/d/XXXXX/view
/// - https://drive.google.com/file/d/XXXXX/edit
/// - https://drive.google.com/drive/folders/XXXXX
/// - https://drive.google.com/drive/u/0/folders/XXXXX
/// - https://drive.google.com/drive/u/1/folders/XXXXX
/// - https://docs.google.com/document/d/XXXXX/edit
/// - https://docs.google.com/spreadsheets/d/XXXXX/edit
/// - https://docs.google.com/presentation/d/XXXXX/edit
pub fn parse_google_drive_url(url_str: &str) -> Option<GoogleDriveUrlInfo> {
    let url = Url::parse(url_str).ok()?;

    let host = url.host_str()?;
    if !host.ends_with("google.com") {
        return None;
    }

    // Check for drive.google.com or docs.google.com
    if !host.starts_with("drive.") && !host.starts_with("docs.") {
        return None;
    }

    let path = url.path();
    let query_pairs: Vec<_> = url.query_pairs().collect();

    // Format: /open?id=XXXXX
    if path == "/open" {
        for (key, value) in &query_pairs {
            if key == "id" && !value.is_empty() {
                // Clean up the ID - remove any whitespace that might have crept in
                let clean_id = value.replace(char::is_whitespace, "");
                return Some(GoogleDriveUrlInfo {
                    id: clean_id,
                    is_folder: false, // Can't tell from this format
                });
            }
        }
        return None;
    }

    // Format: /file/d/XXXXX/view or /file/d/XXXXX/edit
    if path.starts_with("/file/d/") {
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() >= 4 && !parts[3].is_empty() {
            return Some(GoogleDriveUrlInfo {
                id: parts[3].to_string(),
                is_folder: false,
            });
        }
        return None;
    }

    // Format: /drive/folders/XXXXX or /drive/u/N/folders/XXXXX
    if path.contains("/folders/") {
        let parts: Vec<&str> = path.split('/').collect();
        for (i, part) in parts.iter().enumerate() {
            if *part == "folders" && i + 1 < parts.len() {
                let id = parts[i + 1];
                // Remove any query string fragments that might be attached
                let clean_id = id.split('?').next().unwrap_or(id);
                if !clean_id.is_empty() {
                    return Some(GoogleDriveUrlInfo {
                        id: clean_id.to_string(),
                        is_folder: true,
                    });
                }
            }
        }
        return None;
    }

    // Format: /document/d/XXXXX/edit or /spreadsheets/d/XXXXX/edit or /presentation/d/XXXXX/edit
    if path.contains("/d/") {
        let parts: Vec<&str> = path.split('/').collect();
        for (i, part) in parts.iter().enumerate() {
            if *part == "d" && i + 1 < parts.len() {
                let id = parts[i + 1];
                if !id.is_empty() && id != "edit" && id != "view" {
                    return Some(GoogleDriveUrlInfo {
                        id: id.to_string(),
                        is_folder: false,
                    });
                }
            }
        }
        return None;
    }

    None
}

/// Check if a string looks like a Google Drive URL
pub fn is_google_drive_url(s: &str) -> bool {
    if let Ok(url) = Url::parse(s) {
        if let Some(host) = url.host_str() {
            return (host.starts_with("drive.") || host.starts_with("docs."))
                && host.ends_with("google.com");
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_open_id_format() {
        let url = "https://drive.google.com/open?id=1S-OYLnHs9iwOk4xvxTiH5NVMvEP6Wjnw";
        let result = parse_google_drive_url(url).unwrap();
        assert_eq!(result.id, "1S-OYLnHs9iwOk4xvxTiH5NVMvEP6Wjnw");
    }

    #[test]
    fn test_parse_file_view_format() {
        let url = "https://drive.google.com/file/d/1abc123def456/view";
        let result = parse_google_drive_url(url).unwrap();
        assert_eq!(result.id, "1abc123def456");
        assert!(!result.is_folder);
    }

    #[test]
    fn test_parse_folder_format() {
        let url = "https://drive.google.com/drive/folders/1folder123";
        let result = parse_google_drive_url(url).unwrap();
        assert_eq!(result.id, "1folder123");
        assert!(result.is_folder);
    }

    #[test]
    fn test_parse_folder_with_user_format() {
        let url = "https://drive.google.com/drive/u/0/folders/1folder456";
        let result = parse_google_drive_url(url).unwrap();
        assert_eq!(result.id, "1folder456");
        assert!(result.is_folder);
    }

    #[test]
    fn test_parse_docs_format() {
        let url = "https://docs.google.com/document/d/1doc123/edit";
        let result = parse_google_drive_url(url).unwrap();
        assert_eq!(result.id, "1doc123");
        assert!(!result.is_folder);
    }

    #[test]
    fn test_parse_spreadsheet_format() {
        let url = "https://docs.google.com/spreadsheets/d/1sheet123/edit";
        let result = parse_google_drive_url(url).unwrap();
        assert_eq!(result.id, "1sheet123");
    }

    #[test]
    fn test_non_google_url() {
        let url = "https://example.com/file/123";
        assert!(parse_google_drive_url(url).is_none());
    }

    #[test]
    fn test_is_google_drive_url() {
        assert!(is_google_drive_url("https://drive.google.com/open?id=123"));
        assert!(is_google_drive_url("https://docs.google.com/document/d/123/edit"));
        assert!(!is_google_drive_url("https://example.com/file"));
        assert!(!is_google_drive_url("not a url"));
    }
}
