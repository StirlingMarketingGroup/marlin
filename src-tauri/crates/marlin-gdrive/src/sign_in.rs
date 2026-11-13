use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use marlin_vfs::VfsError;
use serde::Deserialize;

use crate::provider::AccessTokens;

/// The OAuth client ID and secret from the build. Empty when the build had none. [`OAuthClient::load`] tries the run-time environment first.
const CLIENT_ID: &str = match option_env!("GOOGLE_CLIENT_ID") {
    Some(id) => id,
    None => "",
};
const CLIENT_SECRET: &str = match option_env!("GOOGLE_CLIENT_SECRET") {
    Some(secret) => secret,
    None => "",
};

const GOOGLE_CLIENT_ID_ENV: &str = "GOOGLE_CLIENT_ID";
const GOOGLE_CLIENT_SECRET_ENV: &str = "GOOGLE_CLIENT_SECRET";
const GOOGLE_CREDENTIALS_FILE_NAME: &str = "google-credentials.json";

/// Marlin's OAuth client: what Google knows the app as, and where it trades tokens.
#[derive(Clone)]
pub struct OAuthClient {
    pub client_id: String,
    pub client_secret: String,
    /// Google's token endpoint. Tests point it at a fake server.
    pub token_uri: String,
}

impl fmt::Debug for OAuthClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthClient")
            .field("client_id", &self.client_id)
            .field("token_uri", &self.token_uri)
            .finish()
    }
}

#[derive(Deserialize)]
struct OAuthCredentialsFile {
    installed: Option<OAuthClientSecret>,
    web: Option<OAuthClientSecret>,
    client_id: Option<String>,
    client_secret: Option<String>,
}

#[derive(Deserialize)]
struct OAuthClientSecret {
    client_id: String,
    client_secret: String,
}

fn normalize_secret(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn oauth_credentials_error() -> String {
    format!(
        "Google OAuth credentials not configured. Set {} and {} or provide {}.",
        GOOGLE_CLIENT_ID_ENV, GOOGLE_CLIENT_SECRET_ENV, GOOGLE_CREDENTIALS_FILE_NAME
    )
}

fn oauth_from_pair(
    client_id: Option<String>,
    client_secret: Option<String>,
    source: &str,
) -> Result<OAuthClient, String> {
    let client_id = normalize_secret(client_id);
    let client_secret = normalize_secret(client_secret);

    match (client_id, client_secret) {
        (Some(client_id), Some(client_secret)) => Ok(OAuthClient {
            client_id,
            client_secret,
            token_uri: "https://oauth2.googleapis.com/token".to_string(),
        }),
        (Some(_), None) | (None, Some(_)) => Err(format!(
            "{} is missing part of the OAuth client secret",
            source
        )),
        (None, None) => Err(format!("{} not configured", source)),
    }
}

fn load_oauth_credentials_from_file(path: &Path) -> Result<OAuthClient, String> {
    let contents = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    let parsed: OAuthCredentialsFile = serde_json::from_str(&contents)
        // Only where the error is: serde's own text can quote a value from the file, and the file holds the secret.
        .map_err(|e| {
            format!(
                "Failed to parse {}: line {}, column {}",
                path.display(),
                e.line(),
                e.column()
            )
        })?;

    if let Some(installed) = parsed.installed {
        return oauth_from_pair(
            Some(installed.client_id),
            Some(installed.client_secret),
            &format!("OAuth credentials file {}", path.display()),
        );
    }

    if let Some(web) = parsed.web {
        return oauth_from_pair(
            Some(web.client_id),
            Some(web.client_secret),
            &format!("OAuth credentials file {}", path.display()),
        );
    }

    oauth_from_pair(
        parsed.client_id,
        parsed.client_secret,
        &format!("OAuth credentials file {}", path.display()),
    )
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn oauth_credentials_candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(config_dir) = dirs::config_dir() {
        push_unique_path(
            &mut paths,
            config_dir.join("marlin").join(GOOGLE_CREDENTIALS_FILE_NAME),
        );
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            push_unique_path(&mut paths, exe_dir.join(GOOGLE_CREDENTIALS_FILE_NAME));
            push_unique_path(
                &mut paths,
                exe_dir
                    .join("../Resources")
                    .join(GOOGLE_CREDENTIALS_FILE_NAME),
            );
        }
    }

    #[cfg(debug_assertions)]
    {
        if let Ok(current_dir) = std::env::current_dir() {
            push_unique_path(&mut paths, current_dir.join(GOOGLE_CREDENTIALS_FILE_NAME));
            push_unique_path(
                &mut paths,
                current_dir
                    .join("src-tauri")
                    .join(GOOGLE_CREDENTIALS_FILE_NAME),
            );
        }

        // `src-tauri/`, where a developer keeps the file.
        push_unique_path(
            &mut paths,
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(GOOGLE_CREDENTIALS_FILE_NAME),
        );
    }

    paths
}

