use chrono::{DateTime, Duration, Utc};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;
use yup_oauth2::authenticator_delegate::InstalledFlowDelegate;
use yup_oauth2::storage::{TokenInfo, TokenStorage, TokenStorageError};
use yup_oauth2::{
    ApplicationSecret, InstalledFlowAuthenticator, InstalledFlowReturnMethod,
    ServiceAccountAuthenticator, ServiceAccountKey,
};

/// Google API OAuth credentials embedded at build time when available.
/// These remain as a fallback, but runtime sources are preferred so refresh
/// can keep working even if the current binary was built without them.
const CLIENT_ID: &str = match option_env!("GOOGLE_CLIENT_ID") {
    Some(id) => id,
    None => "",
};
const CLIENT_SECRET: &str = match option_env!("GOOGLE_CLIENT_SECRET") {
    Some(secret) => secret,
    None => "",
};

/// Scopes needed for Google Drive access
const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/drive",
    "https://www.googleapis.com/auth/userinfo.email",
];

/// Environment variable for service account key file path
const SERVICE_ACCOUNT_KEY_FILE_ENV: &str = "GOOGLE_SERVICE_ACCOUNT_KEY_FILE";
const GOOGLE_CLIENT_ID_ENV: &str = "GOOGLE_CLIENT_ID";
const GOOGLE_CLIENT_SECRET_ENV: &str = "GOOGLE_CLIENT_SECRET";
const GOOGLE_CREDENTIALS_FILE_NAME: &str = "google-credentials.json";

/// Cached service account key (loaded once from environment)
static SERVICE_ACCOUNT_KEY: Lazy<Option<ServiceAccountKey>> =
    Lazy::new(|| load_service_account_key().ok());

/// Load service account key from environment variable
fn load_service_account_key() -> Result<ServiceAccountKey, String> {
    let key_path = std::env::var(SERVICE_ACCOUNT_KEY_FILE_ENV)
        .map_err(|_| format!("{} not set", SERVICE_ACCOUNT_KEY_FILE_ENV))?;

    let key_json = fs::read_to_string(&key_path)
        .map_err(|e| format!("Failed to read service account key file: {}", e))?;

    let key: ServiceAccountKey = serde_json::from_str(&key_json)
        .map_err(|e| format!("Failed to parse service account key: {}", e))?;

    log::info!("Loaded service account: {:?}", key.client_email);

    Ok(key)
}

/// Get the service account email if configured
pub fn get_service_account_email() -> Option<String> {
    SERVICE_ACCOUNT_KEY.as_ref().map(|k| k.client_email.clone())
}

/// Check if the given email is the service account
pub fn is_service_account_email(email: &str) -> bool {
    get_service_account_email().map_or(false, |sa_email| sa_email == email)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OAuthCredentials {
    client_id: String,
    client_secret: String,
}

#[derive(Debug, Deserialize)]
struct OAuthCredentialsFile {
    installed: Option<OAuthClientSecret>,
    web: Option<OAuthClientSecret>,
    client_id: Option<String>,
    client_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
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
) -> Result<OAuthCredentials, String> {
    let client_id = normalize_secret(client_id);
    let client_secret = normalize_secret(client_secret);

    match (client_id, client_secret) {
        (Some(client_id), Some(client_secret)) => Ok(OAuthCredentials {
            client_id,
            client_secret,
        }),
        (Some(_), None) | (None, Some(_)) => Err(format!(
            "{} is missing part of the OAuth client secret",
            source
        )),
        (None, None) => Err(format!("{} not configured", source)),
    }
}

fn load_oauth_credentials_from_file(path: &Path) -> Result<OAuthCredentials, String> {
    let contents = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    let parsed: OAuthCredentialsFile = serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;

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

        push_unique_path(
            &mut paths,
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(GOOGLE_CREDENTIALS_FILE_NAME),
        );
    }

    paths
}

