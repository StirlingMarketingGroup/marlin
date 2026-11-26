//! In-memory stand-in for the OS keychain, shared by every test in this process.
//! There is only one: the keyring builder is process-global, and a second store would split each module's entries.
//! The test builds of the app and of the mount helper can keep it in a file instead, which both processes read: see [`install_for_app_test`].

use keyring::credential::{CredentialApi, CredentialBuilderApi};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

static STORE: Lazy<Mutex<HashMap<(String, String), String>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// `None` keeps the store in memory. A path is the file the test app and its mount helper share.
static FILE: Lazy<Mutex<Option<PathBuf>>> = Lazy::new(|| Mutex::new(None));

static FAIL_WRITES: Lazy<Mutex<HashSet<(String, String)>>> =
    Lazy::new(|| Mutex::new(HashSet::new()));

static FAIL_READS: Lazy<Mutex<HashSet<(String, String)>>> =
    Lazy::new(|| Mutex::new(HashSet::new()));

struct TestCredential {
    service: String,
    user: String,
}

impl CredentialApi for TestCredential {
    fn set_secret(&self, secret: &[u8]) -> keyring::Result<()> {
        let key = (self.service.clone(), self.user.clone());
        if FAIL_WRITES
            .lock()
            .expect("test keychain lock")
            .contains(&key)
        {
            return Err(keyring::Error::NoStorageAccess(
                "test keychain write failed".into(),
            ));
        }
        let password = String::from_utf8(secret.to_vec()).expect("utf-8 secret");
        with_store(|store| {
            store.insert(key, password);
        });
        Ok(())
    }

    fn get_secret(&self) -> keyring::Result<Vec<u8>> {
        let key = (self.service.clone(), self.user.clone());
        if FAIL_READS
            .lock()
            .expect("test keychain lock")
            .contains(&key)
        {
            return Err(keyring::Error::NoStorageAccess(
                "test keychain read failed".into(),
            ));
        }
        with_store(|store| store.get(&key).map(|secret| secret.as_bytes().to_vec()))
            .ok_or(keyring::Error::NoEntry)
    }

    fn delete_credential(&self) -> keyring::Result<()> {
        let key = (self.service.clone(), self.user.clone());
        with_store(|store| store.remove(&key))
            .map(|_| ())
            .ok_or(keyring::Error::NoEntry)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

struct TestCredentialBuilder;

impl CredentialBuilderApi for TestCredentialBuilder {
    fn build(
        &self,
        _target: Option<&str>,
        service: &str,
        user: &str,
    ) -> keyring::Result<Box<keyring::Credential>> {
        Ok(Box::new(TestCredential {
            service: service.to_string(),
            user: user.to_string(),
        }))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub fn install() {
    keyring::set_default_credential_builder(Box::new(TestCredentialBuilder));
}

/// For the build `npm run test:app` drives, in the app and in the mount helper it starts. With `MARLIN_TEST_KEYCHAIN` set, the store is that file, so the helper reads the sign-ins the app saved. Without it the store stays in memory. Never the OS keychain either way.
pub fn install_for_app_test() {
    *FILE.lock().expect("test keychain lock") =
        std::env::var_os("MARLIN_TEST_KEYCHAIN").map(PathBuf::from);
    install();
}

pub fn set(service: &str, user: &str, secret: &str) {
    let key = (service.to_string(), user.to_string());
    let secret = secret.to_string();
    with_store(|store| {
        store.insert(key, secret);
    });
}

pub fn get(service: &str, user: &str) -> Option<String> {
    let key = (service.to_string(), user.to_string());
    with_store(|store| store.get(&key).cloned())
}

pub fn remove(service: &str, user: &str) {
    let key = (service.to_string(), user.to_string());
    with_store(|store| {
        store.remove(&key);
    });
}

/// Applies `change` to the in-memory store, or to the file when `install_for_app_test` chose one.
fn with_store<T>(change: impl FnOnce(&mut HashMap<(String, String), String>) -> T) -> T {
    // Held to the end, so two changes from one process never cross.
    let file = FILE.lock().expect("test keychain lock");
    match &*file {
        None => {
            let mut store = STORE.lock().expect("test keychain lock");
            change(&mut store)
        }
        Some(path) => {
            // The file is a JSON list of `[service, user, secret]`. No lock between the processes: in the tests only one of them writes at a time.
            let triples: Vec<(String, String, String)> = match std::fs::read(&path) {
                Ok(bytes) => serde_json::from_slice(&bytes).expect("test keychain file"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(error) => panic!("test keychain file: {error}"),
            };
            let mut store: HashMap<(String, String), String> = triples
                .into_iter()
                .map(|(service, user, secret)| ((service, user), secret))
                .collect();
            let before = store.clone();
            let result = change(&mut store);
            if store != before {
                let triples: Vec<(&String, &String, &String)> = store
                    .iter()
                    .map(|((service, user), secret)| (service, user, secret))
                    .collect();
                // Renamed into place, so a read in the other process never sees half a file.
                let written = path.with_extension("tmp");
                std::fs::write(&written, serde_json::to_vec(&triples).unwrap())
                    .expect("test keychain file");
                std::fs::rename(&written, &path).expect("test keychain file");
            }
            result
        }
    }
}

pub fn fail_writes_for(service: &str, user: &str) {
    FAIL_WRITES
        .lock()
        .expect("test keychain lock")
        .insert((service.to_string(), user.to_string()));
}

pub fn fail_reads_for(service: &str, user: &str) {
    FAIL_READS
        .lock()
        .expect("test keychain lock")
        .insert((service.to_string(), user.to_string()));
}
