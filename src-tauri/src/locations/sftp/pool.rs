use once_cell::sync::Lazy;
use russh_sftp::client::SftpSession;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, Semaphore};

use super::host_key;
use marlin_sftp::{Passphrase, SftpServerCredentials, SignIn};

type PoolKey = (String, u16); // (hostname, port)

struct PooledConnection {
    sftp: Arc<SftpSession>,
    semaphore: Arc<Semaphore>,
    last_used: Instant,
}

static POOL: Lazy<Mutex<HashMap<PoolKey, PooledConnection>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

const IDLE_TIMEOUT_SECS: u64 = 300; // 5 minutes
/// Max concurrent SFTP file transfer operations per server.
/// Many corporate SFTP servers (e.g., FedEx) can't handle parallel reads at all.
const MAX_CONCURRENT_OPS: usize = 1;
/// Skip the liveness check if the connection was used within this many seconds.
/// Avoids a network round trip per operation when many requests are queued.
const LIVENESS_SKIP_SECS: u64 = 30;

/// Connects, and keeps an unknown host key for the trust prompt.
async fn connect(
    hostname: &str,
    port: u16,
    creds: &SftpServerCredentials,
    known_hosts: &Path,
) -> Result<SftpSession, String> {
    match marlin_sftp::connect(hostname, port, creds, known_hosts).await {
        Ok(session) => Ok(session),
        Err(marlin_sftp::ConnectError::UnknownHostKey(key)) => {
            let unknown = host_key::remember_unknown_host_key(hostname, port, key, known_hosts);
            Err(format!(
                "[SFTP_HOST_KEY_UNKNOWN] {}:{} is not in {}. Its {} key fingerprint is {}.",
                hostname, port, unknown.known_hosts_path, unknown.key_type, unknown.fingerprint
            ))
        }
        Err(error) => Err(error.to_string()),
    }
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
    let creds = marlin_sftp::server_credentials(&marlin_sftp::servers_path()?, hostname, port)
        .map_err(|e| e.to_string())?;
    let known_hosts = marlin_sftp::known_hosts_path()?;
    let sftp = Arc::new(connect(hostname, port, &creds, &known_hosts).await?);
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
pub async fn acquire_permit(
    hostname: &str,
    port: u16,
) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
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
    let sign_in = match auth_method {
        "password" => SignIn::Password(
            password
                .ok_or_else(|| "Password auth selected but no password available".to_string())?
                .to_string(),
        ),
        "key" => SignIn::Key {
            path: key_path
                .ok_or_else(|| "Key auth selected but no key path available".to_string())?
                .to_string(),
            passphrase: password.map_or(Passphrase::None, |s| Passphrase::Given(s.to_string())),
        },
        "agent" => SignIn::Agent,
        other => return Err(format!("Unknown auth method: {}", other)),
    };
    let creds = SftpServerCredentials {
        username: username.to_string(),
        sign_in,
    };

    let known_hosts = marlin_sftp::known_hosts_path()?;
    let sftp = connect(hostname, port, &creds, &known_hosts).await?;

    // Verify we can actually list the root directory
    sftp.metadata(".")
        .await
        .map_err(|e| format!("SFTP session created but root stat failed: {}", e))?;

    Ok(true)
}