fn load_oauth_credentials() -> Result<OAuthCredentials, String> {
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

/// Information about a connected Google account
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleAccountInfo {
    pub email: String,
    pub display_name: Option<String>,
    pub photo_url: Option<String>,
}

/// Storage structure for accounts file
#[derive(Serialize)]
struct AccountStorage {
    accounts: Vec<GoogleAccountInfo>,
}

/// What this process knows about an account's tokens. The access token lives only here.
struct AccountTokens {
    refresh_token: String,
    /// The access token and when it expires. `None` until a refresh has worked in this run.
    access: Option<(String, DateTime<Utc>)>,
}

// Two locks own all Google account state. `TOKENS` covers the keychain entries and the
// tokens in memory. `ACCOUNTS_CACHE` covers the accounts file and its cached copy.
// Take `TOKENS` first, then `ACCOUNTS_CACHE`, and never hold `ACCOUNTS_CACHE` across an `.await`.

/// Held across the keychain read and the refresh request, so calls that arrive together
/// cause one of each. macOS can ask the user on every keychain read.
static TOKENS: Lazy<Mutex<HashMap<String, AccountTokens>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// In-memory cache of accounts
static ACCOUNTS_CACHE: Lazy<StdMutex<Option<Vec<GoogleAccountInfo>>>> =
    Lazy::new(|| StdMutex::new(None));

/// Mutex to prevent concurrent auth flows
static AUTH_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

/// Get the path to the accounts storage file
fn get_accounts_path() -> Result<PathBuf, String> {
    // Reading the accounts file can rewrite it, and unit tests swap the keychain for a fake one.
    if cfg!(test) {
        return Err("A unit test must not open the real Google accounts file".to_string());
    }

    let config_dir =
        dirs::config_dir().ok_or_else(|| "Could not determine config directory".to_string())?;
    let marlin_dir = config_dir.join("marlin");

    // Ensure directory exists
    if !marlin_dir.exists() {
        fs::create_dir_all(&marlin_dir)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }

    Ok(marlin_dir.join("gdrive-accounts.json"))
}

const GDRIVE_KEYRING_SERVICE: &str = "marlin-gdrive";

fn keyring_entry(email: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(GDRIVE_KEYRING_SERVICE, email)
        .map_err(|e| format!("Failed to create keyring entry: {}", e))
}

fn set_refresh_token(email: &str, refresh_token: &str) -> Result<(), String> {
    let entry = keyring_entry(email)?;
    entry
        .set_password(refresh_token)
        .map_err(|e| format!("Failed to store Google sign-in in keychain: {}", e))
}

fn delete_refresh_token(email: &str) -> Result<(), String> {
    let entry = keyring_entry(email)?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!(
            "Failed to delete Google sign-in from keychain: {}",
            e
        )),
    }
}

/// Reads the account list. A file written by an older version also holds tokens: this moves
/// each refresh token into the keychain and rewrites the file without them.
fn read_accounts_file_moving_tokens(path: &Path) -> Result<Vec<GoogleAccountInfo>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let contents =
        fs::read_to_string(path).map_err(|e| format!("Failed to read accounts file: {}", e))?;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GoogleAccountDisk {
        email: String,
        display_name: Option<String>,
        photo_url: Option<String>,
        #[serde(default)]
        access_token: Option<String>,
        #[serde(default)]
        refresh_token: Option<String>,
    }

    #[derive(Deserialize)]
    struct AccountStorageDisk {
        accounts: Vec<GoogleAccountDisk>,
    }

    let storage: AccountStorageDisk = serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse accounts file: {}", e))?;

    let mut had_tokens = false;
    let mut accounts = Vec::with_capacity(storage.accounts.len());
    for account in storage.accounts {
        if account.access_token.is_some() || account.refresh_token.is_some() {
            had_tokens = true;
        }
        if let Some(refresh_token) = account.refresh_token {
            set_refresh_token(&account.email, &refresh_token)?;
        }
        accounts.push(GoogleAccountInfo {
            email: account.email,
            display_name: account.display_name,
            photo_url: account.photo_url,
        });
    }

    // Every keychain write has succeeded by now, so a failed one leaves the file as it was.
    if had_tokens {
        write_accounts_file(path, &accounts)?;
    }

    Ok(accounts)
}

fn write_accounts_file(path: &Path, accounts: &[GoogleAccountInfo]) -> Result<(), String> {
    let storage = AccountStorage {
        accounts: accounts.to_vec(),
    };

    let contents = serde_json::to_string_pretty(&storage)
        .map_err(|e| format!("Failed to serialize accounts: {}", e))?;

    // A failed write leaves the old file whole: rename replaces it only once the new one is written.
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, contents).map_err(|e| format!("Failed to write accounts file: {}", e))?;
    fs::rename(&tmp_path, path).map_err(|e| format!("Failed to write accounts file: {}", e))?;

    Ok(())
}

