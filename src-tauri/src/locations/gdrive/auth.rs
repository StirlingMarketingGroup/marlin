use chrono::{DateTime, Duration, Utc};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use tokio::sync::Mutex;
use yup_oauth2::authenticator_delegate::InstalledFlowDelegate;
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

/// Check if a service account is configured
#[allow(dead_code)] // Useful for checking service account availability
pub fn is_service_account_configured() -> bool {
    SERVICE_ACCOUNT_KEY.is_some()
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

/// Stored account data with tokens
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleAccount {
    pub email: String,
    pub display_name: Option<String>,
    pub photo_url: Option<String>,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
}

/// Storage structure for accounts file
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AccountStorage {
    accounts: Vec<GoogleAccount>,
}

/// In-memory cache of accounts
static ACCOUNTS_CACHE: Lazy<RwLock<Option<Vec<GoogleAccount>>>> = Lazy::new(|| RwLock::new(None));

/// Mutex to prevent concurrent auth flows
static AUTH_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

/// Get the path to the accounts storage file
fn get_accounts_path() -> Result<PathBuf, String> {
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

/// Load accounts from disk
fn load_accounts_from_disk() -> Result<Vec<GoogleAccount>, String> {
    let path = get_accounts_path()?;

    if !path.exists() {
        return Ok(Vec::new());
    }

    let contents =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read accounts file: {}", e))?;

    let storage: AccountStorage = serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse accounts file: {}", e))?;

    Ok(storage.accounts)
}

/// Save accounts to disk
fn save_accounts_to_disk(accounts: &[GoogleAccount]) -> Result<(), String> {
    let path = get_accounts_path()?;

    let storage = AccountStorage {
        accounts: accounts.to_vec(),
    };

    let contents = serde_json::to_string_pretty(&storage)
        .map_err(|e| format!("Failed to serialize accounts: {}", e))?;

    fs::write(&path, contents).map_err(|e| format!("Failed to write accounts file: {}", e))?;

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

    // Check cache first for user accounts
    {
        let cache = ACCOUNTS_CACHE.read().map_err(|e| e.to_string())?;
        if let Some(accounts) = &*cache {
            result.extend(accounts.iter().map(|a| GoogleAccountInfo {
                email: a.email.clone(),
                display_name: a.display_name.clone(),
                photo_url: a.photo_url.clone(),
            }));
            return Ok(result);
        }
    }

    // Load from disk
    let accounts = load_accounts_from_disk()?;

    // Update cache
    {
        let mut cache = ACCOUNTS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(accounts.clone());
    }

    result.extend(accounts.iter().map(|a| GoogleAccountInfo {
        email: a.email.clone(),
        display_name: a.display_name.clone(),
        photo_url: a.photo_url.clone(),
    }));

    Ok(result)
}

/// Get all accounts (internal use for iterating)
pub fn get_all_accounts() -> Result<Vec<GoogleAccount>, String> {
    let mut result = Vec::new();

    // Include service account if configured (with placeholder tokens - real token fetched on demand)
    if let Some(sa_email) = get_service_account_email() {
        result.push(GoogleAccount {
            email: sa_email,
            display_name: Some("Service Account".to_string()),
            photo_url: None,
            access_token: String::new(),  // Will be fetched on demand
            refresh_token: String::new(), // Service accounts use JWT, not refresh tokens
            expires_at: Utc::now(),       // Force refresh on first use
        });
    }

    // Check cache first for user accounts
    {
        let cache = ACCOUNTS_CACHE.read().map_err(|e| e.to_string())?;
        if let Some(accounts) = &*cache {
            result.extend(accounts.clone());
            return Ok(result);
        }
    }

    // Load from disk
    let accounts = load_accounts_from_disk()?;

    // Update cache
    {
        let mut cache = ACCOUNTS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(accounts.clone());
    }

    result.extend(accounts);
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

    // Create a temporary directory for the token cache
    let token_cache_dir = dirs::cache_dir()
        .ok_or_else(|| "Could not determine cache directory".to_string())?
        .join("marlin")
        .join("oauth_temp");

    fs::create_dir_all(&token_cache_dir)
        .map_err(|e| format!("Failed to create token cache directory: {}", e))?;

    let token_cache_path = token_cache_dir.join(format!("token_{}.json", uuid::Uuid::new_v4()));
    log::info!("Token cache path: {:?}", token_cache_path);

    // Build the authenticator
    log::info!("Building authenticator...");
    // Use HTTPRedirect which automatically finds an available port
    let auth = InstalledFlowAuthenticator::builder(secret, InstalledFlowReturnMethod::HTTPRedirect)
        .persist_tokens_to_disk(&token_cache_path)
        .flow_delegate(Box::new(BrowserFlowDelegate))
        .build()
        .await
        .map_err(|e| {
            log::error!("Failed to build authenticator: {}", e);
            format!("Failed to build authenticator: {}", e)
        })?;

    log::info!("Authenticator built, requesting token...");

    // Get a token to trigger the auth flow
    let token = auth.token(SCOPES).await.map_err(|e| {
        log::error!("Failed to get token: {}", e);
        format!("Failed to get token: {}", e)
    })?;

    log::info!("Token received!");

    let access_token = token
        .token()
        .ok_or_else(|| "No access token received".to_string())?
        .to_string();
    log::info!("Access token extracted");

    // Read the cached token to get the refresh token
    let cached_token: serde_json::Value = if token_cache_path.exists() {
        log::info!("Reading token cache file...");
        let contents = fs::read_to_string(&token_cache_path)
            .map_err(|e| format!("Failed to read token cache: {}", e))?;
        serde_json::from_str(&contents)
            .map_err(|e| format!("Failed to parse token cache: {}", e))?
    } else {
        log::error!("Token cache not found at {:?}", token_cache_path);
        return Err("Token cache not found after auth".to_string());
    };

    // The token cache is an array with the token object nested inside
    let refresh_token = cached_token
        .get(0)
        .and_then(|v| v.get("token"))
        .and_then(|v| v.get("refresh_token"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            log::error!("No refresh token in cached token: {:?}", cached_token);
            "No refresh token in response".to_string()
        })?
        .to_string();
    log::info!("Refresh token extracted");

    // Clean up temp token file
    let _ = fs::remove_file(&token_cache_path);

    // Get user info to get the email
    log::info!("Fetching user info...");
    let user_info = fetch_user_info(&access_token).await.map_err(|e| {
        log::error!("Failed to fetch user info: {}", e);
        e
    })?;
    log::info!("User info received: {}", user_info.email);

    // Calculate expiry time (default 1 hour if not provided)
    let expires_at = Utc::now() + Duration::hours(1);

    // Check if account already exists
    let mut accounts = load_accounts_from_disk().map_err(|e| {
        log::error!("Failed to load accounts: {}", e);
        e
    })?;
    log::info!("Loaded {} existing accounts", accounts.len());

    if let Some(existing) = accounts.iter_mut().find(|a| a.email == user_info.email) {
        // Update existing account
        log::info!("Updating existing account: {}", user_info.email);
        existing.access_token = access_token;
        existing.refresh_token = refresh_token;
        existing.expires_at = expires_at;
        existing.display_name = user_info.display_name.clone();
        existing.photo_url = user_info.photo_url.clone();
    } else {
        // Add new account
        log::info!("Adding new account: {}", user_info.email);
        accounts.push(GoogleAccount {
            email: user_info.email.clone(),
            display_name: user_info.display_name.clone(),
            photo_url: user_info.photo_url.clone(),
            access_token,
            refresh_token,
            expires_at,
        });
    }

    save_accounts_to_disk(&accounts).map_err(|e| {
        log::error!("Failed to save accounts: {}", e);
        e
    })?;
    log::info!("Accounts saved to disk");

    // Update cache
    {
        let mut cache = ACCOUNTS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(accounts);
    }
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
pub fn remove_google_account(email: &str) -> Result<(), String> {
    let mut accounts = load_accounts_from_disk()?;

    let original_len = accounts.len();
    accounts.retain(|a| a.email != email);

    if accounts.len() == original_len {
        return Err(format!("Account not found: {}", email));
    }

    save_accounts_to_disk(&accounts)?;

    // Update cache
    {
        let mut cache = ACCOUNTS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(accounts);
    }

    Ok(())
}

/// Refresh an access token if needed
pub async fn ensure_valid_token(email: &str) -> Result<String, String> {
    // Check if this is a service account - use JWT-based auth
    if is_service_account_email(email) {
        log::info!("Using service account authentication for {}", email);
        return get_service_account_token().await;
    }

    // Load accounts once for the entire operation to avoid race conditions
    let mut accounts = load_accounts_from_disk()?;
    let account_index = accounts
        .iter()
        .position(|a| a.email == email)
        .ok_or_else(|| format!("Account not found: {}", email))?;

    // Check if token is still valid (with 5 minute buffer)
    if accounts[account_index].expires_at > Utc::now() + Duration::minutes(5) {
        return Ok(accounts[account_index].access_token.clone());
    }

    let oauth_credentials = load_oauth_credentials()?;

    // Clone the refresh token before the async operation
    let refresh_token = accounts[account_index].refresh_token.clone();
    let client = reqwest::Client::new();

    let response = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", oauth_credentials.client_id.as_str()),
            ("client_secret", oauth_credentials.client_secret.as_str()),
            ("refresh_token", &refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|e| format!("Failed to refresh token: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        log::error!(
            "Token refresh failed with status {}: {}",
            status,
            error_text
        );
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

    let expires_at = Utc::now() + Duration::seconds(token_response.expires_in);

    // Update account in the vector atomically
    let account = &mut accounts[account_index];
    account.access_token = token_response.access_token.clone();
    if let Some(rt) = token_response.refresh_token.as_deref() {
        account.refresh_token = rt.to_string();
    }
    account.expires_at = expires_at;

    // Save all accounts and update cache in one operation
    save_accounts_to_disk(&accounts)?;
    {
        let mut cache = ACCOUNTS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(accounts);
    }

    Ok(token_response.access_token)
}

#[cfg(test)]
mod tests {
    use super::{
        load_oauth_credentials_from_file, oauth_from_pair, OAuthCredentials, GOOGLE_CLIENT_ID_ENV,
        GOOGLE_CLIENT_SECRET_ENV,
    };
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
}
