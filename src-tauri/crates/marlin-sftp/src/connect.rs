use russh::keys::PublicKey;
use russh::{client, ChannelId, ChannelMsg, ChannelStream};
use russh_sftp::client::SftpSession;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::host_key::{self, HostKeyRejection};
use crate::servers::{Passphrase, SftpServerCredentials, SignIn, NO_CREDENTIALS_TAG};

const CONNECT_TIMEOUT_SECS: u64 = 15;

/// How long the server has to answer the request for its SFTP subsystem.
const SUBSYSTEM_ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// Why a connection failed. Each text is what the user reads.
#[derive(Debug)]
pub enum ConnectError {
    /// `known_hosts` holds no key for the host. The caller may ask the user to trust this one.
    UnknownHostKey(PublicKey),
    /// `known_hosts` holds another key for the host. The text is `changed_host_key_warning`, which names a local path.
    ChangedHostKey(String),
    /// The server could not be reached: the 15-second connect limit ran out, the SSH connection failed, or the connection closed during sign-in before any refusal (`"SSH connection to … timed out after …s"`, `"SSH connection failed: …"`, `"The server closed the connection during sign-in"`). A server that closes at once on a wrong password, rather than saying no, lands here too: the two cannot be told apart.
    Unreachable(String),
    /// The server said no to the password, the key, or every agent key it was offered (`"Password authentication rejected by server"`, `"Key authentication rejected by server"`, `"No SSH agent key was accepted by the server"`). A server that closes the connection after refusing agent keys has said no too.
    SignInRejected(String),
    /// The key needs a passphrase this sign-in does not have: none was given, the keychain could not give the saved one, or, for a key in OpenSSH's format, the one given does not open it. The text starts with the `[SFTP_NO_CREDENTIALS]` tag that makes the app ask for it (`"… The SSH key needs its passphrase."`, `"… The passphrase does not open the SSH key."`, or the keychain error). A keychain entry that could not even be named gives an error with no tag.
    PassphraseNeeded(String),
    /// The sign-in cannot be used on this machine: the key file does not load, or the SSH agent cannot sign (`"Failed to load SSH key: …"`, `"Failed to connect to SSH agent: …"`, `"Failed to list SSH agent identities: …"`, `"No keys available in SSH agent"`, `"The SSH agent could not sign with a key: …"`).
    SignInUnusable(String),
    /// A known_hosts file that could not be read, an error returned by a sign-in call (`"Password authentication failed: …"`, `"Key authentication failed: …"`), or the SFTP channel, subsystem, and session start.
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
            | ConnectError::PassphraseNeeded(text)
            | ConnectError::SignInUnusable(text)
            | ConnectError::Failed(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for ConnectError {}

#[derive(Debug)]
pub(crate) enum HandshakeError {
    Ssh(russh::Error),
    HostKey(PublicKey, HostKeyRejection),
}

impl From<russh::Error> for HandshakeError {
    fn from(error: russh::Error) -> Self {
        HandshakeError::Ssh(error)
    }
}

/// SSH client handler that accepts only a server key saved in `known_hosts`.
pub(crate) struct SshHandler {
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

/// How one sign-in try ended.
enum Attempt {
    Accepted,
    /// The server said no, and the connection is still open.
    Refused,
    /// The connection closed before the server answered.
    Closed,
}

/// Sorts one sign-in answer. russh answers `Failure` when the connection closes before the server's reply, and a call on a closed connection fails, so only an open connection means the server said no. Any other error is the caller's to name.
fn attempt<E>(
    session: &client::Handle<SshHandler>,
    answer: Result<client::AuthResult, E>,
) -> Result<Attempt, E> {
    match answer {
        Ok(result) if result.success() => Ok(Attempt::Accepted),
        _ if session.is_closed() => Ok(Attempt::Closed),
        Ok(_) => Ok(Attempt::Refused),
        Err(error) => Err(error),
    }
}

/// The hash an RSA key signs with: the strongest `rsa-sha2` the server offers. With none, russh signs with SHA-1 (`ssh-rsa`), which OpenSSH 8.8 and later refuse. Asking can wait a second for the server's list, so only a sign-in with an RSA key asks. A failed ask means a closed connection, which the sign-in that follows reports.
async fn rsa_hash(session: &client::Handle<SshHandler>) -> Option<russh::keys::HashAlg> {
    session
        .best_supported_rsa_hash()
        .await
        .ok()
        .flatten()
        .flatten()
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
    let (_session, stream) =
        open_sftp_channel(hostname, port, creds, known_hosts, saved_host_keys).await?;
    SftpSession::new(stream)
        .await
        .map_err(|e| ConnectError::Failed(format!("Failed to initialize SFTP session: {}", e)))
}

/// Signs in and opens the SFTP channel. The connection lasts while the channel's stream or the session's handle is held.
pub(crate) async fn open_sftp_channel(
    hostname: &str,
    port: u16,
    creds: &SftpServerCredentials,
    known_hosts: &Path,
    saved_host_keys: Vec<PublicKey>,
) -> Result<(client::Handle<SshHandler>, ChannelStream<client::Msg>), ConnectError> {
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

    // How the sign-in ended, and the text the server's "no" gets.
    let (outcome, refusal) = match &creds.sign_in {
        SignIn::Password(password) => {
            let answer = session
                .authenticate_password(&creds.username, password)
                .await;
            let outcome = attempt(&session, answer).map_err(|e| {
                ConnectError::Failed(format!("Password authentication failed: {}", e))
            })?;
            (outcome, "Password authentication rejected by server")
        }
        SignIn::Key { path, passphrase } => {
            let expanded = if path.starts_with('~') {
                if let Some(home) = dirs::home_dir() {
                    home.join(path[1..].trim_start_matches('/'))
                } else {
                    std::path::PathBuf::from(path)
                }
            } else {
                std::path::PathBuf::from(path)
            };

            let given = match passphrase {
                Passphrase::Given(text) => Some(text.as_str()),
                Passphrase::None | Passphrase::Unread(_) => None,
            };
            let private_key = match russh::keys::load_secret_key(&expanded, given) {
                Ok(key) => key,
                Err(error) => {
                    return Err(match (&error, passphrase) {
                        (russh::keys::Error::KeyIsEncrypted, Passphrase::Unread(keychain)) => {
                            ConnectError::PassphraseNeeded(keychain.clone())
                        }
                        (russh::keys::Error::KeyIsEncrypted, _) => ConnectError::PassphraseNeeded(
                            format!("{} The SSH key needs its passphrase.", NO_CREDENTIALS_TAG),
                        ),
                        // OpenSSH's format checks the passphrase it decrypts with, so a wrong one fails this way.
                        (
                            russh::keys::Error::SshKey(russh::keys::ssh_key::Error::Crypto),
                            Passphrase::Given(_),
                        ) => ConnectError::PassphraseNeeded(format!(
                            "{} The passphrase does not open the SSH key.",
                            NO_CREDENTIALS_TAG
                        )),
                        _ => ConnectError::SignInUnusable(format!(
                            "Failed to load SSH key: {}",
                            error
                        )),
                    });
                }
            };

            let hash = if private_key.algorithm().is_rsa() {
                rsa_hash(&session).await
            } else {
                None
            };
            let key_with_alg = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(private_key), hash);

            let answer = session
                .authenticate_publickey(&creds.username, key_with_alg)
                .await;
            let outcome = attempt(&session, answer)
                .map_err(|e| ConnectError::Failed(format!("Key authentication failed: {}", e)))?;
            (outcome, "Key authentication rejected by server")
        }
        SignIn::Agent => {
            let mut agent = russh::keys::agent::client::AgentClient::connect_env()
                .await
                .map_err(|e| {
                    ConnectError::SignInUnusable(format!("Failed to connect to SSH agent: {}", e))
                })?;

            let identities = agent.request_identities().await.map_err(|e| {
                ConnectError::SignInUnusable(format!("Failed to list SSH agent identities: {}", e))
            })?;

            if identities.is_empty() {
                return Err(ConnectError::SignInUnusable(
                    "No keys available in SSH agent".to_string(),
                ));
            }

            let holds_rsa = identities.iter().any(|identity| match identity {
                russh::keys::agent::AgentIdentity::PublicKey { key, .. } => {
                    key.algorithm().is_rsa()
                }
                russh::keys::agent::AgentIdentity::Certificate { certificate, .. } => {
                    certificate.algorithm().is_rsa()
                }
            });
            let hash = if holds_rsa {
                rsa_hash(&session).await
            } else {
                None
            };

            let outcome = 'offers: {
                for (offered_before, identity) in identities.iter().enumerate() {
                    let answer = match identity {
                        russh::keys::agent::AgentIdentity::PublicKey { key, .. } => {
                            session
                                .authenticate_publickey_with(
                                    &creds.username,
                                    key.clone(),
                                    hash,
                                    &mut agent,
                                )
                                .await
                        }
                        russh::keys::agent::AgentIdentity::Certificate { certificate, .. } => {
                            session
                                .authenticate_certificate_with(
                                    &creds.username,
                                    certificate.clone(),
                                    hash,
                                    &mut agent,
                                )
                                .await
                        }
                    };
                    // An error on an open connection is the agent failing to sign, as when the user declines its prompt. That is not the server saying no, and russh then waits for a signature that never comes, so no other key can be tried.
                    match attempt(&session, answer).map_err(|e| {
                        ConnectError::SignInUnusable(format!(
                            "The SSH agent could not sign with a key: {}",
                            e
                        ))
                    })? {
                        Attempt::Refused => continue,
                        // A server can close the connection after too many refused keys (OpenSSH's `MaxAuthTries`). That is still its "no".
                        Attempt::Closed if offered_before > 0 => break 'offers Attempt::Refused,
                        done => break 'offers done,
                    }
                }
                Attempt::Refused
            };
            (outcome, "No SSH agent key was accepted by the server")
        }
    };

    match outcome {
        Attempt::Accepted => {}
        Attempt::Refused => return Err(ConnectError::SignInRejected(refusal.to_string())),
        Attempt::Closed => {
            return Err(ConnectError::Unreachable(
                "The server closed the connection during sign-in".to_string(),
            ));
        }
    }

    let mut channel = session
        .channel_open_session()
        .await
        .map_err(|e| ConnectError::Failed(format!("Failed to open SSH channel: {}", e)))?;

    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| ConnectError::Failed(format!("Failed to request SFTP subsystem: {}", e)))?;