/// Get all connected Google accounts
pub fn get_google_accounts() -> Result<Vec<GoogleAccountInfo>, String> {
    let mut result = Vec::new();

    // Include service account if configured
    if let Some(sa_email) = get_service_account_email() {
        result.push(GoogleAccountInfo {
            email: sa_email,
            display_name: Some("Service Account".to_string()),
            photo_url: None,
        });
    }

    let mut cache = ACCOUNTS_CACHE.lock().map_err(|e| e.to_string())?;
    if cache.is_none() {
        *cache = Some(read_accounts_file_moving_tokens(&get_accounts_path()?)?);

        // A sign-in by an older version can leave a token file in this folder.
        if let Some(cache_dir) = dirs::cache_dir() {
            let _ = fs::remove_dir_all(cache_dir.join("marlin").join("oauth_temp"));
        }
    }
    if let Some(accounts) = &*cache {
        result.extend(accounts.iter().cloned());
    }

    Ok(result)
}

/// Get an access token for the service account
async fn get_service_account_token() -> Result<String, String> {
    let key = SERVICE_ACCOUNT_KEY
        .as_ref()
        .ok_or_else(|| "Service account not configured".to_string())?;

    let auth = ServiceAccountAuthenticator::builder(key.clone())
        .build()
        .await
        .map_err(|e| format!("Failed to build service account authenticator: {}", e))?;

    let token = auth
        .token(SCOPES)
        .await
        .map_err(|e| format!("Failed to get service account token: {}", e))?;

    token
        .token()
        .map(|t| t.to_string())
        .ok_or_else(|| "No access token in service account response".to_string())
}

/// Custom flow delegate that opens the browser
struct BrowserFlowDelegate;

impl InstalledFlowDelegate for BrowserFlowDelegate {
    fn present_user_url<'a>(
        &'a self,
        url: &'a str,
        _need_code: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async move {
            // Open the URL in the default browser
            if let Err(e) = open::that(url) {
                log::error!("Failed to open browser: {}", e);
                return Err(format!("Failed to open browser: {}", e));
            }

            // The authenticator will handle the redirect
            Ok(String::new())
        })
    }

    fn redirect_uri(&self) -> Option<&str> {
        None // Use default localhost redirect
    }
}

/// Keeps a sign-in's tokens in memory, so the OAuth library never writes them to a file.
struct SignInTokens(Arc<StdMutex<Option<TokenInfo>>>);

#[async_trait::async_trait]
impl TokenStorage for SignInTokens {
    async fn set(&self, _scopes: &[&str], token: TokenInfo) -> Result<(), TokenStorageError> {
        let mut guard = self
            .0
            .lock()
            .map_err(|e| TokenStorageError::Other(e.to_string().into()))?;
        *guard = Some(token);
        Ok(())
    }

    async fn get(&self, _scopes: &[&str]) -> Option<TokenInfo> {
        self.0.lock().ok().and_then(|guard| guard.clone())
    }
}

