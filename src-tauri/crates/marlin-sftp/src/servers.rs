use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

const KEYRING_SERVICE: &str = "marlin-sftp";

/// A saved SFTP server, with no secrets. The frontend gets it, and it is the record in the servers file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpServerInfo {
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub auth_method: String, // "password", "key", "agent"
    pub key_path: Option<String>,
}

/// A saved server's sign-in, with the password or key passphrase from the keychain.
#[derive(Clone)]
pub struct SftpServerCredentials {
    pub username: String,
    pub auth_method: String,
    pub password: Option<String>,
    pub key_path: Option<String>,
}

// Written by hand: a derived `Debug` would print the password.
impl std::fmt::Debug for SftpServerCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SftpServerCredentials")
            .field("username", &self.username)
            .field("auth_method", &self.auth_method)
            .field("password", &self.password.as_ref().map(|_| "<hidden>"))
            .field("key_path", &self.key_path)
            .finish()
    }
}

/// Why a saved server's credentials could not be read. Each text is what the user reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialsError {
    /// No saved server has this host and port, or the keychain holds no password for a password server. The user must add the server again.
    NotSaved(String),
    /// The servers file or the keychain could not be read. The keychain text starts with the tag that makes the app ask for the secret again.
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
    servers: Vec<SftpServerInfo>,
}

fn keyring_user(hostname: &str, port: u16, username: &str) -> String {
    format!("{}@{}:{}", username, hostname, port)
}

fn keyring_entry(hostname: &str, port: u16, username: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, &keyring_user(hostname, port, username))
        .map_err(|e| format!("Failed to create keyring entry: {}", e))
}

fn set_password(hostname: &str, port: u16, username: &str, password: &str) -> Result<(), String> {
    let entry = keyring_entry(hostname, port, username)?;
    match entry.set_password(password) {
        Ok(()) => return Ok(()),
        Err(keyring::Error::Ambiguous(_)) => {}
        Err(e) => {
            let msg = e.to_string().to_lowercase();
            if !msg.contains("already exists") && !msg.contains("duplicate") {
                return Err(format!("Failed to store password in keychain: {}", e));
            }
        }
    }
    // An entry that is already there, once or twice, is replaced.
    let _ = entry.delete_credential();
    keyring_entry(hostname, port, username)?
        .set_password(password)
        .map_err(|e| format!("Failed to store password in keychain: {}", e))
}

fn get_password(hostname: &str, port: u16, username: &str) -> Result<String, CredentialsError> {
    let entry = keyring_entry(hostname, port, username).map_err(CredentialsError::Failed)?;
    entry.get_password().map_err(|e| {
        let text = format!(
            "[SFTP_NO_CREDENTIALS] Failed to read password from keychain: {}",
            e
        );
        match e {
            keyring::Error::NoEntry => CredentialsError::NotSaved(text),
            // A locked or refused keychain: the password may well be there.
            _ => CredentialsError::Failed(text),
        }
    })
}

fn delete_password(hostname: &str, port: u16, username: &str) -> Result<(), String> {
    let entry = keyring_entry(hostname, port, username)?;
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
    Ok(config_dir.join("marlin").join("sftp-servers.json"))
}

/// Where the saved server for this host and port is in `list`. A host name matches in any letter case.
fn position(list: &[SftpServerInfo], hostname: &str, port: u16) -> Option<usize> {
    list.iter()
        .position(|s| s.hostname.eq_ignore_ascii_case(hostname) && s.port == port)
}

/// Saved servers, with no secrets. A missing file is an empty list.
pub fn saved_servers(path: &Path) -> Result<Vec<SftpServerInfo>, String> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Failed to read servers file: {}", e)),
    };

    let storage: ServerStorage = serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse servers file: {}", e))?;

    Ok(storage.servers)
}

