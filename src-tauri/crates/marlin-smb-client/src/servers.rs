use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

const SMB_KEYRING_SERVICE: &str = "marlin-smb";

/// A saved SMB server, with no secrets. The frontend gets it, and it is the record in the servers file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SmbServerInfo {
    pub hostname: String,
    pub username: String,
    pub domain: Option<String>,
}

/// A saved server's sign-in, with the password from the keychain.
#[derive(Clone)]
pub struct SmbServerCredentials {
    pub username: String,
    pub password: String,
    pub domain: Option<String>,
}

// Written by hand: a derived `Debug` would print the password.
impl std::fmt::Debug for SmbServerCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmbServerCredentials")
            .field("username", &self.username)
            .field("password", &"<hidden>")
            .field("domain", &self.domain)
            .finish()
    }
}

/// Why a saved server's credentials could not be read. Each text is what the user reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialsError {
    /// No saved server has this host, or the keychain holds no password for it.
    NotSaved(String),
    /// The servers file or the keychain could not be read. A keychain failure keeps the
    /// `[SMB_NO_CREDENTIALS]` tag, so the app still asks the user for the password.
    Failed(String),
}

impl std::fmt::Display for CredentialsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CredentialsError::NotSaved(text) | CredentialsError::Failed(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for CredentialsError {}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ServerStorage {
    servers: Vec<SmbServerInfo>,
}

/// macOS can ask the user on every keychain read, so each process keeps each password it has
/// read or written for the rest of its run. The lock is held across the keychain call, so
/// callers that arrive together cause one read, and the map never holds a password that
/// this process has replaced or deleted. A change made by another process is not seen.
static PASSWORDS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn keyring_entry(hostname: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SMB_KEYRING_SERVICE, hostname)
        .map_err(|e| format!("Failed to create keyring entry: {}", e))
}

fn set_password(hostname: &str, password: &str) -> Result<(), String> {
    let mut passwords = PASSWORDS.lock().map_err(|e| e.to_string())?;
    // A failed write must not leave the previous password in memory: the retry
    // path deletes the keychain entry before it writes the new one.
    passwords.remove(hostname);

    let entry = keyring_entry(hostname)?;
    let result = match entry.set_password(password) {
        // An entry with another attribute set already exists. On macOS that can also come
        // back as a platform error. Only these delete and write again: deleting on any other
        // error could lose the password to a passing failure.
        Err(e)
            if matches!(e, keyring::Error::Ambiguous(_))
                || e.to_string().to_lowercase().contains("already exists")
                || e.to_string().to_lowercase().contains("duplicate") =>
        {
            let _ = entry.delete_credential();
            keyring_entry(hostname)?.set_password(password)
        }
        other => other,
    }
    .map_err(|e| format!("Failed to store password in keychain: {}", e));
    if result.is_ok() {
        passwords.insert(hostname.to_string(), password.to_string());
    }
    result
}

fn get_password(hostname: &str) -> Result<String, CredentialsError> {
    let mut passwords = PASSWORDS
        .lock()
        .map_err(|e| CredentialsError::Failed(e.to_string()))?;
    if let Some(password) = passwords.get(hostname) {
        return Ok(password.clone());
    }

    let entry = keyring_entry(hostname).map_err(CredentialsError::Failed)?;
    let password = entry.get_password().map_err(|e| {
        let text = format!(
            "[SMB_NO_CREDENTIALS] Failed to read password from keychain: {}",
            e
        );
        match e {
            keyring::Error::NoEntry => CredentialsError::NotSaved(text),
            // A locked or refused keychain: the password may well be there.
            _ => CredentialsError::Failed(text),
        }
    })?;
    passwords.insert(hostname.to_string(), password.clone());
    Ok(password)
}

fn delete_password(hostname: &str) -> Result<(), String> {
    let mut passwords = PASSWORDS.lock().map_err(|e| e.to_string())?;
    passwords.remove(hostname);

    let entry = keyring_entry(hostname)?;
    // Ignore missing entries; we still want server removal to succeed.
    match entry.delete_credential() {
        Ok(_) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Failed to delete password from keychain: {}", e)),
    }
}