/// Add a new Google account via OAuth flow
pub async fn add_google_account() -> Result<GoogleAccountInfo, String> {
    let oauth_credentials = load_oauth_credentials()?;

    // Prevent concurrent auth flows
    let _guard = AUTH_MUTEX.lock().await;

    log::info!("Starting Google OAuth flow...");

    let secret = ApplicationSecret {
        client_id: oauth_credentials.client_id,
        client_secret: oauth_credentials.client_secret,
        auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
        token_uri: "https://oauth2.googleapis.com/token".to_string(),
        redirect_uris: vec![
            "http://127.0.0.1".to_string(),
            "http://localhost".to_string(),
        ],
        ..Default::default()
    };

    let stored = Arc::new(StdMutex::new(None));

    // Build the authenticator
    log::info!("Building authenticator...");
    // Use HTTPRedirect which automatically finds an available port
    let auth = InstalledFlowAuthenticator::builder(secret, InstalledFlowReturnMethod::HTTPRedirect)
        .with_storage(Box::new(SignInTokens(stored.clone())))
        .flow_delegate(Box::new(BrowserFlowDelegate))
        .build()
        .await
        .map_err(|e| {
            log::error!("Failed to build authenticator: {}", e);
            format!("Failed to build authenticator: {}", e)
        })?;

    log::info!("Authenticator built, requesting token...");

    // Get a token to trigger the auth flow
    auth.token(SCOPES).await.map_err(|e| {
        log::error!("Failed to get token: {}", e);
        format!("Failed to get token: {}", e)
    })?;

    log::info!("Token received!");

    let token_info = stored
        .lock()
        .map_err(|e| e.to_string())?
        .take()
        .ok_or_else(|| "No access token received".to_string())?;
    let expires_at = token_info
        .expires_at
        .and_then(|t| DateTime::from_timestamp(t.unix_timestamp(), 0))
        .unwrap_or_else(|| Utc::now() + Duration::hours(1));
    let access_token = token_info
        .access_token
        .ok_or_else(|| "No access token received".to_string())?;
    let refresh_token = token_info
        .refresh_token
        .ok_or_else(|| "No refresh token in response".to_string())?;

    // Get user info to get the email
    log::info!("Fetching user info...");
    let user_info = fetch_user_info(&access_token).await.map_err(|e| {
        log::error!("Failed to fetch user info: {}", e);
        e
    })?;
    log::info!("User info received: {}", user_info.email);

    let mut tokens = TOKENS.lock().await;
    let mut cache = ACCOUNTS_CACHE.lock().map_err(|e| e.to_string())?;

    // Check if account already exists
    let path = get_accounts_path()?;
    let mut accounts = read_accounts_file_moving_tokens(&path).map_err(|e| {
        log::error!("Failed to load accounts: {}", e);
        e
    })?;
    log::info!("Loaded {} existing accounts", accounts.len());

    set_refresh_token(&user_info.email, &refresh_token)?;

    if let Some(existing) = accounts.iter_mut().find(|a| a.email == user_info.email) {
        // Update existing account
        log::info!("Updating existing account: {}", user_info.email);
        existing.display_name = user_info.display_name.clone();
        existing.photo_url = user_info.photo_url.clone();
    } else {
        // Add new account
        log::info!("Adding new account: {}", user_info.email);
        accounts.push(user_info.clone());
    }

    write_accounts_file(&path, &accounts).map_err(|e| {
        log::error!("Failed to save accounts: {}", e);
        e
    })?;
    log::info!("Accounts saved to disk");

    *cache = Some(accounts);
    tokens.insert(
        user_info.email.clone(),
        AccountTokens {
            refresh_token,
            access: Some((access_token, expires_at)),
        },
    );
    log::info!("Account cache updated, returning user info");

    Ok(user_info)
}

/// Fetch user info from Google
async fn fetch_user_info(access_token: &str) -> Result<GoogleAccountInfo, String> {
    let client = reqwest::Client::new();

    let response = client
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch user info: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("User info request failed: {}", response.status()));
    }

    #[derive(Deserialize)]
    struct UserInfo {
        email: String,
        name: Option<String>,
        picture: Option<String>,
    }

    let user_info: UserInfo = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse user info: {}", e))?;

    Ok(GoogleAccountInfo {
        email: user_info.email,
        display_name: user_info.name,
        photo_url: user_info.picture,
    })
}

/// Remove a Google account
pub async fn remove_google_account(email: &str) -> Result<(), String> {
    let mut tokens = TOKENS.lock().await;
    let mut cache = ACCOUNTS_CACHE.lock().map_err(|e| e.to_string())?;
    let path = get_accounts_path()?;
    let mut accounts = read_accounts_file_moving_tokens(&path)?;

    let original_len = accounts.len();
    accounts.retain(|a| a.email != email);

    if accounts.len() == original_len {
        return Err(format!("Account not found: {}", email));
    }

    delete_refresh_token(email)?;
    write_accounts_file(&path, &accounts)?;
    *cache = Some(accounts);
    tokens.remove(email);

    Ok(())
}

