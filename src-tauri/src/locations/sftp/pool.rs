use once_cell::sync::Lazy;
use russh::keys::PublicKey;
use russh::{client, ChannelId};
use russh_sftp::client::SftpSession;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore};

use super::auth;

type PoolKey = (String, u16); // (hostname, port)

struct PooledConnection {
    sftp: Arc<SftpSession>,
    semaphore: Arc<Semaphore>,
    last_used: Instant,
}

static POOL: Lazy<Mutex<HashMap<PoolKey, PooledConnection>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

const IDLE_TIMEOUT_SECS: u64 = 300; // 5 minutes
const CONNECT_TIMEOUT_SECS: u64 = 15;
/// Max concurrent SFTP file transfer operations per server.
/// Many corporate SFTP servers (e.g., FedEx) can't handle parallel reads at all.
const MAX_CONCURRENT_OPS: usize = 1;
/// Skip the liveness check if the connection was used within this many seconds.
/// Avoids a network round trip per operation when many requests are queued.
const LIVENESS_SKIP_SECS: u64 = 30;

/// SSH client handler that accepts all host keys
struct SshHandler;

impl client::Handler for SshHandler {
    type Error = russh::Error;

    fn check_server_key(
        &mut self,
        _server_public_key: &PublicKey,
    ) -> impl std::future::Future<Output = Result<bool, Self::Error>> + Send {
        // TODO: known_hosts verification
        async { Ok(true) }
    }

    fn channel_open_confirmation(
        &mut self,
        _channel: ChannelId,
        _max_packet_size: u32,
        _window_size: u32,
        _session: &mut client::Session,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        async { Ok(()) }
    }

    fn data(
        &mut self,
        _channel: ChannelId,
        _data: &[u8],
        _session: &mut client::Session,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        async { Ok(()) }
    }
}

/// Create a new SFTP session from credentials
async fn create_session(
    hostname: &str,
    port: u16,
    creds: &auth::SftpServerCredentials,
) -> Result<SftpSession, String> {
    let config = client::Config {
        ..Default::default()
    };

    let mut session = tokio::time::timeout(
        Duration::from_secs(CONNECT_TIMEOUT_SECS),
        client::connect(Arc::new(config), (hostname, port), SshHandler),
    )
    .await
    .map_err(|_| format!("SSH connection to {}:{} timed out after {}s", hostname, port, CONNECT_TIMEOUT_SECS))?
    .map_err(|e| format!("SSH connection failed: {}", e))?;

    // Authenticate
    match creds.auth_method.as_str() {
        "password" => {
            let password = creds
                .password
                .as_deref()
                .ok_or("Password auth selected but no password available")?;
            let auth_result = session
                .authenticate_password(&creds.username, password)
                .await
                .map_err(|e| format!("Password authentication failed: {}", e))?;
            if !auth_result.success() {
                return Err("Password authentication rejected by server".to_string());
            }
        }
        "key" => {
            let key_path_str = creds
                .key_path
                .as_deref()
                .ok_or("Key auth selected but no key path available")?;

            // Expand ~ in key path
            let expanded = if key_path_str.starts_with('~') {
                if let Some(home) = dirs::home_dir() {
                    home.join(&key_path_str[1..].trim_start_matches('/'))
                } else {
                    std::path::PathBuf::from(key_path_str)
                }
            } else {
                std::path::PathBuf::from(key_path_str)
            };

            let private_key = if let Some(passphrase) = creds.password.as_deref() {
                russh::keys::load_secret_key(&expanded, Some(passphrase))
                    .map_err(|e| format!("Failed to load SSH key: {}", e))?
            } else {
                russh::keys::load_secret_key(&expanded, None)
                    .map_err(|e| format!("Failed to load SSH key: {}", e))?
            };

            let key_with_alg = russh::keys::PrivateKeyWithHashAlg::new(
                Arc::new(private_key),
                None,
            );

            let auth_result = session
                .authenticate_publickey(&creds.username, key_with_alg)
                .await
                .map_err(|e| format!("Key authentication failed: {}", e))?;
            if !auth_result.success() {
                return Err("Key authentication rejected by server".to_string());
            }
        }
        "agent" => {
            let mut agent = russh::keys::agent::client::AgentClient::connect_env()
                .await
                .map_err(|e| format!("Failed to connect to SSH agent: {}", e))?;

            let identities = agent
                .request_identities()
                .await
                .map_err(|e| format!("Failed to list SSH agent identities: {}", e))?;

            if identities.is_empty() {
                return Err("No keys available in SSH agent".to_string());
            }

            let mut authenticated = false;
            for key in identities {
                match session
                    .authenticate_publickey_with(&creds.username, key, None, &mut agent)
                    .await
                {
                    Ok(result) if result.success() => {
                        authenticated = true;
                        break;
                    }
                    Ok(_) => continue,
                    Err(_) => continue,
                }
            }

            if !authenticated {
                return Err("No SSH agent key was accepted by the server".to_string());
            }
        }
        other => {
            return Err(format!("Unknown auth method: {}", other));
        }
    }

    // Open SFTP channel
    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| format!("Failed to open SSH channel: {}", e))?;

    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("Failed to request SFTP subsystem: {}", e))?;

    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| format!("Failed to initialize SFTP session: {}", e))?;

    Ok(sftp)
}