impl OAuthClient {
    /// Finds the client ID and secret: run-time environment variables, then build-time ones, then a `google-credentials.json` file.
    pub fn load() -> Result<Self, String> {
        if let Ok(credentials) = oauth_from_pair(
            std::env::var(GOOGLE_CLIENT_ID_ENV).ok(),
            std::env::var(GOOGLE_CLIENT_SECRET_ENV).ok(),
            "runtime environment variables",
        ) {
            return Ok(credentials);
        }

        if let Ok(credentials) = oauth_from_pair(
            Some(CLIENT_ID.to_string()),
            Some(CLIENT_SECRET.to_string()),
            "build-time environment variables",
        ) {
            return Ok(credentials);
        }

        let mut file_errors = Vec::new();
        for path in oauth_credentials_candidate_paths() {
            if !path.exists() {
                continue;
            }

            match load_oauth_credentials_from_file(&path) {
                Ok(credentials) => return Ok(credentials),
                Err(error) => file_errors.push(error),
            }
        }

        if file_errors.is_empty() {
            Err(oauth_credentials_error())
        } else {
            Err(format!(
                "{} Tried credential files: {}",
                oauth_credentials_error(),
                file_errors.join(" | ")
            ))
        }
    }
}

/// The keychain service of every Google sign-in. The user is the account's email.
pub const KEYRING_SERVICE: &str = "marlin-gdrive";

/// The keychain entry that holds an account's refresh token.
pub fn keyring_entry(email: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, email)
        .map_err(|e| format!("Failed to create keyring entry: {}", e))
}

/// Why an account has no access token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInError {
    /// The keychain holds no sign-in for the account. The user must sign in again.
    NoSavedSignIn,
    /// Google answered `invalid_grant`: it no longer accepts the saved sign-in. The user must sign in again.
    Rejected,
    /// Google could not be reached. The text is the transport error.
    Offline(String),
    /// Anything else: the keychain could not be read, or Google sent an answer that is not a token. The text holds no token and no client secret.
    Other(String),
}

/// What this process knows of an account's saved sign-in.
enum Saved {
    /// Nothing read from the keychain yet.
    Unread,
    /// A refresh token Google has not refused, and the access token it last gave with the time that one expires.
    Held {
        refresh_token: String,
        access: Option<(String, SystemTime)>,
    },
    /// Google refused this refresh token. It is never sent again.
    Refused(String),
}

/// One Google account's sign-in, as the app saved it. One value per account per process.
pub struct SavedSignIn {
    email: String,
    client: OAuthClient,
    http: reqwest::Client,
    /// Held across the keychain read and the refresh request, so calls that arrive together cause one of each. macOS can ask the user on every keychain read.
    refreshing: tokio::sync::Mutex<()>,
    /// Never held across an `.await`.
    saved: std::sync::Mutex<Saved>,
}

impl SavedSignIn {
    /// For an account the keychain already holds a sign-in for.
    pub fn new(email: &str, client: OAuthClient) -> Self {
        Self {
            email: email.to_string(),
            client,
            http: reqwest::Client::new(),
            refreshing: tokio::sync::Mutex::new(()),
            saved: std::sync::Mutex::new(Saved::Unread),
        }
    }

    /// For an account that signed in a moment ago in this process: no keychain read and no refresh until `expires_at`.
    pub fn signed_in(
        email: &str,
        client: OAuthClient,
        refresh_token: String,
        access_token: String,
        expires_at: SystemTime,
    ) -> Self {
        Self {
            saved: std::sync::Mutex::new(Saved::Held {
                refresh_token,
                access: Some((access_token, expires_at)),
            }),
            ..Self::new(email, client)
        }
    }

