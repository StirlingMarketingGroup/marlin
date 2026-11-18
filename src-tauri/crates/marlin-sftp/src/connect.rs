use russh::keys::PublicKey;
use russh::{client, ChannelId};
use russh_sftp::client::SftpSession;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::host_key::{self, HostKeyRejection};
use crate::servers::SftpServerCredentials;

const CONNECT_TIMEOUT_SECS: u64 = 15;

/// Why a connection failed. Each text is what the user reads.
#[derive(Debug)]
pub enum ConnectError {
    /// `known_hosts` holds no key for the host. The caller may ask the user to trust this one.
    UnknownHostKey(PublicKey),
    /// `known_hosts` holds another key for the host. The text is `changed_host_key_warning`, which names a local path.
    ChangedHostKey(String),
    /// The server could not be reached: the 15-second connect limit ran out, or the SSH connection failed (`"SSH connection to … timed out after …s"`, `"SSH connection failed: …"`).
    Unreachable(String),
    /// The server said no to the password, the key, or every agent key (`"Password authentication rejected by server"`, `"Key authentication rejected by server"`, `"No SSH agent key was accepted by the server"`).
    SignInRejected(String),
    /// Any other failure, as the text the user reads: a known_hosts file that could not be read, a key file or passphrase that would not load, no agent, no password for password sign-in, an unknown auth method, an error during a sign-in call, or the SFTP channel.
    Failed(String),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::UnknownHostKey(_) => {
                f.write_str("The server's host key is not in known_hosts.")
            }
            ConnectError::ChangedHostKey(text)
            | ConnectError::Unreachable(text)
            | ConnectError::SignInRejected(text)
            | ConnectError::Failed(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for ConnectError {}

#[derive(Debug)]
enum HandshakeError {
    Ssh(russh::Error),
    HostKey(PublicKey, HostKeyRejection),
}

impl From<russh::Error> for HandshakeError {
    fn from(error: russh::Error) -> Self {
        HandshakeError::Ssh(error)
    }
}

/// SSH client handler that accepts only a server key saved in `known_hosts`.
struct SshHandler {
    saved_host_keys: Vec<PublicKey>,
}

impl client::Handler for SshHandler {
    type Error = HandshakeError;

    fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> impl std::future::Future<Output = Result<bool, Self::Error>> + Send {
        let checked = host_key::check_host_key(server_public_key, &self.saved_host_keys)
            .map(|()| true)
            .map_err(|rejection| HandshakeError::HostKey(server_public_key.clone(), rejection));
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

/// Open an SFTP session. An unknown host key is returned, not remembered.
pub async fn connect(
    hostname: &str,
    port: u16,
    creds: &SftpServerCredentials,
    known_hosts: &Path,
) -> Result<SftpSession, ConnectError> {
    let saved_host_keys =
        host_key::saved_host_keys(hostname, port, known_hosts).map_err(ConnectError::Failed)?;
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
        ConnectError::Unreachable(format!(
            "SSH connection to {}:{} timed out after {}s",
            hostname, port, CONNECT_TIMEOUT_SECS
        ))
    })?;

    let mut session = match connect_result {
        Ok(session) => session,
        Err(HandshakeError::HostKey(key, HostKeyRejection::Unknown)) => {
            return Err(ConnectError::UnknownHostKey(key));
        }
        Err(HandshakeError::HostKey(key, HostKeyRejection::Changed { saved_fingerprint })) => {
            return Err(ConnectError::ChangedHostKey(
                host_key::changed_host_key_warning(
                    hostname,
                    port,
                    &key,
                    &saved_fingerprint,
                    known_hosts,
                ),
            ));
        }
        Err(HandshakeError::Ssh(error)) => {
            return Err(ConnectError::Unreachable(format!(
                "SSH connection failed: {}",
                error
            )));
        }
    };

    match creds.auth_method.as_str() {
        "password" => {
            let password = creds.password.as_deref().ok_or_else(|| {
                ConnectError::Failed("Password auth selected but no password available".to_string())
            })?;
            let auth_result = session
                .authenticate_password(&creds.username, password)
                .await
                .map_err(|e| {
                    ConnectError::Failed(format!("Password authentication failed: {}", e))
                })?;
            if !auth_result.success() {
                return Err(ConnectError::SignInRejected(
                    "Password authentication rejected by server".to_string(),
                ));
            }
        }
        "key" => {
            let key_path_str = creds.key_path.as_deref().ok_or_else(|| {
                ConnectError::Failed("Key auth selected but no key path available".to_string())
            })?;

            let expanded = if key_path_str.starts_with('~') {
                if let Some(home) = dirs::home_dir() {
                    home.join(&key_path_str[1..].trim_start_matches('/'))
                } else {
                    std::path::PathBuf::from(key_path_str)
                }
            } else {
                std::path::PathBuf::from(key_path_str)
            };

            let private_key = russh::keys::load_secret_key(&expanded, creds.password.as_deref())
                .map_err(|e| ConnectError::Failed(format!("Failed to load SSH key: {}", e)))?;

            let key_with_alg = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(private_key), None);

            let auth_result = session
                .authenticate_publickey(&creds.username, key_with_alg)
                .await
                .map_err(|e| ConnectError::Failed(format!("Key authentication failed: {}", e)))?;
            if !auth_result.success() {
                return Err(ConnectError::SignInRejected(
                    "Key authentication rejected by server".to_string(),
                ));
            }
        }
        "agent" => 'agent: {
            let mut agent = russh::keys::agent::client::AgentClient::connect_env()
                .await
                .map_err(|e| {
                    ConnectError::Failed(format!("Failed to connect to SSH agent: {}", e))
                })?;

            let identities = agent.request_identities().await.map_err(|e| {
                ConnectError::Failed(format!("Failed to list SSH agent identities: {}", e))
            })?;

            if identities.is_empty() {
                return Err(ConnectError::Failed(
                    "No keys available in SSH agent".to_string(),
                ));
            }

            // An agent can fail to sign, as when the user refuses its prompt. That is not the server saying no.
            let mut signing_failed = false;
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
                    Ok(result) if result.success() => break 'agent,
                    Ok(_) => {}
                    Err(_) => signing_failed = true,
                }
            }