/// Refresh an access token if needed
pub async fn ensure_valid_token(email: &str) -> Result<String, String> {
    // Check if this is a service account - use JWT-based auth
    if is_service_account_email(email) {
        log::info!("Using service account authentication for {}", email);
        return get_service_account_token().await;
    }

    let mut tokens = TOKENS.lock().await;
    let refresh_token = match tokens.get(email) {
        Some(AccountTokens {
            access: Some((access_token, expires_at)),
            ..
        }) if *expires_at > Utc::now() + Duration::minutes(5) => {
            return Ok(access_token.clone());
        }
        Some(kept) => kept.refresh_token.clone(),
        None => {
            // Listing the accounts moves an older file's tokens into the keychain first.
            let accounts = get_google_accounts()?;
            if !accounts.iter().any(|account| account.email == email) {
                return Err(format!("Account not found: {}", email));
            }
            let refresh_token = keyring_entry(email)?.get_password().map_err(|e| {
                format!(
                    "No Google sign-in stored for {}. Re-authentication may be required. ({})",
                    email, e
                )
            })?;
            tokens.insert(
                email.to_string(),
                AccountTokens {
                    refresh_token: refresh_token.clone(),
                    access: None,
                },
            );
            refresh_token
        }
    };

    let oauth_credentials = load_oauth_credentials()?;
    let client = reqwest::Client::new();

    let response = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", oauth_credentials.client_id.as_str()),
            ("client_secret", oauth_credentials.client_secret.as_str()),
            ("refresh_token", refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ])
        // `TOKENS` stays locked while this runs.
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Failed to refresh token: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        // Google's `error` code (`invalid_grant`, `invalid_client`) holds no token.
        #[derive(Deserialize)]
        struct RefreshError {
            error: String,
        }
        let reason = response
            .json::<RefreshError>()
            .await
            .map(|body| body.error)
            .unwrap_or_default();
        log::error!("Token refresh failed with status {}: {}", status, reason);
        return Err(format!(
            "Token refresh failed (status {}). Re-authentication may be required.",
            status
        ));
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        expires_in: i64,
        #[serde(default)]
        refresh_token: Option<String>,
    }

    let token_response: TokenResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse token response: {}", e))?;

    let mut refresh_token = refresh_token;
    if let Some(new_refresh_token) = token_response.refresh_token {
        if new_refresh_token != refresh_token {
            // The refresh worked, so a failed keychain write must not fail the call.
            if let Err(e) = set_refresh_token(email, &new_refresh_token) {
                log::warn!("{}", e);
            }
            refresh_token = new_refresh_token;
        }
    }

    tokens.insert(
        email.to_string(),
        AccountTokens {
            refresh_token,
            access: Some((
                token_response.access_token.clone(),
                Utc::now() + Duration::seconds(token_response.expires_in),
            )),
        },
    );

    Ok(token_response.access_token)
}