/// The default servers file. Reading it creates nothing.
pub fn servers_path() -> Result<PathBuf, String> {
    let config_dir =
        dirs::config_dir().ok_or_else(|| "Could not determine config directory".to_string())?;
    Ok(config_dir.join("marlin").join("smb-servers.json"))
}

/// Saved servers, with no secrets. A missing file is an empty list, and a leftover
/// plaintext `password` field is ignored.
pub fn saved_servers(path: &Path) -> Result<Vec<SmbServerInfo>, String> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Failed to read servers file: {}", e)),
    };

    let storage: ServerStorage = serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse servers file: {}", e))?;

    Ok(storage.servers)
}

/// Move each plaintext `password` from the servers file into the keychain, and return the
/// saved servers. A file with no plaintext password is left as it is. The app calls this at
/// launch, before any window can read a server, and `add_server` and `remove_server` load
/// the servers through it: a save keeps only what it returns, so a failed migration must
/// stop the save. The helper never calls it: only the app writes the file.
pub fn migrate_plaintext_passwords(path: &Path) -> Result<Vec<SmbServerInfo>, String> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Failed to read servers file: {}", e)),
    };

    #[derive(Deserialize)]
    struct DiskServer {
        #[serde(flatten)]
        info: SmbServerInfo,
        #[serde(default)]
        password: Option<String>,
    }

    #[derive(Deserialize)]
    struct DiskStorage {
        servers: Vec<DiskServer>,
    }

    let storage: DiskStorage = serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse servers file: {}", e))?;

    let plaintext = storage
        .servers
        .iter()
        .any(|server| server.password.is_some());
    for server in &storage.servers {
        if let Some(password) = &server.password {
            set_password(&server.info.hostname, password)?;
        }
    }
    let servers: Vec<SmbServerInfo> = storage
        .servers
        .into_iter()
        .map(|server| server.info)
        .collect();
    if plaintext {
        save_servers(path, &servers)?;
    }
    Ok(servers)
}

fn save_servers(path: &Path, servers: &[SmbServerInfo]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("Failed to create config directory: {}", e))?;
    }

    let storage = ServerStorage {
        servers: servers.to_vec(),
    };

    let contents = serde_json::to_string_pretty(&storage)
        .map_err(|e| format!("Failed to serialize servers: {}", e))?;

    // A second process may read this file during a save. A plain write truncates it first,
    // so that reader could see an empty or half-written file. Rename replaces it only once
    // the new file is complete.
    let tmp_path = path.with_extension("json.tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    // The file names the user's servers, and a rename gives the new file's mode to it.
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
        .open(&tmp_path)
        .and_then(|mut file| file.write_all(contents.as_bytes()))
        .map_err(|e| format!("Failed to write servers file: {}", e))?;
    fs::rename(&tmp_path, path).map_err(|e| format!("Failed to write servers file: {}", e))?;

    Ok(())
}

/// Credentials for one saved server.
pub fn server_credentials(
    path: &Path,
    hostname: &str,
) -> Result<SmbServerCredentials, CredentialsError> {
    let servers = saved_servers(path).map_err(CredentialsError::Failed)?;
    let server = servers
        .into_iter()
        .find(|server| server.hostname.eq_ignore_ascii_case(hostname))
        .ok_or_else(|| {
            CredentialsError::NotSaved(format!(
                "[SMB_NO_CREDENTIALS] No credentials stored for server: {}",
                hostname
            ))
        })?;

    let password = get_password(&server.hostname)?;
    Ok(SmbServerCredentials {
        username: server.username,
        password,
        domain: server.domain,
    })
}

