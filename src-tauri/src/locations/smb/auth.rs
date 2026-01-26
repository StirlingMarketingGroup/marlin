use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::RwLock;

const SMB_KEYRING_SERVICE: &str = "marlin-smb";

/// Information about a connected SMB server (safe to expose to frontend)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmbServerInfo {
    pub hostname: String,
    pub username: String,
    pub domain: Option<String>,
}

/// Stored server data on disk (no secrets)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmbServer {
    pub hostname: String,
    pub username: String,
    pub domain: Option<String>,
}

/// Server credentials resolved from keychain (internal use)
#[cfg_attr(not(feature = "smb"), allow(dead_code))]
#[derive(Debug, Clone)]
pub struct SmbServerCredentials {
    pub hostname: String,
    pub username: String,
    pub password: String,
    pub domain: Option<String>,
}

/// Storage structure for servers file
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ServerStorage {
    servers: Vec<SmbServer>,
}

/// In-memory cache of servers
static SERVERS_CACHE: Lazy<RwLock<Option<Vec<SmbServer>>>> = Lazy::new(|| RwLock::new(None));

fn keyring_entry(hostname: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SMB_KEYRING_SERVICE, hostname)
        .map_err(|e| format!("Failed to create keyring entry: {}", e))
}

fn set_password(hostname: &str, password: &str) -> Result<(), String> {
    let entry = keyring_entry(hostname)?;
    entry
        .set_password(password)
        .map_err(|e| format!("Failed to store password in keychain: {}", e))
}

fn get_password(hostname: &str) -> Result<String, String> {
    let entry = keyring_entry(hostname)?;
    entry
        .get_password()
        .map_err(|e| format!("[SMB_NO_CREDENTIALS] Failed to read password from keychain: {}", e))
}

fn delete_password(hostname: &str) -> Result<(), String> {
    let entry = keyring_entry(hostname)?;
    // Ignore missing entries; we still want server removal to succeed.
    match entry.delete_credential() {
        Ok(_) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Failed to delete password from keychain: {}", e)),
    }
}

/// Get the path to the servers storage file
fn get_servers_path() -> Result<PathBuf, String> {
    let config_dir = dirs::config_dir()
        .ok_or_else(|| "Could not determine config directory".to_string())?;
    let marlin_dir = config_dir.join("marlin");

    if !marlin_dir.exists() {
        fs::create_dir_all(&marlin_dir)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }

    Ok(marlin_dir.join("smb-servers.json"))
}

/// Load servers from disk
fn load_servers_from_disk() -> Result<Vec<SmbServer>, String> {
    let path = get_servers_path()?;

    if !path.exists() {
        return Ok(Vec::new());
    }

    let contents = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read servers file: {}", e))?;

    #[derive(Debug, Clone, Serialize, Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct SmbServerDisk {
        hostname: String,
        username: String,
        #[serde(default)]
        password: Option<String>,
        domain: Option<String>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, Default)]
    struct ServerStorageDisk {
        servers: Vec<SmbServerDisk>,
    }

    let storage: ServerStorageDisk =
        serde_json::from_str(&contents).map_err(|e| format!("Failed to parse servers file: {}", e))?;

    // Migrate any legacy plaintext passwords into the OS keychain, and rewrite file without them.
    let mut migrated = false;
    let mut servers: Vec<SmbServer> = Vec::with_capacity(storage.servers.len());
    for server in storage.servers {
        if let Some(password) = server.password.as_deref() {
            set_password(&server.hostname, password)?;
            migrated = true;
        }

        servers.push(SmbServer {
            hostname: server.hostname,
            username: server.username,
            domain: server.domain,
        });
    }

    if migrated {
        save_servers_to_disk(&servers)?;
    }

    Ok(servers)
}

/// Save servers to disk
fn save_servers_to_disk(servers: &[SmbServer]) -> Result<(), String> {
    let path = get_servers_path()?;

    let storage = ServerStorage {
        servers: servers.to_vec(),
    };

    let contents = serde_json::to_string_pretty(&storage)
        .map_err(|e| format!("Failed to serialize servers: {}", e))?;

    fs::write(&path, contents)
        .map_err(|e| format!("Failed to write servers file: {}", e))?;

    Ok(())
}