    /// An access token Google should accept now. Refreshes when the kept one is missing or ends within 5 minutes.
    pub async fn valid_token(&self) -> Result<String, SignInError> {
        let _refreshing = self.refreshing.lock().await;

        let held = match &*self.saved.lock().unwrap() {
            Saved::Held {
                access: Some((token, expires_at)),
                ..
            } if *expires_at > SystemTime::now() + Duration::from_secs(5 * 60) => {
                return Ok(token.clone());
            }
            Saved::Held { refresh_token, .. } => Some(refresh_token.clone()),
            // The app is the only process that signs in, and it overwrites the keychain entry. So both read it again.
            Saved::Unread | Saved::Refused(_) => None,
        };

        let refresh_token = match held {
            Some(refresh_token) => refresh_token,
            None => {
                let read = keyring_entry(&self.email)
                    .map_err(SignInError::Other)?
                    .get_password()
                    .map_err(|error| match error {
                        keyring::Error::NoEntry => SignInError::NoSavedSignIn,
                        error => SignInError::Other(format!(
                            "Failed to read Google sign-in from keychain: {error}"
                        )),
                    })?;
                let mut saved = self.saved.lock().unwrap();
                // A mount whose sign-in is gone gets a call for every file the OS touches, and each would otherwise ask Google again with a token Google already refused.
                if matches!(&*saved, Saved::Refused(refused) if *refused == read) {
                    return Err(SignInError::Rejected);
                }
                // Kept before the request. A refresh that fails because Google cannot be reached must not cause a second keychain read: macOS can ask the user on every read.
                *saved = Saved::Held {
                    refresh_token: read.clone(),
                    access: None,
                };
                read
            }
        };

        let response = self
            .http
            .post(&self.client.token_uri)
            .form(&[
                ("client_id", self.client.client_id.as_str()),
                ("client_secret", self.client.client_secret.as_str()),
                ("refresh_token", refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            // The `refreshing` lock stays held while this runs.
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|error| SignInError::Offline(error.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            #[derive(Deserialize)]
            struct RefreshError {
                error: String,
            }
            let body = response.bytes().await.unwrap_or_default();
            let reason = serde_json::from_slice::<RefreshError>(&body)
                .map(|body| body.error)
                .unwrap_or_default();
            if reason == "invalid_grant" {
                *self.saved.lock().unwrap() = Saved::Refused(refresh_token);
                return Err(SignInError::Rejected);
            }
            return Err(SignInError::Other(format!(
                "Token refresh failed (status {status})."
            )));
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            expires_in: u64,
            #[serde(default)]
            refresh_token: Option<String>,
        }

        let body = response
            .bytes()
            .await
            .map_err(|error| SignInError::Offline(error.to_string()))?;
        let token_response: TokenResponse = serde_json::from_slice(&body).map_err(|_| {
            SignInError::Other("Google sent a token answer Marlin could not read".into())
        })?;

        let mut refresh_token = refresh_token;
        if let Some(new_refresh_token) = token_response.refresh_token {
            if new_refresh_token != refresh_token {
                // The refresh worked, so a failed keychain write must not fail the call.
                if let Ok(entry) = keyring_entry(&self.email) {
                    let _ = entry.set_password(&new_refresh_token);
                }
                refresh_token = new_refresh_token;
            }
        }

        *self.saved.lock().unwrap() = Saved::Held {
            refresh_token,
            access: Some((
                token_response.access_token.clone(),
                SystemTime::now() + Duration::from_secs(token_response.expires_in),
            )),
        };
        Ok(token_response.access_token)
    }
}

#[async_trait::async_trait]
impl AccessTokens for SavedSignIn {
    async fn access_token(&self) -> Result<String, VfsError> {
        self.valid_token().await.map_err(|error| match error {
            SignInError::NoSavedSignIn | SignInError::Rejected => VfsError::SignInNeeded,
            SignInError::Offline(_) => VfsError::Offline,
            // A fixed text: the helper logs it, and a keychain error's text comes from the OS.
            SignInError::Other(_) => {
                VfsError::Other("Marlin could not use the saved Google sign-in".into())
            }
        })
    }

    fn rejected(&self, token: &str) {
        if let Saved::Held { access, .. } = &mut *self.saved.lock().unwrap() {
            if matches!(access, Some((kept, _)) if kept == token) {
                *access = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{load_oauth_credentials_from_file, oauth_from_pair};
    use std::fs;

    #[test]
    fn oauth_from_pair_rejects_partial_credentials() {
        let result = oauth_from_pair(Some("client".into()), None, "test");
        assert!(result.is_err());
    }

    #[test]
    fn loads_google_credentials_json_installed_shape() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("google-credentials.json");
        fs::write(
            &path,
            r#"{
              "installed": {
                "client_id": "client-id.apps.googleusercontent.com",
                "client_secret": "client-secret"
              }
            }"#,
        )
        .unwrap();

        let credentials = load_oauth_credentials_from_file(&path).unwrap();
        assert_eq!(
            credentials.client_id,
            "client-id.apps.googleusercontent.com"
        );
        assert_eq!(credentials.client_secret, "client-secret");
    }

    #[test]
    fn loads_google_credentials_json_flat_shape() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("google-credentials.json");
        fs::write(
            &path,
            r#"{
              "client_id": "flat-client-id",
              "client_secret": "flat-client-secret"
            }"#,
        )
        .unwrap();

        let credentials = load_oauth_credentials_from_file(&path).unwrap();
        assert_eq!(credentials.client_id, "flat-client-id");
        assert_eq!(credentials.client_secret, "flat-client-secret");
    }

    #[test]
    fn a_credentials_file_that_is_not_the_expected_json_keeps_its_text_out_of_the_error() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("google-credentials.json");
        fs::write(
            &path,
            r#"{"installed": "client-secret-in-the-wrong-place"}"#,
        )
        .unwrap();

        let Err(error) = load_oauth_credentials_from_file(&path) else {
            panic!("expected an error");
        };
        assert!(
            !error.contains("client-secret-in-the-wrong-place"),
            "{error}"
        );
        assert!(error.contains("line 1"), "{error}");
    }
}
