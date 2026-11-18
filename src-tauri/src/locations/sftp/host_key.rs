use once_cell::sync::Lazy;
use russh::keys::{HashAlg, PublicKey};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

/// An unknown server key that waits for the user's answer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnknownHostKey {
    pub key_type: String,
    pub fingerprint: String,
    pub known_hosts_path: String,
    /// Never sent to the frontend.
    #[serde(skip)]
    pub key: PublicKey,
}

/// The last unknown key each server sent, by lower-case host name and port. Only the app asks the user to trust a key, so the app holds it.
static UNKNOWN_HOST_KEYS: Lazy<Mutex<HashMap<(String, u16), UnknownHostKey>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Keeps an unknown server key until the user answers, and returns what the user is shown.
pub fn remember_unknown_host_key(
    hostname: &str,
    port: u16,
    key: PublicKey,
    known_hosts: &Path,
) -> UnknownHostKey {
    let unknown = UnknownHostKey {
        key_type: key.algorithm().as_str().to_string(),
        fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
        known_hosts_path: known_hosts.display().to_string(),
        key,
    };
    UNKNOWN_HOST_KEYS
        .lock()
        .unwrap()
        .insert((hostname.to_lowercase(), port), unknown.clone());
    unknown
}

/// The unknown key this server last sent, until the user trusts it.
pub fn unknown_host_key(hostname: &str, port: u16) -> Option<UnknownHostKey> {
    let server = (hostname.to_lowercase(), port);
    UNKNOWN_HOST_KEYS.lock().unwrap().get(&server).cloned()
}

/// Saves the unknown key this server last sent, when `fingerprint` is the one the user was shown.
pub fn trust_unknown_host_key(
    hostname: &str,
    port: u16,
    fingerprint: &str,
    known_hosts: &Path,
) -> Result<(), String> {
    let mut unknown_keys = UNKNOWN_HOST_KEYS.lock().unwrap();
    let server = (hostname.to_lowercase(), port);
    let Some(unknown) = unknown_keys.get(&server) else {
        return Err(format!(
            "No unknown host key is waiting for {hostname}:{port}."
        ));
    };
    if unknown.fingerprint != fingerprint {
        return Err(
            "The host key fingerprint does not match the server Marlin just saw.".to_string(),
        );
    }
    marlin_sftp::trust_host_key(hostname, port, &unknown.key, known_hosts)?;
    unknown_keys.remove(&server);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::parse_public_key_base64;

    const ED25519_A: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";

    fn key(b64: &str) -> PublicKey {
        parse_public_key_base64(b64).unwrap()
    }

    #[test]
    fn an_unknown_key_reaches_the_frontend_without_the_key_itself() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let unknown = remember_unknown_host_key("Shown.Example", 2200, key(ED25519_A), &path);

        assert_eq!(
            serde_json::to_value(&unknown).unwrap(),
            serde_json::json!({
                "keyType": "ssh-ed25519",
                "fingerprint": key(ED25519_A).fingerprint(HashAlg::Sha256).to_string(),
                "knownHostsPath": path.display().to_string(),
            })
        );
    }

    #[test]
    fn only_the_remembered_key_is_saved_and_only_for_the_fingerprint_shown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        assert!(trust_unknown_host_key("waiting.example", 22, "SHA256:any", &path).is_err());

        let unknown = remember_unknown_host_key("Waiting.Example", 22, key(ED25519_A), &path);
        assert!(trust_unknown_host_key("waiting.example", 22, "SHA256:other", &path).is_err());
        assert!(!path.exists());

        trust_unknown_host_key("waiting.example", 22, &unknown.fingerprint, &path).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            saved
                .lines()
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>(),
            [format!(
                "waiting.example {}",
                key(ED25519_A).to_openssh().unwrap()
            )]
        );
        assert!(unknown_host_key("waiting.example", 22).is_none());
    }
}
