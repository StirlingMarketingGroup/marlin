//! The fake Google that the build `npm run test:app` drives signs in and mounts against, in place of Google (decision 0004). `MARLIN_TEST_GOOGLE` holds its address, and `marlin-test-google` serves it. With no address there is no OAuth client, so that build gets no token to send anywhere.

use crate::{Config, OAuthClient};

const FAKE_GOOGLE: &str = "MARLIN_TEST_GOOGLE";

/// The fake Google's address. Only a plain-HTTP address of this machine counts: with any other value, or none, every address below is relative, and no request can be sent anywhere.
fn address() -> Option<String> {
    let value = std::env::var(FAKE_GOOGLE).ok()?;
    let url = reqwest::Url::parse(&value).ok()?;
    let local = url.scheme() == "http"
        && matches!(url.host_str(), Some("127.0.0.1" | "localhost"))
        && url.username().is_empty()
        && url.password().is_none();
    local.then_some(value)
}

pub fn oauth_client() -> Result<OAuthClient, String> {
    let address = address()
        .ok_or_else(|| format!("{FAKE_GOOGLE} must be the fake Google's loopback address"))?;
    Ok(OAuthClient {
        client_id: "marlin-app-test".to_string(),
        client_secret: "marlin-app-test".to_string(),
        token_uri: format!("{address}/token"),
    })
}

pub fn sign_in_page() -> String {
    format!("{}/auth", address().unwrap_or_default())
}

pub fn user_info() -> String {
    format!("{}/userinfo", address().unwrap_or_default())
}

/// Plays the browser: the fake Google signs the user in at once and sends the browser back to the app's OAuth listener with a code.
pub async fn open_sign_in_page(url: &str) -> Result<(), String> {
    reqwest::get(url)
        .await
        .map(|_| ())
        .map_err(|e| format!("Failed to open the fake Google's sign-in page: {e}"))
}

/// The fake Google serves the Drive API at its root.
pub fn drive_config() -> Config {
    let address = address().unwrap_or_default();
    Config {
        upload: format!("{address}/upload"),
        api: address,
        retry_delay: Config::default().retry_delay,
    }
}