fn save_servers(path: &Path, servers: &[SftpServerInfo]) -> Result<(), String> {
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
    // The file names the user's servers and key paths, and a rename gives the new file's mode to it.
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
    servers: &Path,
    hostname: &str,
    port: u16,
) -> Result<SftpServerCredentials, CredentialsError> {
    let mut list = saved_servers(servers).map_err(CredentialsError::Failed)?;

    let server = position(&list, hostname, port)
        .map(|index| list.swap_remove(index))
        .ok_or_else(|| {
            CredentialsError::NotSaved(format!(
                "[SFTP_NO_CREDENTIALS] No credentials stored for server: {}:{}",
                hostname, port
            ))
        })?;

    let password = if server.auth_method == "password" {
        Some(get_password(
            &server.hostname,
            server.port,
            &server.username,
        )?)
    } else if server.auth_method == "key" {
        // A key with no passphrase has no keychain entry, and a machine with no keychain service
        // (a Linux session without one) has none to read.
        get_password(&server.hostname, server.port, &server.username).ok()
    } else {
        None
    };

    Ok(SftpServerCredentials {
        username: server.username,
        auth_method: server.auth_method,
        password,
        key_path: server.key_path,
    })
}

/// Add a new SFTP server, or replace the one with this host and port.
pub fn add_server(
    servers: &Path,
    hostname: String,
    port: u16,
    username: String,
    password: Option<String>,
    auth_method: String,
    key_path: Option<String>,
) -> Result<SftpServerInfo, String> {
    let mut list = saved_servers(servers)?;
    let server = SftpServerInfo {
        hostname,
        port,
        username,
        auth_method,
        key_path,
    };

    match position(&list, &server.hostname, port) {
        Some(index) => {
            let old = std::mem::replace(&mut list[index], server.clone());
            // The keychain entry is named by the host as typed and the user, so a new spelling
            // or user leaves the old secret where nothing reads it.
            if old.hostname != server.hostname || old.username != server.username {
                let _ = delete_password(&old.hostname, port, &old.username);
            }
        }
        None => list.push(server.clone()),
    }

    // A server with no secret keeps no keychain entry, so an old password is never read as a key passphrase.
    match password.as_deref().filter(|pw| !pw.is_empty()) {
        Some(pw) => set_password(&server.hostname, port, &server.username, pw)?,
        None => {
            let _ = delete_password(&server.hostname, port, &server.username);
        }
    }

    save_servers(servers, &list)?;

    Ok(server)
}

