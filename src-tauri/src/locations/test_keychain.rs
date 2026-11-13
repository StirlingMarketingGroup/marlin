//! In-memory stand-in for the OS keychain, shared by every test in this process.
//! There is only one: the keyring builder is process-global, and a second store would split each module's entries.

use keyring::credential::{CredentialApi, CredentialBuilderApi};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

static STORE: Lazy<Mutex<HashMap<(String, String), String>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

static FAIL_WRITES: Lazy<Mutex<HashSet<(String, String)>>> =
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
        STORE
            .lock()
            .expect("test keychain lock")
            .insert(key, password);
        Ok(())
    }

    fn get_secret(&self) -> keyring::Result<Vec<u8>> {
        STORE
            .lock()
            .expect("test keychain lock")
            .get(&(self.service.clone(), self.user.clone()))
            .map(|secret| secret.as_bytes().to_vec())
            .ok_or(keyring::Error::NoEntry)
    }

    fn delete_credential(&self) -> keyring::Result<()> {
        STORE
            .lock()
            .expect("test keychain lock")
            .remove(&(self.service.clone(), self.user.clone()))
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

pub(crate) fn install() {
    keyring::set_default_credential_builder(Box::new(TestCredentialBuilder));
}

pub(crate) fn set(service: &str, user: &str, secret: &str) {
    STORE
        .lock()
        .expect("test keychain lock")
        .insert((service.to_string(), user.to_string()), secret.to_string());
}

pub(crate) fn get(service: &str, user: &str) -> Option<String> {
    STORE
        .lock()
        .expect("test keychain lock")
        .get(&(service.to_string(), user.to_string()))
        .cloned()
}

pub(crate) fn remove(service: &str, user: &str) {
    STORE
        .lock()
        .expect("test keychain lock")
        .remove(&(service.to_string(), user.to_string()));
}

pub(crate) fn fail_writes_for(service: &str, user: &str) {
    FAIL_WRITES
        .lock()
        .expect("test keychain lock")
        .insert((service.to_string(), user.to_string()));
}
