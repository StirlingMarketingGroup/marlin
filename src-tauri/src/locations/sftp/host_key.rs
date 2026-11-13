use once_cell::sync::Lazy;
use russh::keys::known_hosts::{known_host_keys_path, learn_known_hosts_path};
use russh::keys::{Algorithm, HashAlg, PublicKey};
use serde::Serialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Why a server key was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyRejection {
    /// `known_hosts` holds no key for the host. The user may trust this one.
    Unknown,
    /// `known_hosts` holds a key for the host, and it is not this one. The app has no override.
    Changed { saved_fingerprint: String },
}

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

/// The last unknown key each server sent, by lower-case host name and port.
static UNKNOWN_HOST_KEYS: Lazy<Mutex<HashMap<(String, u16), UnknownHostKey>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub fn known_hosts_path() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|home| home.join(".ssh").join("known_hosts"))
        .ok_or_else(|| "Could not determine home directory".to_string())
}

/// The keys `known_hosts` holds for this host. A missing file holds none.
pub fn saved_host_keys(
    hostname: &str,
    port: u16,
    known_hosts: &Path,
) -> Result<Vec<PublicKey>, String> {
    // russh reads a file it cannot open as an empty one, which would make a saved host look new.
    if let Err(e) = File::open(known_hosts) {
        if e.kind() != ErrorKind::NotFound {
            return Err(format!("Failed to read {}: {}", known_hosts.display(), e));
        }
    }
    // OpenSSH saves host names in lower case.
    let saved = known_host_keys_path(&hostname.to_lowercase(), port, known_hosts)
        .map_err(|e| format!("Failed to read {}: {}", known_hosts.display(), e))?;
    // The line numbers that come with the keys are wrong: russh does not count comment lines.
    Ok(saved.into_iter().map(|(_, key)| key).collect())
}

/// Accepts `key` only when it equals one of the keys saved for its host.
pub fn check_host_key(key: &PublicKey, saved: &[PublicKey]) -> Result<(), HostKeyRejection> {
    if saved.iter().any(|s| s.key_data() == key.key_data()) {
        return Ok(());
    }
    let Some(first) = saved.first() else {
        return Err(HostKeyRejection::Unknown);
    };
    let shown = saved
        .iter()
        .find(|s| s.algorithm() == key.algorithm())
        .unwrap_or(first);
    Err(HostKeyRejection::Changed {
        saved_fingerprint: shown.fingerprint(HashAlg::Sha256).to_string(),
    })
}

/// Appends `key` for a host that has no saved key. Never writes beside a saved key.
pub fn trust_host_key(
    hostname: &str,
    port: u16,
    key: &PublicKey,
    known_hosts: &Path,
) -> Result<(), String> {
    let saved = saved_host_keys(hostname, port, known_hosts)?;
    match check_host_key(key, &saved) {
        Ok(()) => Ok(()),
        Err(HostKeyRejection::Changed { saved_fingerprint }) => Err(changed_host_key_warning(
            hostname,
            port,
            key,
            &saved_fingerprint,
            known_hosts,
        )),
        Err(HostKeyRejection::Unknown) => {
            // OpenSSH keeps `~/.ssh` private. russh would create it readable by every user.
            #[cfg(unix)]
            if let Some(dir) = known_hosts.parent().filter(|dir| !dir.exists()) {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(dir)
                    .map_err(|e| format!("Failed to create {}: {}", dir.display(), e))?;
            }
            learn_known_hosts_path(&hostname.to_lowercase(), port, key, known_hosts)
                .map_err(|e| format!("Failed to write {}: {}", known_hosts.display(), e))
        }
    }
}

