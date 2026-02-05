use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::RwLock;

const SFTP_KEYRING_SERVICE: &str = "marlin-sftp";

/// Information about a connected SFTP server (safe to expose to frontend)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpServerInfo {
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub auth_method: String, // "password", "key", "agent"
    pub key_path: Option<String>,
}

/// Stored server data on disk (no secrets)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SftpServer {
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub auth_method: String,
    pub key_path: Option<String>,
}

/// Server credentials resolved from keychain (internal use)
#[derive(Debug, Clone)]
pub struct SftpServerCredentials {
    pub username: String,
    pub auth_method: String,
    pub password: Option<String>,
    pub key_path: Option<String>,
}

/// Storage structure for servers file
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ServerStorage {
    servers: Vec<SftpServer>,
}

/// In-memory cache of servers
static SERVERS_CACHE: Lazy<RwLock<Option<Vec<SftpServer>>>> = Lazy::new(|| RwLock::new(None));

fn keyring_user(hostname: &str, port: u16, username: &str) -> String {
    format!("{}@{}:{}", username, hostname, port)
}

fn keyring_entry(hostname: &str, port: u16, username: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SFTP_KEYRING_SERVICE, &keyring_user(hostname, port, username))
        .map_err(|e| format!("Failed to create keyring entry: {}", e))
}

fn set_password(hostname: &str, port: u16, username: &str, password: &str) -> Result<(), String> {
    let entry = keyring_entry(hostname, port, username)?;
    match entry.set_password(password) {
        Ok(()) => Ok(()),
        Err(keyring::Error::Ambiguous(_)) => {
            let _ = entry.delete_credential();
            let entry = keyring_entry(hostname, port, username)?;
            entry
                .set_password(password)
                .map_err(|e| format!("Failed to store password in keychain: {}", e))
        }
        Err(e) => {
            let msg = e.to_string().to_lowercase();
            if msg.contains("already exists") || msg.contains("duplicate") {
                let _ = entry.delete_credential();
                let entry = keyring_entry(hostname, port, username)?;
                entry.set_password(password).map_err(|retry_err| {
                    format!("Failed to store password in keychain: {}", retry_err)
                })
            } else {
                Err(format!("Failed to store password in keychain: {}", e))
            }
        }
    }
}

