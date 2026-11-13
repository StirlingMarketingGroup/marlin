use once_cell::sync::Lazy;
use russh::keys::PublicKey;
use russh::{client, ChannelId};
use russh_sftp::client::SftpSession;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore};

use super::auth;
use super::host_key::{self, HostKeyRejection};

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

#[derive(Debug)]
enum ConnectError {
    Ssh(russh::Error),
    HostKey(PublicKey, HostKeyRejection),
}

impl From<russh::Error> for ConnectError {
    fn from(error: russh::Error) -> Self {
        ConnectError::Ssh(error)
    }
}

/// SSH client handler that accepts only a server key saved in `known_hosts`.
struct SshHandler {
    saved_host_keys: Vec<PublicKey>,
}

impl client::Handler for SshHandler {
    type Error = ConnectError;

    fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> impl std::future::Future<Output = Result<bool, Self::Error>> + Send {
        let checked = host_key::check_host_key(server_public_key, &self.saved_host_keys)
            .map(|()| true)
            .map_err(|rejection| ConnectError::HostKey(server_public_key.clone(), rejection));
        async move { checked }
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

/// Create a new SFTP session from credentials.
async fn create_session(
    hostname: &str,
    port: u16,
    creds: &auth::SftpServerCredentials,
    known_hosts: &Path,
) -> Result<SftpSession, String> {
    let saved_host_keys = host_key::saved_host_keys(hostname, port, known_hosts)?;
    let config = client::Config {
        preferred: russh::Preferred {
            key: host_key::preferred_host_key_algorithms(&saved_host_keys).into(),
            ..Default::default()
        },
        ..Default::default()
    };

    let connect_result = tokio::time::timeout(
        Duration::from_secs(CONNECT_TIMEOUT_SECS),
        client::connect(
            Arc::new(config),
            (hostname, port),
            SshHandler { saved_host_keys },
        ),
    )
    .await
    .map_err(|_| {
        format!(
            "SSH connection to {}:{} timed out after {}s",
            hostname, port, CONNECT_TIMEOUT_SECS
        )
    })?;

    let mut session = match connect_result {
        Ok(session) => session,
        Err(ConnectError::HostKey(key, HostKeyRejection::Unknown)) => {
            let unknown = host_key::remember_unknown_host_key(hostname, port, key, known_hosts);
            return Err(format!(
                "[SFTP_HOST_KEY_UNKNOWN] {}:{} is not in {}. Its {} key fingerprint is {}.",
                hostname, port, unknown.known_hosts_path, unknown.key_type, unknown.fingerprint
            ));
        }
        Err(ConnectError::HostKey(key, HostKeyRejection::Changed { saved_fingerprint })) => {
            return Err(host_key::changed_host_key_warning(
                hostname,
                port,
                &key,
                &saved_fingerprint,
                known_hosts,
            ));
        }
        Err(ConnectError::Ssh(error)) => {
            return Err(format!("SSH connection failed: {}", error));
        }
    };

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

            let key_with_alg = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(private_key), None);

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
            for identity in identities {
                let auth_result = match &identity {
                    russh::keys::agent::AgentIdentity::PublicKey { key, .. } => {
                        session
                            .authenticate_publickey_with(
                                &creds.username,
                                key.clone(),
                                None,
                                &mut agent,
                            )
                            .await
                    }
                    russh::keys::agent::AgentIdentity::Certificate { certificate, .. } => {
                        session
                            .authenticate_certificate_with(
                                &creds.username,
                                certificate.clone(),
                                None,
                                &mut agent,
                            )
                            .await
                    }
                };

                match auth_result {
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
    let known_hosts = host_key::known_hosts_path()?;
    let sftp = Arc::new(create_session(hostname, port, &creds, &known_hosts).await?);
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
    let creds = auth::SftpServerCredentials {
        username: username.to_string(),
        auth_method: auth_method.to_string(),
        password: password.map(|s| s.to_string()),
        key_path: key_path.map(|s| s.to_string()),
    };

    let known_hosts = host_key::known_hosts_path()?;
    let sftp = create_session(hostname, port, &creds, &known_hosts).await?;

    // Verify we can actually list the root directory
    sftp.metadata(".")
        .await
        .map_err(|e| format!("SFTP session created but root stat failed: {}", e))?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::{Algorithm, EcdsaCurve, HashAlg, PrivateKey};
    use russh::server::{Auth, Handler, Server};
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Rejects every login and counts the passwords it was sent.
    struct RejectLogins {
        passwords_seen: Arc<AtomicUsize>,
    }

    impl Handler for RejectLogins {
        type Error = russh::Error;

        async fn auth_password(
            &mut self,
            _user: &str,
            _password: &str,
        ) -> Result<Auth, Self::Error> {
            self.passwords_seen.fetch_add(1, Ordering::SeqCst);
            Ok(Auth::reject())
        }
    }

    struct RejectServer {
        passwords_seen: Arc<AtomicUsize>,
    }

    impl Server for RejectServer {
        type Handler = RejectLogins;

        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
            RejectLogins {
                passwords_seen: self.passwords_seen.clone(),
            }
        }
    }

    fn password_creds() -> auth::SftpServerCredentials {
        auth::SftpServerCredentials {
            username: "user".to_string(),
            auth_method: "password".to_string(),
            password: Some("secret".to_string()),
            key_path: None,
        }
    }

    fn write_host_line(path: &Path, entry: &str, key: &PublicKey) {
        fs::write(
            path,
            format!("{entry} {}\n", key.to_openssh().expect("encode public key")),
        )
        .unwrap();
    }

    async fn session_error(port: u16, known_hosts: &Path) -> String {
        match create_session("127.0.0.1", port, &password_creds(), known_hosts).await {
            Ok(_) => panic!("expected the connection to fail"),
            Err(error) => error,
        }
    }

    #[tokio::test]
    async fn create_session_checks_the_server_key_against_known_hosts() {
        let host_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let server_public = host_key.public_key().clone();
        let server_fingerprint = server_public.fingerprint(HashAlg::Sha256).to_string();

        let mut server_config = russh::server::Config::default();
        server_config.auth_rejection_time = Duration::from_millis(0);
        server_config.auth_rejection_time_initial = Some(Duration::from_millis(0));
        server_config.inactivity_timeout = None;
        server_config.keys.push(host_key);
        let server_config = Arc::new(server_config);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let passwords_seen = Arc::new(AtomicUsize::new(0));
        let server_passwords_seen = passwords_seen.clone();
        tokio::spawn(async move {
            let mut server = RejectServer {
                passwords_seen: server_passwords_seen,
            };
            let _ = server.run_on_socket(server_config, &listener).await;
        });

        let unknown_dir = tempfile::tempdir().unwrap();
        let unknown_path = unknown_dir.path().join("known_hosts");
        fs::write(&unknown_path, b"").unwrap();

        let unknown = session_error(port, &unknown_path).await;
        assert!(unknown.starts_with("[SFTP_HOST_KEY_UNKNOWN]"), "{unknown}");
        let waiting = host_key::unknown_host_key("127.0.0.1", port).expect("unknown key");
        assert_eq!(waiting.fingerprint, server_fingerprint);
        assert_eq!(passwords_seen.load(Ordering::SeqCst), 0);

        host_key::trust_unknown_host_key("127.0.0.1", port, &server_fingerprint, &unknown_path)
            .unwrap();
        let trusted = session_error(port, &unknown_path).await;
        assert_eq!(trusted, "Password authentication rejected by server");
        assert_eq!(passwords_seen.load(Ordering::SeqCst), 1);

        let changed_dir = tempfile::tempdir().unwrap();
        let changed_path = changed_dir.path().join("known_hosts");
        let other_ed25519 = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let entry = format!("[127.0.0.1]:{port}");
        write_host_line(&changed_path, &entry, other_ed25519.public_key());
        let changed_before = fs::read(&changed_path).unwrap();

        let changed = session_error(port, &changed_path).await;
        assert!(
            changed.starts_with(&format!("The key for 127.0.0.1:{port} does not match")),
            "{changed}"
        );
        assert!(changed.contains(&server_fingerprint), "{changed}");
        assert!(host_key::unknown_host_key("127.0.0.1", port).is_none());
        assert_eq!(fs::read(&changed_path).unwrap(), changed_before);

        let ecdsa_dir = tempfile::tempdir().unwrap();
        let ecdsa_path = ecdsa_dir.path().join("known_hosts");
        let ecdsa = PrivateKey::random(
            &mut rand::rng(),
            Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP256,
            },
        )
        .unwrap();
        write_host_line(&ecdsa_path, &entry, ecdsa.public_key());

        let ecdsa_only = session_error(port, &ecdsa_path).await;
        assert!(
            ecdsa_only.starts_with(&format!("The key for 127.0.0.1:{port} does not match")),
            "{ecdsa_only}"
        );
        assert_eq!(passwords_seen.load(Ordering::SeqCst), 1);
    }
}
