use chrono::{DateTime, Duration, Utc};
use marlin_gdrive::{keyring_entry, OAuthClient, SavedSignIn, SignInError};
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

/// Scopes needed for Google Drive access
const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/drive",
    "https://www.googleapis.com/auth/userinfo.email",
];

/// Environment variable for service account key file path
const SERVICE_ACCOUNT_KEY_FILE_ENV: &str = "GOOGLE_SERVICE_ACCOUNT_KEY_FILE";

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

/// Information about a connected Google account
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleAccountInfo {
    pub email: String,
    pub display_name: Option<String>,
    pub photo_url: Option<String>,
    /// The folder name the account mounts at. `None` only for the service account, which is not
    /// stored in the accounts file and which the mount helper cannot sign in as.
    pub mount_name: Option<String>,
}

/// Storage structure for accounts file
#[derive(Serialize)]
struct AccountStorage {
    accounts: Vec<GoogleAccountInfo>,
}

// Two locks own all Google account state. `TOKENS` covers the keychain entries and the
// tokens in memory. `ACCOUNTS_CACHE` covers the accounts file and its cached copy.
// Take `TOKENS` first, then `ACCOUNTS_CACHE`, and never hold `ACCOUNTS_CACHE` across an `.await`.

/// Held across `valid_token`, so a sign-in or a removal never runs during a refresh.
/// `SavedSignIn` itself makes calls that arrive together cause one keychain read and one request.
static TOKENS: Lazy<Mutex<HashMap<String, SavedSignIn>>> = Lazy::new(|| Mutex::new(HashMap::new()));

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

