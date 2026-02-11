pub mod auth;
pub mod provider;
pub mod url_parser;

#[cfg(test)]
mod tests;

pub use auth::{add_google_account, get_google_accounts, remove_google_account, GoogleAccountInfo};
pub use provider::GoogleDriveProvider;

/// Parse a `gdrive://user@domain/path` URI into (email, decoded_path).
///
/// The url crate treats `gdrive://brian@smg.gg/My Drive/file.zpl` as
/// scheme=gdrive, username=brian, host=smg.gg, path=/My%20Drive/file.zpl.
/// This function extracts the email and decodes the path.
pub fn parse_gdrive_uri(raw: &str) -> Result<(String, String), String> {
    let url = url::Url::parse(raw).map_err(|e| format!("Invalid Google Drive path: {e}"))?;

    let host = url
        .host_str()
        .ok_or_else(|| "Google Drive path missing account".to_string())?;

    let email = if url.username().is_empty() {
        host.to_string()
    } else {
        format!("{}@{}", url.username(), host)
    };

    let path = urlencoding::decode(url.path())
        .map_err(|e| format!("Invalid UTF-8 in Google Drive path: {e}"))?
        .into_owned();

    Ok((email, path))
}