#[cfg(test)]
mod tests {
    use super::{
        delete_refresh_token, load_oauth_credentials_from_file, oauth_from_pair,
        read_accounts_file_moving_tokens, write_accounts_file, GoogleAccountInfo, OAuthCredentials,
        GDRIVE_KEYRING_SERVICE, GOOGLE_CLIENT_ID_ENV, GOOGLE_CLIENT_SECRET_ENV,
    };
    use crate::locations::test_keychain;
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
        assert_eq!(
            credentials,
            OAuthCredentials {
                client_id: "flat-client-id".into(),
                client_secret: "flat-client-secret".into(),
            }
        );
    }

    #[test]
    fn runtime_env_var_names_remain_stable() {
        assert_eq!(GOOGLE_CLIENT_ID_ENV, "GOOGLE_CLIENT_ID");
        assert_eq!(GOOGLE_CLIENT_SECRET_ENV, "GOOGLE_CLIENT_SECRET");
    }

    #[test]
    fn read_accounts_file_moves_tokens_to_the_keychain() {
        test_keychain::install();
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("gdrive-accounts.json");
        let ada = "move-ada@example.com";
        let bea = "move-bea@example.com";
        let access_ada = "access-token-move-ada";
        let refresh_ada = "refresh-token-move-ada";
        let access_bea = "access-token-move-bea";
        let refresh_bea = "refresh-token-move-bea";
        fs::write(
            &path,
            format!(
                r#"{{
                  "accounts": [
                    {{
                      "email": "{ada}",
                      "displayName": "Ada",
                      "photoUrl": "https://example.com/ada.png",
                      "accessToken": "{access_ada}",
                      "refreshToken": "{refresh_ada}",
                      "expiresAt": "2020-01-01T00:00:00Z"
                    }},
                    {{
                      "email": "{bea}",
                      "displayName": "Bea",
                      "photoUrl": "https://example.com/bea.png",
                      "accessToken": "{access_bea}",
                      "refreshToken": "{refresh_bea}",
                      "expiresAt": "2020-01-01T00:00:00Z"
                    }}
                  ]
                }}"#
            ),
        )
        .unwrap();

        let accounts = read_accounts_file_moving_tokens(&path).unwrap();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].email, ada);
        assert_eq!(accounts[0].display_name.as_deref(), Some("Ada"));
        assert_eq!(
            accounts[0].photo_url.as_deref(),
            Some("https://example.com/ada.png")
        );
        assert_eq!(accounts[1].email, bea);
        assert_eq!(accounts[1].display_name.as_deref(), Some("Bea"));
        assert_eq!(
            accounts[1].photo_url.as_deref(),
            Some("https://example.com/bea.png")
        );
        assert_eq!(
            test_keychain::get(GDRIVE_KEYRING_SERVICE, ada).as_deref(),
            Some(refresh_ada)
        );
        assert_eq!(
            test_keychain::get(GDRIVE_KEYRING_SERVICE, bea).as_deref(),
            Some(refresh_bea)
        );

        let rewritten = fs::read_to_string(&path).unwrap();
        for secret in [access_ada, refresh_ada, access_bea, refresh_bea] {
            assert!(!rewritten.contains(secret), "{rewritten}");
        }
        assert!(!rewritten.contains("accessToken"));
        assert!(!rewritten.contains("refreshToken"));

        let again = read_accounts_file_moving_tokens(&path).unwrap();
        assert_eq!(again.len(), accounts.len());
        for (left, right) in again.iter().zip(&accounts) {
            assert_eq!(left.email, right.email);
            assert_eq!(left.display_name, right.display_name);
            assert_eq!(left.photo_url, right.photo_url);
        }
    }

    #[test]
    fn read_accounts_file_keeps_the_file_when_the_keychain_fails() {
        test_keychain::install();
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("gdrive-accounts.json");
        let email = "keychain-fails@example.com";
        let original = format!(
            r#"{{"accounts":[{{"email":"{email}","displayName":"Fail","photoUrl":null,"accessToken":"access-fail","refreshToken":"refresh-fail","expiresAt":"2020-01-01T00:00:00Z"}}]}}"#
        );
        fs::write(&path, &original).unwrap();
        test_keychain::fail_writes_for(GDRIVE_KEYRING_SERVICE, email);

        let err = read_accounts_file_moving_tokens(&path).unwrap_err();
        assert!(
            err.contains("Failed to store Google sign-in in keychain"),
            "{err}"
        );
        assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
    }

    #[test]
    fn read_accounts_file_rewrites_a_file_with_only_an_access_token() {
        test_keychain::install();
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("gdrive-accounts.json");
        let email = "access-only@example.com";
        let access_token = "access-token-only";
        fs::write(
            &path,
            format!(
                r#"{{
                  "accounts": [
                    {{
                      "email": "{email}",
                      "displayName": "Access",
                      "photoUrl": null,
                      "accessToken": "{access_token}"
                    }}
                  ]
                }}"#
            ),
        )
        .unwrap();

        let accounts = read_accounts_file_moving_tokens(&path).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].email, email);
        assert_eq!(accounts[0].display_name.as_deref(), Some("Access"));
        let rewritten = fs::read_to_string(&path).unwrap();
        assert!(!rewritten.contains(access_token), "{rewritten}");
        assert!(test_keychain::get(GDRIVE_KEYRING_SERVICE, email).is_none());
    }

    #[test]
    fn read_accounts_file_leaves_a_file_without_tokens_alone() {
        test_keychain::install();
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("gdrive-accounts.json");
        let original = "{\"accounts\":[ {\"email\":\"plain-spacing@example.com\", \"displayName\":\"Pat\", \"photoUrl\":null} ]}";
        fs::write(&path, original).unwrap();

        let accounts = read_accounts_file_moving_tokens(&path).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].email, "plain-spacing@example.com");
        assert_eq!(accounts[0].display_name.as_deref(), Some("Pat"));
        assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
    }

    #[test]
    fn delete_refresh_token_ignores_a_missing_entry() {
        test_keychain::install();
        let email = "delete-twice@example.com";
        test_keychain::set(GDRIVE_KEYRING_SERVICE, email, "secret");

        delete_refresh_token(email).unwrap();
        assert!(test_keychain::get(GDRIVE_KEYRING_SERVICE, email).is_none());
        delete_refresh_token(email).unwrap();
    }

    #[test]
    fn write_accounts_file_replaces_the_file_and_leaves_no_temp_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("gdrive-accounts.json");
        fs::write(&path, "old").unwrap();

        write_accounts_file(
            &path,
            &[GoogleAccountInfo {
                email: "write-replace@example.com".to_string(),
                display_name: Some("Writer".to_string()),
                photo_url: None,
            }],
        )
        .unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("write-replace@example.com"), "{written}");
        assert!(!written.contains("old"), "{written}");
        let names: Vec<_> = fs::read_dir(temp_dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1);
    }
}