fn get_password(hostname: &str, port: u16, username: &str) -> Result<String, String> {
    let entry = keyring_entry(hostname, port, username)?;
    entry.get_password().map_err(|e| {
        format!(
            "[SFTP_NO_CREDENTIALS] Failed to read password from keychain: {}",
            e
        )
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

fn get_servers_path() -> Result<PathBuf, String> {
    let config_dir =
        dirs::config_dir().ok_or_else(|| "Could not determine config directory".to_string())?;
    let marlin_dir = config_dir.join("marlin");

    if !marlin_dir.exists() {
        fs::create_dir_all(&marlin_dir)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }

    Ok(marlin_dir.join("sftp-servers.json"))
}

fn load_servers_from_disk() -> Result<Vec<SftpServer>, String> {
    let path = get_servers_path()?;

    if !path.exists() {
        return Ok(Vec::new());
    }

    let contents =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read servers file: {}", e))?;

    let storage: ServerStorage = serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse servers file: {}", e))?;

    Ok(storage.servers)
}

fn save_servers_to_disk(servers: &[SftpServer]) -> Result<(), String> {
    let path = get_servers_path()?;

    let storage = ServerStorage {
        servers: servers.to_vec(),
    };

    let contents = serde_json::to_string_pretty(&storage)
        .map_err(|e| format!("Failed to serialize servers: {}", e))?;

    fs::write(&path, contents).map_err(|e| format!("Failed to write servers file: {}", e))?;

    Ok(())
}

/// Get all connected SFTP servers (safe info only)
pub fn get_sftp_servers() -> Result<Vec<SftpServerInfo>, String> {
    {
        let cache = SERVERS_CACHE.read().map_err(|e| e.to_string())?;
        if let Some(servers) = &*cache {
            return Ok(servers
                .iter()
                .map(|s| SftpServerInfo {
                    hostname: s.hostname.clone(),
                    port: s.port,
                    username: s.username.clone(),
                    auth_method: s.auth_method.clone(),
                    key_path: s.key_path.clone(),
                })
                .collect());
        }
    }

    let servers = load_servers_from_disk()?;

    {
        let mut cache = SERVERS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(servers.clone());
    }

    Ok(servers
        .iter()
        .map(|s| SftpServerInfo {
            hostname: s.hostname.clone(),
            port: s.port,
            username: s.username.clone(),
            auth_method: s.auth_method.clone(),
            key_path: s.key_path.clone(),
        })
        .collect())
}

/// Get credentials for a specific server (internal use)
pub fn get_server_credentials(hostname: &str, port: u16) -> Result<SftpServerCredentials, String> {
    {
        let cache = SERVERS_CACHE.read().map_err(|e| e.to_string())?;
        if let Some(servers) = &*cache {
            if let Some(server) = servers.iter().find(|s| {
                s.hostname.eq_ignore_ascii_case(hostname) && s.port == port
            }) {
                let password = if server.auth_method == "password" {
                    Some(get_password(&server.hostname, server.port, &server.username)?)
                } else if server.auth_method == "key" {
                    // Key passphrase stored in keychain (may be empty string for no passphrase)
                    get_password(&server.hostname, server.port, &server.username).ok()
                } else {
                    None
                };
                return Ok(SftpServerCredentials {
                    username: server.username.clone(),
                    auth_method: server.auth_method.clone(),
                    password,
                    key_path: server.key_path.clone(),
                });
            }
        }
    }

    let servers = load_servers_from_disk()?;

    {
        let mut cache = SERVERS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(servers.clone());
    }

    let server = servers
        .iter()
        .find(|s| s.hostname.eq_ignore_ascii_case(hostname) && s.port == port)
        .cloned()
        .ok_or_else(|| {
            format!(
                "[SFTP_NO_CREDENTIALS] No credentials stored for server: {}:{}",
                hostname, port
            )
        })?;

    let password = if server.auth_method == "password" {
        Some(get_password(&server.hostname, server.port, &server.username)?)
    } else if server.auth_method == "key" {
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

/// Add a new SFTP server
pub fn add_sftp_server(
    hostname: String,
    port: u16,
    username: String,
    password: Option<String>,
    auth_method: String,
    key_path: Option<String>,
) -> Result<SftpServerInfo, String> {
    let mut servers = load_servers_from_disk()?;

    if let Some(existing) = servers.iter_mut().find(|s| {
        s.hostname.eq_ignore_ascii_case(&hostname) && s.port == port
    }) {
        existing.username = username.clone();
        existing.auth_method = auth_method.clone();
        existing.key_path = key_path.clone();
    } else {
        servers.push(SftpServer {
            hostname: hostname.clone(),
            port,
            username: username.clone(),
            auth_method: auth_method.clone(),
            key_path: key_path.clone(),
        });
    }

    // Clear old keychain entry when switching auth methods (prevents stale password being
    // used as key passphrase or vice-versa)
    if password.as_ref().map_or(true, |pw| pw.is_empty()) {
        let _ = delete_password(&hostname, port, &username);
    }

    // Store password/passphrase in keychain if provided
    if let Some(pw) = &password {
        if !pw.is_empty() {
            set_password(&hostname, port, &username, pw)?;
        }
    }

    save_servers_to_disk(&servers)?;

    {
        let mut cache = SERVERS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(servers);
    }

    Ok(SftpServerInfo {
        hostname,
        port,
        username,
        auth_method,
        key_path,
    })
}

/// Remove an SFTP server
pub fn remove_sftp_server(hostname: &str, port: u16) -> Result<(), String> {
    let mut servers = load_servers_from_disk()?;

    // Find the username before removing, so we can delete the correct keychain entry
    let username = servers
        .iter()
        .find(|s| s.hostname.eq_ignore_ascii_case(hostname) && s.port == port)
        .map(|s| s.username.clone());

    let original_len = servers.len();
    servers.retain(|s| !(s.hostname.eq_ignore_ascii_case(hostname) && s.port == port));

    if servers.len() == original_len {
        return Err(format!("Server not found: {}:{}", hostname, port));
    }

    if let Some(ref user) = username {
        let _ = delete_password(hostname, port, user);
    }
    save_servers_to_disk(&servers)?;

    {
        let mut cache = SERVERS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(servers);
    }

    Ok(())
}