/// Get all connected SMB servers (safe info only)
pub fn get_smb_servers() -> Result<Vec<SmbServerInfo>, String> {
    // Check cache first
    {
        let cache = SERVERS_CACHE.read().map_err(|e| e.to_string())?;
        if let Some(servers) = &*cache {
            return Ok(servers
                .iter()
                .map(|s| SmbServerInfo {
                    hostname: s.hostname.clone(),
                    username: s.username.clone(),
                    domain: s.domain.clone(),
                })
                .collect());
        }
    }

    // Load from disk
    let servers = load_servers_from_disk()?;

    // Update cache
    {
        let mut cache = SERVERS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(servers.clone());
    }

    Ok(servers
        .iter()
        .map(|s| SmbServerInfo {
            hostname: s.hostname.clone(),
            username: s.username.clone(),
            domain: s.domain.clone(),
        })
        .collect())
}

/// Get credentials for a specific server (internal use)
#[cfg_attr(not(feature = "smb"), allow(dead_code))]
pub fn get_server_credentials(hostname: &str) -> Result<SmbServerCredentials, String> {
    // Check cache first
    {
        let cache = SERVERS_CACHE.read().map_err(|e| e.to_string())?;
        if let Some(servers) = &*cache {
            if let Some(server) = servers
                .iter()
                .find(|s| s.hostname.eq_ignore_ascii_case(hostname))
            {
                let password = get_password(&server.hostname)?;
                return Ok(SmbServerCredentials {
                    hostname: server.hostname.clone(),
                    username: server.username.clone(),
                    password,
                    domain: server.domain.clone(),
                });
            }
        }
    }

    // Load from disk
    let servers = load_servers_from_disk()?;

    // Update cache
    {
        let mut cache = SERVERS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(servers.clone());
    }

    let server = servers
        .iter()
        .find(|s| s.hostname.eq_ignore_ascii_case(hostname))
        .cloned()
        .ok_or_else(|| format!("[SMB_NO_CREDENTIALS] No credentials stored for server: {}", hostname))?;

    let password = get_password(&server.hostname)?;
    Ok(SmbServerCredentials {
        hostname: server.hostname,
        username: server.username,
        password,
        domain: server.domain,
    })
}

/// Add a new SMB server
pub fn add_smb_server(
    hostname: String,
    username: String,
    password: String,
    domain: Option<String>,
) -> Result<SmbServerInfo, String> {
    let mut servers = load_servers_from_disk()?;
    let mut keychain_hostname = hostname.clone();

    // Check if server already exists
    if let Some(existing) = servers
        .iter_mut()
        .find(|s| s.hostname.eq_ignore_ascii_case(&hostname))
    {
        // Update existing
        keychain_hostname = existing.hostname.clone();
        existing.username = username.clone();
        existing.domain = domain.clone();
    } else {
        // Add new
        servers.push(SmbServer {
            hostname: hostname.clone(),
            username: username.clone(),
            domain: domain.clone(),
        });
    }

    set_password(&keychain_hostname, &password)?;
    save_servers_to_disk(&servers)?;

    // Update cache
    {
        let mut cache = SERVERS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(servers);
    }

    Ok(SmbServerInfo {
        hostname,
        username,
        domain,
    })
}

/// Remove an SMB server
pub fn remove_smb_server(hostname: &str) -> Result<(), String> {
    let mut servers = load_servers_from_disk()?;

    let original_len = servers.len();
    let stored_hostname = servers
        .iter()
        .find(|s| s.hostname.eq_ignore_ascii_case(hostname))
        .map(|s| s.hostname.clone());
    servers.retain(|s| !s.hostname.eq_ignore_ascii_case(hostname));

    if servers.len() == original_len {
        return Err(format!("Server not found: {}", hostname));
    }

    if let Some(stored_hostname) = stored_hostname {
        delete_password(&stored_hostname)?;
    }
    save_servers_to_disk(&servers)?;

    // Update cache
    {
        let mut cache = SERVERS_CACHE.write().map_err(|e| e.to_string())?;
        *cache = Some(servers);
    }

    Ok(())
}

/// Test connection to an SMB server (without saving credentials)
pub fn test_smb_connection(
    hostname: &str,
    username: &str,
    password: &str,
    domain: Option<&str>,
) -> Result<bool, String> {
    use super::client::{self, SidecarStatus};

    // Check sidecar availability
    if !client::is_available() {
        let status = client::initialize();
        if status != SidecarStatus::Available {
            return Err(status.error_message().unwrap_or_else(|| {
                "SMB support is not available".to_string()
            }));
        }
    }

    let params = serde_json::json!({
        "credentials": {
            "hostname": hostname,
            "username": username,
            "password": password,
            "domain": domain
        }
    });

    let result: serde_json::Value = client::call_method("test_connection", params)?;

    let success = result
        .get("success")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);

    Ok(success)
}