/// What the user reads when a server's key does not match the saved one.
pub fn changed_host_key_warning(
    hostname: &str,
    port: u16,
    key: &PublicKey,
    saved_fingerprint: &str,
    known_hosts: &Path,
) -> String {
    let host = hostname.to_lowercase();
    let (shown, entry) = if port == 22 {
        (hostname.to_string(), host)
    } else {
        (format!("{hostname}:{port}"), format!("[{host}]:{port}"))
    };
    format!(
        "The key for {shown} does not match the one saved in {}.\n\n\
         Someone may be intercepting the connection, or the server's key was replaced.\n\n\
         Key the server sent ({}): {}\n\
         Saved key: {saved_fingerprint}\n\n\
         Marlin did not connect. If the server's owner confirms the new key, remove the saved \
         one with this command, then connect again:\n\
         ssh-keygen -R \"{entry}\"",
        known_hosts.display(),
        key.algorithm().as_str(),
        key.fingerprint(HashAlg::Sha256),
    )
}

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
    trust_host_key(hostname, port, &unknown.key, known_hosts)?;
    unknown_keys.remove(&server);
    Ok(())
}

/// The default host key algorithms, with the types saved for the host first.
///
/// The SSH handshake picks the first host key type on the client's list that the server also
/// has, so a host saved with an ECDSA key must be asked for ECDSA first, or its key looks changed.
pub fn preferred_host_key_algorithms(saved: &[PublicKey]) -> Vec<Algorithm> {
    let (mut saved_types, other_types): (Vec<Algorithm>, Vec<Algorithm>) =
        russh::Preferred::DEFAULT
            .key
            .iter()
            .cloned()
            .partition(|algorithm| {
                saved.iter().any(|s| {
                    *algorithm == s.algorithm()
                        || (matches!(algorithm, Algorithm::Rsa { .. })
                            && matches!(s.algorithm(), Algorithm::Rsa { .. }))
                })
            });
    saved_types.extend(other_types);
    saved_types
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::parse_public_key_base64;
    use std::fs;

    const ED25519_A: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const ED25519_B: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIA6rWI3G1sz07DnfFlrouTcysQlj2P+jpNSOEWD9OJ3X";
    const ECDSA_P256: &str = "AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBHwf2HMM5TRXvo2SQJjsNkiDD5KqiiNjrGVv3UUh+mMT5RHxiRtOnlqvjhQtBq0VpmpCV/PwUdhOig4vkbqAcEc=";

    fn key(b64: &str) -> PublicKey {
        parse_public_key_base64(b64).unwrap()
    }

    fn changed_from(saved: &PublicKey) -> HostKeyRejection {
        HostKeyRejection::Changed {
            saved_fingerprint: saved.fingerprint(HashAlg::Sha256).to_string(),
        }
    }

    fn non_empty_lines(path: &Path) -> Vec<String> {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn a_host_with_no_saved_key_is_unknown() {
        assert_eq!(
            check_host_key(&key(ED25519_A), &[]),
            Err(HostKeyRejection::Unknown)
        );
    }

    #[test]
    fn a_saved_key_is_accepted_wherever_it_is_in_the_list() {
        let saved = [key(ED25519_A), key(ED25519_B)];
        assert_eq!(check_host_key(&key(ED25519_B), &saved), Ok(()));
    }

    #[test]
    fn a_different_key_of_the_same_type_is_changed() {
        let saved = [key(ECDSA_P256), key(ED25519_A)];
        assert_eq!(
            check_host_key(&key(ED25519_B), &saved),
            Err(changed_from(&saved[1]))
        );
    }

    #[test]
    fn a_saved_key_of_another_type_only_is_changed() {
        let saved = [key(ECDSA_P256)];
        assert_eq!(
            check_host_key(&key(ED25519_A), &saved),
            Err(changed_from(&saved[0]))
        );
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
        assert_eq!(
            saved_host_keys("waiting.example", 22, &path).unwrap(),
            [key(ED25519_A)]
        );
        assert!(unknown_host_key("waiting.example", 22).is_none());
    }

    #[test]
    fn the_changed_key_warning_shows_both_fingerprints_and_how_to_remove_the_saved_key() {
        let warning = changed_host_key_warning(
            "NAS.local",
            2222,
            &key(ED25519_B),
            "SHA256:saved",
            Path::new("/home/me/.ssh/known_hosts"),
        );

        assert!(warning.starts_with(
            "The key for NAS.local:2222 does not match the one saved in /home/me/.ssh/known_hosts."
        ));
        assert!(warning.contains(&key(ED25519_B).fingerprint(HashAlg::Sha256).to_string()));
        assert!(warning.contains("Saved key: SHA256:saved"));
        assert!(warning.ends_with("ssh-keygen -R \"[nas.local]:2222\""));
    }

    #[test]
    fn a_missing_file_and_a_file_without_the_host_hold_no_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        assert!(saved_host_keys("example.com", 22, &path)
            .unwrap()
            .is_empty());
        assert!(!path.exists());

        trust_host_key("other.example", 22, &key(ED25519_A), &path).unwrap();
        assert!(saved_host_keys("example.com", 22, &path)
            .unwrap()
            .is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_file_that_cannot_be_opened_is_an_error_not_an_empty_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        trust_host_key("example.com", 22, &key(ED25519_A), &path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        if File::open(&path).is_ok() {
            return; // root can open anything, so the case cannot be staged
        }

        let error = saved_host_keys("example.com", 22, &path).unwrap_err();
        assert!(error.starts_with("Failed to read "), "{error}");
    }

    #[test]
    fn saved_keys_skip_comments_and_other_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let body = format!(
            "# comment above the host\nexample.com {}\nother.example {}\nexample.com {}\n",
            key(ED25519_A).to_openssh().unwrap(),
            key(ECDSA_P256).to_openssh().unwrap(),
            key(ED25519_B).to_openssh().unwrap()
        );
        fs::write(&path, body).unwrap();

        let saved = saved_host_keys("example.com", 22, &path).unwrap();
        assert_eq!(saved, [key(ED25519_A), key(ED25519_B)]);
    }

    #[test]
    fn trusting_a_key_saves_one_line_however_often_it_is_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let server_key = key(ED25519_A);

        trust_host_key("example.com", 22, &server_key, &path).unwrap();
        trust_host_key("example.com", 22, &server_key, &path).unwrap();

        assert_eq!(non_empty_lines(&path).len(), 1);
        assert_eq!(
            saved_host_keys("example.com", 22, &path).unwrap(),
            [server_key]
        );
    }

    #[test]
    fn a_key_is_never_saved_beside_a_different_saved_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        trust_host_key("example.com", 22, &key(ED25519_A), &path).unwrap();
        let before = fs::read(&path).unwrap();

        assert!(trust_host_key("example.com", 22, &key(ED25519_B), &path).is_err());
        assert!(trust_host_key("example.com", 22, &key(ECDSA_P256), &path).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn a_port_other_than_22_is_saved_in_brackets_and_is_a_separate_host() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let server_key = key(ED25519_A);
        trust_host_key("example.com", 2222, &server_key, &path).unwrap();

        assert!(non_empty_lines(&path)[0].starts_with("[example.com]:2222 "));
        assert_eq!(
            saved_host_keys("example.com", 2222, &path).unwrap(),
            [server_key]
        );
        assert!(saved_host_keys("example.com", 22, &path)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_host_name_is_saved_and_looked_up_in_lower_case() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let server_key = key(ED25519_A);
        trust_host_key("Example.COM", 22, &server_key, &path).unwrap();

        assert!(non_empty_lines(&path)[0].starts_with("example.com "));
        assert_eq!(
            saved_host_keys("EXAMPLE.com", 22, &path).unwrap(),
            [server_key]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_ssh_folder_is_created_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let ssh_dir = dir.path().join(".ssh");
        trust_host_key(
            "example.com",
            22,
            &key(ED25519_A),
            &ssh_dir.join("known_hosts"),
        )
        .unwrap();

        let mode = fs::metadata(&ssh_dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn preferred_algorithms_put_a_saved_type_first_without_dropping_any() {
        let saved = [key(ECDSA_P256)];
        let algorithms = preferred_host_key_algorithms(&saved);
        let defaults: Vec<Algorithm> = russh::Preferred::DEFAULT.key.iter().cloned().collect();

        assert_eq!(algorithms.first(), Some(&saved[0].algorithm()));
        assert_eq!(algorithms.len(), defaults.len());
        for algorithm in defaults {
            assert_eq!(
                algorithms.iter().filter(|item| **item == algorithm).count(),
                1
            );
        }
    }
}