/// Add a new SMB server, or replace the one with this host. A host name matches in any letter case.
/// An existing host keeps its stored spelling for the keychain key.
pub fn add_server(
    path: &Path,
    hostname: String,
    username: String,
    password: String,
    domain: Option<String>,
) -> Result<SmbServerInfo, String> {
    let mut servers = migrate_plaintext_passwords(path)?;
    let mut keychain_hostname = hostname.clone();

    if let Some(existing) = servers
        .iter_mut()
        .find(|server| server.hostname.eq_ignore_ascii_case(&hostname))
    {
        keychain_hostname = existing.hostname.clone();
        existing.username = username.clone();
        existing.domain = domain.clone();
    } else {
        servers.push(SmbServerInfo {
            hostname: hostname.clone(),
            username: username.clone(),
            domain: domain.clone(),
        });
    }

    set_password(&keychain_hostname, &password)?;
    save_servers(path, &servers)?;

    Ok(SmbServerInfo {
        hostname,
        username,
        domain,
    })
}

/// Remove an SMB server and its keychain password.
pub fn remove_server(path: &Path, hostname: &str) -> Result<(), String> {
    let mut servers = migrate_plaintext_passwords(path)?;

    let Some(index) = servers
        .iter()
        .position(|server| server.hostname.eq_ignore_ascii_case(hostname))
    else {
        return Err(format!("Server not found: {}", hostname));
    };
    let removed = servers.remove(index);

    // The keychain entry is named by the stored host spelling, which can differ from `hostname`.
    delete_password(&removed.hostname)?;
    save_servers(path, &servers)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_password_keeps_a_keychain_read() {
        marlin_test_keychain::install();
        let host = "kept.example";
        marlin_test_keychain::set(SMB_KEYRING_SERVICE, host, "secret");

        assert_eq!(get_password(host).unwrap(), "secret");

        marlin_test_keychain::remove(SMB_KEYRING_SERVICE, host);
        assert_eq!(get_password(host).unwrap(), "secret");
    }

    #[test]
    fn get_password_retries_a_failed_read() {
        marlin_test_keychain::install();
        let host = "retried.example";

        let err = get_password(host).unwrap_err();
        assert!(
            matches!(err, CredentialsError::NotSaved(ref text) if text.contains("[SMB_NO_CREDENTIALS]")),
            "{err}"
        );

        marlin_test_keychain::set(SMB_KEYRING_SERVICE, host, "later");
        assert_eq!(get_password(host).unwrap(), "later");
    }

    #[test]
    fn set_password_replaces_what_was_kept() {
        marlin_test_keychain::install();
        let host = "replaced.example";
        marlin_test_keychain::set(SMB_KEYRING_SERVICE, host, "old");

        assert_eq!(get_password(host).unwrap(), "old");
        set_password(host, "new").unwrap();

        assert_eq!(get_password(host).unwrap(), "new");
        assert_eq!(
            marlin_test_keychain::get(SMB_KEYRING_SERVICE, host).as_deref(),
            Some("new")
        );
    }

    #[test]
    fn delete_password_forgets() {
        marlin_test_keychain::install();
        let host = "forgotten.example";

        set_password(host, "gone").unwrap();
        delete_password(host).unwrap();

        let err = get_password(host).unwrap_err();
        assert!(err.to_string().contains("[SMB_NO_CREDENTIALS]"), "{err}");
        assert!(marlin_test_keychain::get(SMB_KEYRING_SERVICE, host).is_none());
    }

    #[test]
    fn a_server_written_after_a_read_is_seen_by_the_next_read() {
        marlin_test_keychain::install();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("smb-servers.json");
        assert!(saved_servers(&path).unwrap().is_empty());
        assert!(!path.exists());

        fs::write(
            &path,
            r#"{"servers":[{"hostname":"Reread.example","username":"other","domain":"CORP"}]}"#,
        )
        .unwrap();
        marlin_test_keychain::set(SMB_KEYRING_SERVICE, "Reread.example", "secret");

        let servers = saved_servers(&path).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].hostname, "Reread.example");
        let creds = server_credentials(&path, "reread.example").unwrap();
        assert_eq!(creds.username, "other");
        assert_eq!(creds.password, "secret");
        assert_eq!(creds.domain.as_deref(), Some("CORP"));
    }

    #[test]
    fn reading_does_not_rewrite_a_file_with_a_plaintext_password() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("smb-servers.json");
        let original = r#"{"servers":[{"hostname":"plain.example","username":"user","password":"secret","domain":null}]}"#;
        fs::write(&path, original).unwrap();

        let servers = saved_servers(&path).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].hostname, "plain.example");
        assert_eq!(servers[0].username, "user");
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn migrate_moves_a_plaintext_password_to_the_keychain() {
        marlin_test_keychain::install();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("smb-servers.json");
        fs::write(
            &path,
            r#"{"servers":[{"hostname":"plaintext.example","username":"user","password":"secret","domain":null}]}"#,
        )
        .unwrap();

        migrate_plaintext_passwords(&path).unwrap();

        let after = fs::read_to_string(&path).unwrap();
        assert!(!after.contains("password"), "{after}");
        assert!(!after.contains("secret"), "{after}");
        assert_eq!(
            marlin_test_keychain::get(SMB_KEYRING_SERVICE, "plaintext.example").as_deref(),
            Some("secret")
        );
        let creds = server_credentials(&path, "plaintext.example").unwrap();
        assert_eq!(creds.password, "secret");
        assert_eq!(creds.username, "user");

        let settled = fs::read_to_string(&path).unwrap();
        migrate_plaintext_passwords(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), settled);
    }

    #[test]
    fn a_missing_server_and_a_missing_password_are_not_saved() {
        marlin_test_keychain::install();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("smb-servers.json");

        assert_eq!(
            server_credentials(&path, "missing.example").unwrap_err(),
            CredentialsError::NotSaved(
                "[SMB_NO_CREDENTIALS] No credentials stored for server: missing.example"
                    .to_string()
            )
        );

        fs::write(
            &path,
            r#"{"servers":[{"hostname":"nopassword.example","username":"user","domain":null}]}"#,
        )
        .unwrap();
        match server_credentials(&path, "nopassword.example") {
            Err(CredentialsError::NotSaved(text)) => assert!(
                text.starts_with("[SMB_NO_CREDENTIALS] Failed to read password from keychain: "),
                "{text}"
            ),
            other => panic!("expected a missing password, got {other:?}"),
        }
    }

    #[test]
    fn remove_server_deletes_the_record_and_the_keychain_password() {
        marlin_test_keychain::install();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("smb-servers.json");
        add_server(
            &path,
            "Remove.example".to_string(),
            "user".to_string(),
            "secret".to_string(),
            None,
        )
        .unwrap();

        remove_server(&path, "remove.example").unwrap();
        assert!(saved_servers(&path).unwrap().is_empty());
        assert_eq!(
            marlin_test_keychain::get(SMB_KEYRING_SERVICE, "Remove.example"),
            None
        );
        assert_eq!(
            remove_server(&path, "remove.example").unwrap_err(),
            "Server not found: remove.example"
        );
    }

    #[test]
    fn re_adding_a_server_keeps_the_stored_host_spelling_for_the_keychain() {
        marlin_test_keychain::install();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("smb-servers.json");

        add_server(
            &path,
            "NAS.readd".to_string(),
            "ana".to_string(),
            "old".to_string(),
            None,
        )
        .unwrap();
        let saved = add_server(
            &path,
            "nas.readd".to_string(),
            "bo".to_string(),
            "new".to_string(),
            Some("CORP".to_string()),
        )
        .unwrap();

        assert_eq!(saved.hostname, "nas.readd");
        let listed = saved_servers(&path).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].hostname, "NAS.readd");
        assert_eq!(listed[0].username, "bo");
        assert_eq!(
            server_credentials(&path, "nas.readd")
                .unwrap()
                .password
                .as_str(),
            "new"
        );
        assert_eq!(
            marlin_test_keychain::get(SMB_KEYRING_SERVICE, "NAS.readd").as_deref(),
            Some("new")
        );
        assert_eq!(
            marlin_test_keychain::get(SMB_KEYRING_SERVICE, "nas.readd"),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_servers_file_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        marlin_test_keychain::install();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("marlin").join("smb-servers.json");
        add_server(
            &path,
            "private.example".to_string(),
            "user".to_string(),
            "secret".to_string(),
            None,
        )
        .unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
