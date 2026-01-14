use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::RwLock;

/// Information about a connected SMB server (safe to expose to frontend)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmbServerInfo {
    pub hostname: String,
    pub username: String,
    pub domain: Option<String>,
}

/// Stored server data with credentials
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmbServer {
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

    let storage: ServerStorage = serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse servers file: {}", e))?;

    Ok(storage.servers)
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
pub fn get_server_credentials(hostname: &str) -> Result<SmbServer, String> {
    // Check cache first
    {
        let cache = SERVERS_CACHE.read().map_err(|e| e.to_string())?;
        if let Some(servers) = &*cache {
            if let Some(server) = servers
                .iter()
                .find(|s| s.hostname.eq_ignore_ascii_case(hostname))
            {
                return Ok(server.clone());
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

    servers
        .iter()
        .find(|s| s.hostname.eq_ignore_ascii_case(hostname))
        .cloned()
        .ok_or_else(|| format!("[SMB_NO_CREDENTIALS] No credentials stored for server: {}", hostname))
}

/// Add a new SMB server
pub fn add_smb_server(
    hostname: String,
    username: String,
    password: String,
    domain: Option<String>,
) -> Result<SmbServerInfo, String> {
    let mut servers = load_servers_from_disk()?;

    // Check if server already exists
    if let Some(existing) = servers
        .iter_mut()
        .find(|s| s.hostname.eq_ignore_ascii_case(&hostname))
    {
        // Update existing
        existing.username = username.clone();
        existing.password = password;
        existing.domain = domain.clone();
    } else {
        // Add new
        servers.push(SmbServer {
            hostname: hostname.clone(),
            username: username.clone(),
            password,
            domain: domain.clone(),
        });
    }

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
    servers.retain(|s| !s.hostname.eq_ignore_ascii_case(hostname));

    if servers.len() == original_len {
        return Err(format!("Server not found: {}", hostname));
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
#[cfg(feature = "smb")]
pub fn test_smb_connection(
    hostname: &str,
    username: &str,
    password: &str,
    domain: Option<&str>,
) -> Result<bool, String> {
    use pavao::{SmbClient, SmbCredentials, SmbOptions};

    let smb_url = format!("smb://{}", hostname);

    let mut credentials = SmbCredentials::default()
        .server(&smb_url)
        .share("/")
        .username(username)
        .password(password);

    if let Some(d) = domain {
        credentials = credentials.workgroup(d);
    }

    match SmbClient::new(credentials, SmbOptions::default()) {
        Ok(_) => Ok(true),
        Err(e) => Err(format!("Connection failed: {}", e)),
    }
}

/// Test connection stub when SMB feature is disabled
#[cfg(not(feature = "smb"))]
pub fn test_smb_connection(
    _hostname: &str,
    _username: &str,
    _password: &str,
    _domain: Option<&str>,
) -> Result<bool, String> {
    Err("SMB support not compiled. Build with --features smb".to_string())
}