            let text = "No SSH agent key was accepted by the server".to_string();
            return Err(if signing_failed {
                ConnectError::Failed(text)
            } else {
                ConnectError::SignInRejected(text)
            });
        }
        other => {
            return Err(ConnectError::Failed(format!(
                "Unknown auth method: {}",
                other
            )));
        }
    }

    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| ConnectError::Failed(format!("Failed to open SSH channel: {}", e)))?;

    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| ConnectError::Failed(format!("Failed to request SFTP subsystem: {}", e)))?;

    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| ConnectError::Failed(format!("Failed to initialize SFTP session: {}", e)))?;

    Ok(sftp)
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

    fn password_creds() -> SftpServerCredentials {
        SftpServerCredentials {
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

    async fn session_error(port: u16, known_hosts: &Path) -> ConnectError {
        match connect("127.0.0.1", port, &password_creds(), known_hosts).await {
            Ok(_) => panic!("expected the connection to fail"),
            Err(error) => error,
        }
    }

    #[tokio::test]
    async fn connect_checks_the_server_key_against_known_hosts() {
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

        let unknown_key = match session_error(port, &unknown_path).await {
            ConnectError::UnknownHostKey(key) => key,
            other => panic!("expected an unknown host key, got {other:?}"),
        };
        assert_eq!(
            unknown_key.fingerprint(HashAlg::Sha256).to_string(),
            server_fingerprint
        );
        assert_eq!(passwords_seen.load(Ordering::SeqCst), 0);

        host_key::trust_host_key("127.0.0.1", port, &unknown_key, &unknown_path).unwrap();
        match session_error(port, &unknown_path).await {
            ConnectError::SignInRejected(text) => {
                assert_eq!(text, "Password authentication rejected by server");
            }
            other => panic!("expected a rejected password, got {other:?}"),
        }
        assert_eq!(passwords_seen.load(Ordering::SeqCst), 1);

        let changed_dir = tempfile::tempdir().unwrap();
        let changed_path = changed_dir.path().join("known_hosts");
        let other_ed25519 = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let entry = format!("[127.0.0.1]:{port}");
        write_host_line(&changed_path, &entry, other_ed25519.public_key());
        let changed_before = fs::read(&changed_path).unwrap();

        match session_error(port, &changed_path).await {
            ConnectError::ChangedHostKey(text) => {
                assert!(
                    text.starts_with(&format!("The key for 127.0.0.1:{port} does not match")),
                    "{text}"
                );
                assert!(text.contains(&server_fingerprint), "{text}");
            }
            other => panic!("expected a changed host key, got {other:?}"),
        }
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

        match session_error(port, &ecdsa_path).await {
            ConnectError::ChangedHostKey(text) => {
                assert!(
                    text.starts_with(&format!("The key for 127.0.0.1:{port} does not match")),
                    "{text}"
                );
            }
            other => panic!("expected a changed host key, got {other:?}"),
        }
        assert_eq!(passwords_seen.load(Ordering::SeqCst), 1);
    }
}