    // The request only goes out, and the stream drops the server's answer, so it is read here: a server with SFTP turned off says no at once. A server that never answers gets the 10 seconds that the app's first SFTP request gives it.
    let answer = tokio::time::timeout(SUBSYSTEM_ANSWER_TIMEOUT, async {
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Success) => return Ok(()),
                Some(ChannelMsg::Failure) => return Err("The server refused the SFTP subsystem"),
                Some(_) => {}
                None => return Err("The server closed the SSH channel before SFTP started"),
            }
        }
    })
    .await
    .unwrap_or(Err("The server did not answer the request for SFTP"));
    answer.map_err(|text| ConnectError::Failed(text.to_string()))?;
    Ok((session, channel.into_stream()))
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
            sign_in: SignIn::Password("secret".to_string()),
        }
    }

    fn key_creds(path: &Path, passphrase: Passphrase) -> SftpServerCredentials {
        SftpServerCredentials {
            username: "user".to_string(),
            sign_in: SignIn::Key {
                path: path.to_string_lossy().into_owned(),
                passphrase,
            },
        }
    }

    /// The keychain error a key server gets when its passphrase cannot be read.
    const UNREAD_KEYCHAIN: &str =
        "[SFTP_NO_CREDENTIALS] Failed to read the saved secret from the keychain: test keychain read failed";

    async fn key_error(creds: &SftpServerCredentials) -> ConnectError {
        let (port, _dir, known_hosts) = trusted(RejectServer {
            passwords_seen: Arc::new(AtomicUsize::new(0)),
        })
        .await;
        match connect("127.0.0.1", port, creds, &known_hosts).await {
            Ok(_) => panic!("expected the connection to fail"),
            Err(error) => error,
        }
    }

    async fn start_server<S: Server + Send + 'static>(server: S) -> (u16, PublicKey) {
        let host_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let public_key = host_key.public_key().clone();
        let mut config = russh::server::Config::default();
        config.auth_rejection_time = Duration::from_millis(0);
        config.auth_rejection_time_initial = Some(Duration::from_millis(0));
        config.inactivity_timeout = None;
        config.keys.push(host_key);
        let config = Arc::new(config);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut server = server;
            let _ = server.run_on_socket(config, &listener).await;
        });
        (port, public_key)
    }

    async fn trusted<S: Server + Send + 'static>(
        server: S,
    ) -> (u16, tempfile::TempDir, std::path::PathBuf) {
        let (port, public_key) = start_server(server).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        write_host_line(&path, &format!("[127.0.0.1]:{port}"), &public_key);
        (port, dir, path)
    }

    /// Completes the handshake and never answers a password: the handler returns an error, which closes the connection.
    struct ClosesOnPassword;

    impl Handler for ClosesOnPassword {
        type Error = russh::Error;

        async fn auth_password(
            &mut self,
            _user: &str,
            _password: &str,
        ) -> Result<Auth, Self::Error> {
            Err(russh::Error::Disconnect)
        }
    }

    impl Server for ClosesOnPassword {
        type Handler = Self;

        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
            ClosesOnPassword
        }
    }

    /// Takes any password and opens the session, but has no SFTP: it says no to every subsystem, or never answers, and keeps the connection.
    struct NoSftp {
        says_no: bool,
        channels: Vec<russh::Channel<russh::server::Msg>>,
    }

    impl Handler for NoSftp {
        type Error = russh::Error;

        async fn auth_password(
            &mut self,
            _user: &str,
            _password: &str,
        ) -> Result<Auth, Self::Error> {
            Ok(Auth::Accept)
        }

        async fn channel_open_session(
            &mut self,
            channel: russh::Channel<russh::server::Msg>,
            _session: &mut russh::server::Session,
        ) -> Result<bool, Self::Error> {
            self.channels.push(channel);
            Ok(true)
        }

        async fn subsystem_request(
            &mut self,
            channel: ChannelId,
            _name: &str,
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            if self.says_no {
                session.channel_failure(channel)?;
            }
            Ok(())
        }
    }

    impl Server for NoSftp {
        type Handler = Self;

        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
            NoSftp {
                says_no: self.says_no,
                channels: Vec::new(),
            }
        }
    }

    /// The error `connect` returns from a server with no SFTP, within 20 seconds.
    async fn no_sftp_error(says_no: bool) -> String {
        let server = NoSftp {
            says_no,
            channels: Vec::new(),
        };
        let (port, _dir, known_hosts) = trusted(server).await;
        let connected = tokio::time::timeout(
            Duration::from_secs(20),
            connect("127.0.0.1", port, &password_creds(), &known_hosts),
        )
        .await
        .expect("the connect waited past its limit for the subsystem");
        match connected {
            Err(ConnectError::Failed(text)) => text,
            Err(other) => panic!("expected a failed start, got {other:?}"),
            Ok(_) => panic!("expected the connection to fail"),
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
        let passwords_seen = Arc::new(AtomicUsize::new(0));
        let (port, server_public) = start_server(RejectServer {
            passwords_seen: passwords_seen.clone(),
        })
        .await;
        let server_fingerprint = server_public.fingerprint(HashAlg::Sha256).to_string();

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

    #[tokio::test]
    async fn an_encrypted_key_without_a_working_passphrase_asks_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id_ed25519");
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
            .unwrap()
            .encrypt(&mut rand::rng(), "correct-horse")
            .unwrap();
        fs::write(
            &path,
            key.to_openssh(russh::keys::ssh_key::LineEnding::LF)
                .unwrap()
                .as_bytes(),
        )
        .unwrap();

        for (passphrase, expected) in [
            (
                Passphrase::Unread(UNREAD_KEYCHAIN.to_string()),
                UNREAD_KEYCHAIN,
            ),
            (
                Passphrase::None,
                "[SFTP_NO_CREDENTIALS] The SSH key needs its passphrase.",
            ),
            (
                Passphrase::Given("wrong-horse".to_string()),
                "[SFTP_NO_CREDENTIALS] The passphrase does not open the SSH key.",
            ),
        ] {
            match key_error(&key_creds(&path, passphrase)).await {
                ConnectError::PassphraseNeeded(text) => assert_eq!(text, expected),
                other => panic!("expected a request for the passphrase, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_missing_key_file_says_the_key_did_not_load_whatever_the_keychain_said() {
        let missing = Path::new("/no/such/marlin-sftp-key");
        for passphrase in [
            Passphrase::None,
            Passphrase::Unread(UNREAD_KEYCHAIN.to_string()),
        ] {
            match key_error(&key_creds(missing, passphrase)).await {
                ConnectError::SignInUnusable(text) => {
                    assert!(text.starts_with("Failed to load SSH key"), "{text}");
                }
                other => panic!("expected an unusable sign-in, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_server_that_closes_during_sign_in_is_unreachable() {
        let (port, _dir, known_hosts) = trusted(ClosesOnPassword).await;
        let error = match connect("127.0.0.1", port, &password_creds(), &known_hosts).await {
            Ok(_) => panic!("expected the connection to fail"),
            Err(error) => error,
        };
        match error {
            ConnectError::Unreachable(text) => {
                assert_eq!(text, "The server closed the connection during sign-in");
            }
            other => panic!("expected an unreachable server, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_server_that_refuses_sftp_fails_at_once() {
        let started = std::time::Instant::now();
        assert_eq!(
            no_sftp_error(true).await,
            "The server refused the SFTP subsystem"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn a_server_that_never_answers_for_sftp_fails_after_its_limit() {
        assert_eq!(
            no_sftp_error(false).await,
            "The server did not answer the request for SFTP"
        );
    }
}