/// Reads the account list. A file written by an older version holds tokens and no mount names: this moves
/// each refresh token into the keychain, builds each missing mount name, and rewrites the file.
fn read_and_upgrade_accounts_file(path: &Path) -> Result<Vec<GoogleAccountInfo>, String> {
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
        mount_name: Option<String>,
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

    let mut needs_rewrite = false;
    let mut mount_names: Vec<String> = storage
        .accounts
        .iter()
        .filter_map(|account| account.mount_name.clone())
        .collect();
    let mut accounts = Vec::with_capacity(storage.accounts.len());
    for account in storage.accounts {
        if account.access_token.is_some() || account.refresh_token.is_some() {
            needs_rewrite = true;
        }
        if let Some(refresh_token) = account.refresh_token {
            set_refresh_token(&account.email, &refresh_token)?;
        }
        let mount_name = match account.mount_name {
            Some(name) => name,
            None => {
                let name =
                    marlin_mount_client::mount_name::build("gdrive", &account.email, &mount_names);
                mount_names.push(name.clone());
                needs_rewrite = true;
                name
            }
        };
        accounts.push(GoogleAccountInfo {
            email: account.email,
            display_name: account.display_name,
            photo_url: account.photo_url,
            mount_name: Some(mount_name),
        });
    }

    // Every keychain write has succeeded by now, so a failed one leaves the file as it was.
    if needs_rewrite {
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
            mount_name: None,
        });
    }

    let mut cache = ACCOUNTS_CACHE.lock().map_err(|e| e.to_string())?;
    if cache.is_none() {
        *cache = Some(read_and_upgrade_accounts_file(&get_accounts_path()?)?);

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

/// Opens the Google sign-in page. `login_hint` is the email the user is expected to pick.
struct BrowserFlowDelegate {
    login_hint: Option<String>,
}

impl InstalledFlowDelegate for BrowserFlowDelegate {
    fn present_user_url<'a>(
        &'a self,
        url: &'a str,
        _need_code: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async move {
            // Google only returns a refresh token on the first consent unless this is set.
            let mut open_url = format!("{url}&prompt=consent");
            if let Some(hint) = &self.login_hint {
                open_url.push_str("&login_hint=");
                open_url.push_str(&urlencoding::encode(hint));
            }

            if let Err(e) = open::that(&open_url) {
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

/// What Google returns for the signed-in user.
#[derive(Deserialize)]
struct GoogleUserInfo {
    email: String,
    name: Option<String>,
    picture: Option<String>,
}

/// Puts a signed-in account in the list. An account that is already listed keeps its mount name: signing in again never moves its mount.
fn add_or_update_account(
    accounts: &mut Vec<GoogleAccountInfo>,
    user: GoogleUserInfo,
) -> GoogleAccountInfo {
    if let Some(existing) = accounts
        .iter_mut()
        .find(|account| account.email == user.email)
    {
        log::info!("Updating existing account: {}", user.email);
        existing.display_name = user.name;
        existing.photo_url = user.picture;
        return existing.clone();
    }

    log::info!("Adding new account: {}", user.email);
    let names: Vec<String> = accounts
        .iter()
        .filter_map(|account| account.mount_name.clone())
        .collect();
    let mount_name = marlin_mount_client::mount_name::build("gdrive", &user.email, &names);
    let account = GoogleAccountInfo {
        email: user.email,
        display_name: user.name,
        photo_url: user.picture,
        mount_name: Some(mount_name),
    };
    accounts.push(account.clone());
    account
}

/// Signs in to a Google account in the browser. Adds the account, or updates it when it is already listed.
/// `login_hint` is the email the user is expected to pick.
pub async fn add_google_account(login_hint: Option<String>) -> Result<GoogleAccountInfo, String> {
    let client = OAuthClient::load()?;

    // One sign-in at a time. A second one fails at once, so it never waits on a browser tab the user closed.
    let _guard = AUTH_MUTEX.try_lock().map_err(|_| {
        "A Google sign-in is already open in your browser. Finish it there, or try again in a few minutes."
            .to_string()
    })?;

    log::info!("Starting Google OAuth flow...");

    let secret = ApplicationSecret {
        client_id: client.client_id.clone(),
        client_secret: client.client_secret.clone(),
        auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
        token_uri: client.token_uri.clone(),
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
        .flow_delegate(Box::new(BrowserFlowDelegate { login_hint }))
        .build()
        .await
        .map_err(|e| {
            log::error!("Failed to build authenticator: {}", e);
            format!("Failed to build authenticator: {}", e)
        })?;

    log::info!("Authenticator built, requesting token...");

    // Get a token to trigger the auth flow. A user who closes the browser tab never answers,
    // and everything that waits on this sign-in would wait with them.
    tokio::time::timeout(std::time::Duration::from_secs(300), auth.token(SCOPES))
        .await
        .map_err(|_| "Google sign-in was not finished within 5 minutes. Try again.".to_string())?
        .map_err(|e| {
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
    let user = fetch_user_info(&access_token).await.map_err(|e| {
        log::error!("Failed to fetch user info: {}", e);
        e
    })?;
    log::info!("User info received: {}", user.email);

    let mut tokens = TOKENS.lock().await;
    let mut cache = ACCOUNTS_CACHE.lock().map_err(|e| e.to_string())?;

    let path = get_accounts_path()?;
    let mut accounts = read_and_upgrade_accounts_file(&path).map_err(|e| {
        log::error!("Failed to load accounts: {}", e);
        e
    })?;
    log::info!("Loaded {} existing accounts", accounts.len());

    set_refresh_token(&user.email, &refresh_token)?;

    let account = add_or_update_account(&mut accounts, user);

    write_accounts_file(&path, &accounts).map_err(|e| {
        log::error!("Failed to save accounts: {}", e);
        e
    })?;
    log::info!("Accounts saved to disk");

    *cache = Some(accounts);
    tokens.insert(
        account.email.clone(),
        SavedSignIn::signed_in(
            &account.email,
            client,
            refresh_token,
            access_token,
            expires_at.into(),
        ),
    );
    log::info!("Account cache updated, returning user info");

    Ok(account)
}

/// Fetch user info from Google
async fn fetch_user_info(access_token: &str) -> Result<GoogleUserInfo, String> {
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

    response
        .json()
        .await
        .map_err(|e| format!("Failed to parse user info: {}", e))
}

/// Remove a Google account
pub async fn remove_google_account(email: &str) -> Result<(), String> {
    let mut tokens = TOKENS.lock().await;
    let mut cache = ACCOUNTS_CACHE.lock().map_err(|e| e.to_string())?;
    let path = get_accounts_path()?;
    let mut accounts = read_and_upgrade_accounts_file(&path)?;

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

/// Starts the error for an account whose saved sign-in no longer works. The frontend offers to sign in again.
const SIGN_IN_NEEDED: &str = "[GDRIVE_SIGN_IN_NEEDED]";

/// Refresh an access token if needed
pub async fn ensure_valid_token(email: &str) -> Result<String, String> {
    // Check if this is a service account - use JWT-based auth
    if is_service_account_email(email) {
        log::info!("Using service account authentication for {}", email);
        return get_service_account_token().await;
    }

    let mut tokens = TOKENS.lock().await;
    if !tokens.contains_key(email) {
        // Listing the accounts moves an older file's tokens into the keychain first.
        let accounts = get_google_accounts()?;
        if !accounts.iter().any(|account| account.email == email) {
            return Err(format!("Account not found: {}", email));
        }
        tokens.insert(
            email.to_string(),
            SavedSignIn::new(email, OAuthClient::load()?),
        );
    }

    tokens[email].valid_token().await.map_err(|error| match error {
        SignInError::NoSavedSignIn => format!(
            "{} No Google sign-in is stored for {}. Open a folder in this account to sign in again.",
            SIGN_IN_NEEDED, email
        ),
        SignInError::Rejected => format!(
            "{} Google no longer accepts the saved sign-in for {}. Open a folder in this account to sign in again.",
            SIGN_IN_NEEDED, email
        ),
        SignInError::Offline(error) => format!("Failed to refresh token: {}", error),
        SignInError::Other(error) => {
            log::error!("{}", error);
            error
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        add_or_update_account, delete_refresh_token, read_and_upgrade_accounts_file,
        write_accounts_file, GoogleAccountInfo, GoogleUserInfo,
    };
    use marlin_gdrive::KEYRING_SERVICE;
    use marlin_test_keychain as test_keychain;
    use std::fs;

    #[test]
    fn read_and_upgrade_accounts_file_moves_tokens_to_the_keychain() {
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

        let accounts = read_and_upgrade_accounts_file(&path).unwrap();
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
            test_keychain::get(KEYRING_SERVICE, ada).as_deref(),
            Some(refresh_ada)
        );
        assert_eq!(
            test_keychain::get(KEYRING_SERVICE, bea).as_deref(),
            Some(refresh_bea)
        );

        let rewritten = fs::read_to_string(&path).unwrap();
        for secret in [access_ada, refresh_ada, access_bea, refresh_bea] {
            assert!(!rewritten.contains(secret), "{rewritten}");
        }
        assert!(!rewritten.contains("accessToken"));
        assert!(!rewritten.contains("refreshToken"));

        let again = read_and_upgrade_accounts_file(&path).unwrap();
        assert_eq!(again.len(), accounts.len());
        for (left, right) in again.iter().zip(&accounts) {
            assert_eq!(left.email, right.email);
            assert_eq!(left.display_name, right.display_name);
            assert_eq!(left.photo_url, right.photo_url);
            assert_eq!(left.mount_name, right.mount_name);
        }
    }

    #[test]
    fn read_and_upgrade_accounts_file_keeps_the_file_when_the_keychain_fails() {
        test_keychain::install();
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("gdrive-accounts.json");
        let email = "keychain-fails@example.com";
        let original = format!(
            r#"{{"accounts":[{{"email":"{email}","displayName":"Fail","photoUrl":null,"accessToken":"access-fail","refreshToken":"refresh-fail","expiresAt":"2020-01-01T00:00:00Z"}}]}}"#
        );
        fs::write(&path, &original).unwrap();
        test_keychain::fail_writes_for(KEYRING_SERVICE, email);

        let err = read_and_upgrade_accounts_file(&path).unwrap_err();
        assert!(
            err.contains("Failed to store Google sign-in in keychain"),
            "{err}"
        );
        assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
    }

    #[test]
    fn read_and_upgrade_accounts_file_rewrites_a_file_with_only_an_access_token() {
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

        let accounts = read_and_upgrade_accounts_file(&path).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].email, email);
        assert_eq!(accounts[0].display_name.as_deref(), Some("Access"));
        let rewritten = fs::read_to_string(&path).unwrap();
        assert!(!rewritten.contains(access_token), "{rewritten}");
        assert!(test_keychain::get(KEYRING_SERVICE, email).is_none());
    }

    #[test]
    fn read_and_upgrade_accounts_file_leaves_a_file_without_tokens_and_with_mount_names_alone() {
        test_keychain::install();
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("gdrive-accounts.json");
        let original = "{\"accounts\":[ {\"email\":\"plain-spacing@example.com\", \"displayName\":\"Pat\", \"photoUrl\":null, \"mountName\":\"gdrive-plain-spacing@example.com\"} ]}";
        fs::write(&path, original).unwrap();

        let accounts = read_and_upgrade_accounts_file(&path).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].email, "plain-spacing@example.com");
        assert_eq!(accounts[0].display_name.as_deref(), Some("Pat"));
        assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
    }

    #[test]
    fn delete_refresh_token_ignores_a_missing_entry() {
        test_keychain::install();
        let email = "delete-twice@example.com";
        test_keychain::set(KEYRING_SERVICE, email, "secret");

        delete_refresh_token(email).unwrap();
        assert!(test_keychain::get(KEYRING_SERVICE, email).is_none());
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
                mount_name: Some("gdrive-write-replace@example.com".to_string()),
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

    #[test]
    fn read_and_upgrade_accounts_file_builds_mount_names_for_an_older_file() {
        test_keychain::install();
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("gdrive-accounts.json");
        let ada = "older-ada@example.com";
        let bea = "older-bea@example.com";
        fs::write(
            &path,
            format!(
                r#"{{"accounts":[
                    {{"email":"{ada}","displayName":"Ada","photoUrl":"https://example.com/ada.png"}},
                    {{"email":"{bea}","displayName":"Bea","photoUrl":"https://example.com/bea.png"}}
                ]}}"#
            ),
        )
        .unwrap();

        let accounts = read_and_upgrade_accounts_file(&path).unwrap();
        assert_eq!(accounts[0].email, ada);
        assert_eq!(accounts[0].display_name.as_deref(), Some("Ada"));
        assert_eq!(
            accounts[0].photo_url.as_deref(),
            Some("https://example.com/ada.png")
        );
        assert_eq!(
            accounts[0].mount_name.as_deref(),
            Some("gdrive-older-ada@example.com")
        );
        assert_eq!(accounts[1].email, bea);
        assert_eq!(accounts[1].display_name.as_deref(), Some("Bea"));
        assert_eq!(
            accounts[1].photo_url.as_deref(),
            Some("https://example.com/bea.png")
        );
        assert_eq!(
            accounts[1].mount_name.as_deref(),
            Some("gdrive-older-bea@example.com")
        );

        let rewritten = fs::read(&path).unwrap();
        let text = String::from_utf8(rewritten.clone()).unwrap();
        assert!(text.contains("gdrive-older-ada@example.com"), "{text}");
        assert!(text.contains("gdrive-older-bea@example.com"), "{text}");

        let again = read_and_upgrade_accounts_file(&path).unwrap();
        assert_eq!(again[0].mount_name, accounts[0].mount_name);
        assert_eq!(again[1].mount_name, accounts[1].mount_name);
        assert_eq!(fs::read(&path).unwrap(), rewritten);
    }

    #[test]
    fn a_mount_name_keeps_its_suffix_after_the_other_account_is_removed() {
        test_keychain::install();
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("gdrive-accounts.json");
        let first = "Case-Clash@example.com";
        let second = "case-clash@example.com";
        fs::write(
            &path,
            format!(
                r#"{{"accounts":[
                    {{"email":"{first}","displayName":"First","photoUrl":null}},
                    {{"email":"{second}","displayName":"Second","photoUrl":null}}
                ]}}"#
            ),
        )
        .unwrap();

        let mut accounts = read_and_upgrade_accounts_file(&path).unwrap();
        assert_eq!(
            accounts[0].mount_name.as_deref(),
            Some("gdrive-Case-Clash@example.com")
        );
        assert_eq!(
            accounts[1].mount_name.as_deref(),
            Some("gdrive-case-clash@example.com-2")
        );

        accounts.retain(|account| account.email != first);
        write_accounts_file(&path, &accounts).unwrap();
        let again = read_and_upgrade_accounts_file(&path).unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(
            again[0].mount_name.as_deref(),
            Some("gdrive-case-clash@example.com-2")
        );
    }

    #[test]
    fn a_stored_mount_name_wins_when_an_earlier_account_would_build_it() {
        test_keychain::install();
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("gdrive-accounts.json");
        let first = "Stored-Wins@example.com";
        let second = "stored-wins-other@example.com";
        let stored = "gdrive-stored-wins@example.com";
        fs::write(
            &path,
            format!(
                r#"{{"accounts":[
                    {{"email":"{first}","displayName":"First","photoUrl":null}},
                    {{"email":"{second}","displayName":"Second","photoUrl":null,"mountName":"{stored}"}}
                ]}}"#
            ),
        )
        .unwrap();

        let accounts = read_and_upgrade_accounts_file(&path).unwrap();
        assert_eq!(
            accounts[0].mount_name.as_deref(),
            Some("gdrive-Stored-Wins@example.com-2")
        );
        assert_eq!(accounts[1].mount_name.as_deref(), Some(stored));
    }

    #[test]
    fn add_or_update_account_keeps_a_mount_name_and_numbers_a_clash() {
        let mut accounts = Vec::new();
        let email = "add-or-update@example.com";
        let added = add_or_update_account(
            &mut accounts,
            GoogleUserInfo {
                email: email.to_string(),
                name: Some("Old".to_string()),
                picture: Some("https://example.com/old.png".to_string()),
            },
        );
        assert_eq!(
            added.mount_name.as_deref(),
            Some("gdrive-add-or-update@example.com")
        );
        assert_eq!(accounts.len(), 1);

        let updated = add_or_update_account(
            &mut accounts,
            GoogleUserInfo {
                email: email.to_string(),
                name: Some("New".to_string()),
                picture: None,
            },
        );
        assert_eq!(accounts.len(), 1);
        assert_eq!(updated.display_name.as_deref(), Some("New"));
        assert_eq!(updated.photo_url, None);
        assert_eq!(updated.mount_name, added.mount_name);

        let clash = add_or_update_account(
            &mut accounts,
            GoogleUserInfo {
                email: "Add-Or-Update@example.com".to_string(),
                name: None,
                picture: None,
            },
        );
        assert_eq!(
            clash.mount_name.as_deref(),
            Some("gdrive-Add-Or-Update@example.com-2")
        );
        assert_eq!(accounts.len(), 2);
    }
}