/// Remove an SFTP server and its keychain password.
pub fn remove_server(servers: &Path, hostname: &str, port: u16) -> Result<(), String> {
    let mut list = saved_servers(servers)?;

    let Some(index) = position(&list, hostname, port) else {
        return Err(format!("Server not found: {}:{}", hostname, port));
    };
    let removed = list.remove(index);

    // The keychain entry is named by the stored host spelling, which can differ from `hostname`.
    let _ = delete_password(&removed.hostname, port, &removed.username);
    save_servers(servers, &list)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn servers_file(dir: &std::path::Path) -> PathBuf {
        dir.join("marlin").join("sftp-servers.json")
    }

    #[test]
    fn adding_a_password_server_creates_the_file_and_reads_the_password_back() {
        marlin_test_keychain::install();
        let dir = tempfile::tempdir().unwrap();
        let path = servers_file(dir.path());
        assert!(!path.parent().unwrap().exists());

        let saved = add_server(
            &path,
            "password.example".to_string(),
            22,
            "user".to_string(),
            Some("secret".to_string()),
            "password".to_string(),
            None,
        )
        .unwrap();

        assert!(path.parent().unwrap().is_dir());
        assert!(path.is_file());
        let listed = saved_servers(&path).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].hostname, saved.hostname);
        assert_eq!(listed[0].port, saved.port);
        assert_eq!(listed[0].username, saved.username);
        assert_eq!(listed[0].auth_method, saved.auth_method);
        assert_eq!(listed[0].key_path, saved.key_path);
        let creds = server_credentials(&path, "Password.Example", 22).unwrap();
        assert_eq!(creds.password.as_deref(), Some("secret"));
        assert_eq!(creds.username, "user");
        assert_eq!(creds.auth_method, "password");
    }

    #[test]
    fn a_server_written_after_a_read_is_seen_by_the_next_read() {
        marlin_test_keychain::install();
        let dir = tempfile::tempdir().unwrap();
        let path = servers_file(dir.path());
        assert!(saved_servers(&path).unwrap().is_empty());

        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"servers":[{"hostname":"reread.example","port":22,"username":"other","authMethod":"agent","keyPath":null}]}"#,
        )
        .unwrap();

        let servers = saved_servers(&path).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].hostname, "reread.example");
        let creds = server_credentials(&path, "reread.example", 22).unwrap();
        assert_eq!(creds.username, "other");
        assert_eq!(creds.auth_method, "agent");
        assert_eq!(creds.password, None);
    }

    #[test]
    fn remove_server_deletes_the_record_and_the_keychain_password() {
        marlin_test_keychain::install();
        let dir = tempfile::tempdir().unwrap();
        let path = servers_file(dir.path());
        add_server(
            &path,
            "remove.example".to_string(),
            22,
            "user".to_string(),
            Some("secret".to_string()),
            "password".to_string(),
            None,
        )
        .unwrap();

        remove_server(&path, "remove.example", 22).unwrap();
        assert!(saved_servers(&path).unwrap().is_empty());
        assert_eq!(
            marlin_test_keychain::get(KEYRING_SERVICE, "user@remove.example:22"),
            None
        );
        assert_eq!(
            remove_server(&path, "remove.example", 22).unwrap_err(),
            "Server not found: remove.example:22"
        );
    }

    #[test]
    fn re_adding_a_server_moves_its_password_to_the_new_host_spelling_and_user() {
        marlin_test_keychain::install();
        let dir = tempfile::tempdir().unwrap();
        let path = servers_file(dir.path());
        let add = |host: &str, user: &str, password: &str| {
            add_server(
                &path,
                host.to_string(),
                22,
                user.to_string(),
                Some(password.to_string()),
                "password".to_string(),
                None,
            )
            .unwrap()
        };

        add("NAS.readd", "ana", "old");
        add("nas.readd", "ana", "new");
        assert_eq!(saved_servers(&path).unwrap().len(), 1);
        assert_eq!(
            server_credentials(&path, "NAS.readd", 22)
                .unwrap()
                .password
                .as_deref(),
            Some("new")
        );
        assert_eq!(
            marlin_test_keychain::get(KEYRING_SERVICE, "ana@NAS.readd:22"),
            None
        );

        add("nas.readd", "bo", "other");
        assert_eq!(
            marlin_test_keychain::get(KEYRING_SERVICE, "ana@nas.readd:22"),
            None
        );

        remove_server(&path, "NAS.READD", 22).unwrap();
        assert_eq!(
            marlin_test_keychain::get(KEYRING_SERVICE, "bo@nas.readd:22"),
            None
        );
    }

    #[test]
    fn a_password_server_with_no_keychain_entry_is_not_saved() {
        marlin_test_keychain::install();
        let dir = tempfile::tempdir().unwrap();
        let path = servers_file(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"servers":[{"hostname":"nopassword.example","port":22,"username":"user","authMethod":"password","keyPath":null}]}"#,
        )
        .unwrap();

        match server_credentials(&path, "nopassword.example", 22) {
            Err(CredentialsError::NotSaved(text)) => assert!(
                text.starts_with("[SFTP_NO_CREDENTIALS] Failed to read password from keychain: "),
                "{text}"
            ),
            other => panic!("expected a missing password, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_servers_file_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        marlin_test_keychain::install();
        let dir = tempfile::tempdir().unwrap();
        let path = servers_file(dir.path());
        add_server(
            &path,
            "private.example".to_string(),
            22,
            "user".to_string(),
            None,
            "agent".to_string(),
            None,
        )
        .unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn saved_servers_on_a_missing_folder_returns_an_empty_list_and_creates_nothing() {
        marlin_test_keychain::install();
        let dir = tempfile::tempdir().unwrap();
        let path = servers_file(dir.path());

        assert!(saved_servers(&path).unwrap().is_empty());
        assert!(!path.parent().unwrap().exists());
    }

    #[test]
    fn server_credentials_reports_an_unknown_host_and_a_bad_file() {
        marlin_test_keychain::install();
        let dir = tempfile::tempdir().unwrap();
        let missing = servers_file(dir.path());
        assert_eq!(
            server_credentials(&missing, "unknown.example", 22).unwrap_err(),
            CredentialsError::NotSaved(
                "[SFTP_NO_CREDENTIALS] No credentials stored for server: unknown.example:22"
                    .to_string()
            )
        );

        let bad_dir = tempfile::tempdir().unwrap();
        let bad = bad_dir.path().join("sftp-servers.json");
        fs::write(&bad, "not json").unwrap();
        match server_credentials(&bad, "badjson.example", 22) {
            Err(CredentialsError::Failed(text)) => {
                assert!(text.starts_with("Failed to parse servers file: "), "{text}");
            }
            other => panic!("expected a parse failure, got {other:?}"),
        }
    }
}