/// Get an SFTP session, reusing from pool if possible.
/// Returns an Arc<SftpSession> since SftpSession is not Clone.
pub async fn get_sftp_session(hostname: &str, port: u16) -> Result<Arc<SftpSession>, String> {
    let key = (hostname.to_lowercase(), port);

    // Try to reuse a pooled connection — only hold the lock briefly (no network I/O)
    let cached = {
        let mut pool = POOL.lock().await;
        if let Some(conn) = pool.get_mut(&key) {
            let elapsed = conn.last_used.elapsed().as_secs();
            if elapsed < IDLE_TIMEOUT_SECS {
                conn.last_used = Instant::now();
                Some((conn.sftp.clone(), elapsed))
            } else {
                pool.remove(&key);
                None
            }
        } else {
            None
        }
    };
    // Lock is released here

    if let Some((sftp, elapsed)) = cached {
        // Skip liveness check if used recently — avoids a network round trip per call
        if elapsed < LIVENESS_SKIP_SECS {
            return Ok(sftp);
        }

        // Verify liveness outside the lock
        match sftp.metadata(".").await {
            Ok(_) => return Ok(sftp),
            Err(_) => {
                // Connection is dead, remove from pool
                let mut pool = POOL.lock().await;
                pool.remove(&key);
            }
        }
    }

    // Create a new connection
    let creds = auth::get_server_credentials(hostname, port)?;
    let sftp = Arc::new(create_session(hostname, port, &creds).await?);
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_OPS));

    // Store in pool
    {
        let mut pool = POOL.lock().await;
        pool.insert(
            key,
            PooledConnection {
                sftp: sftp.clone(),
                semaphore: semaphore.clone(),
                last_used: Instant::now(),
            },
        );
    }

    Ok(sftp)
}

/// Acquire a concurrency permit for the given server.
/// Limits parallel SFTP operations to avoid overwhelming servers.
/// Hold the returned permit for the duration of the operation.
pub async fn acquire_permit(hostname: &str, port: u16) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
    let key = (hostname.to_lowercase(), port);
    let sem = {
        let pool = POOL.lock().await;
        pool.get(&key)
            .map(|c| c.semaphore.clone())
            .ok_or_else(|| "No pooled connection for permit".to_string())?
    };
    sem.acquire_owned()
        .await
        .map_err(|e| format!("Failed to acquire SFTP permit: {}", e))
}

/// Drop all connections for a specific server
pub async fn drop_connections(hostname: &str, port: u16) {
    let key = (hostname.to_lowercase(), port);
    let mut pool = POOL.lock().await;
    pool.remove(&key);
}

/// Test SFTP connection without caching the session
pub async fn test_connection(
    hostname: &str,
    port: u16,
    username: &str,
    password: Option<&str>,
    auth_method: &str,
    key_path: Option<&str>,
) -> Result<bool, String> {
    let creds = auth::SftpServerCredentials {
        username: username.to_string(),
        auth_method: auth_method.to_string(),
        password: password.map(|s| s.to_string()),
        key_path: key_path.map(|s| s.to_string()),
    };

    let sftp = create_session(hostname, port, &creds).await?;

    // Verify we can actually list the root directory
    sftp.metadata(".")
        .await
        .map_err(|e| format!("SFTP session created but root stat failed: {}", e))?;

    Ok(true)
}
