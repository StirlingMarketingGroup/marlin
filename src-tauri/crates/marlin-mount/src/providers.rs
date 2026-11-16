//! The storage providers the helper links.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use marlin_gdrive::{DriveProvider, OAuthClient, SavedSignIn};
use marlin_vfs::Provider;

use crate::MountError;

/// The storage providers the helper links. Google Drive (`gdrive`) is the only one.
pub struct LinkedProviders {
    drive: marlin_gdrive::Config,
    /// Finds Marlin's OAuth client. `OAuthClient::load` in the helper. A test hands in one that points at a fake token endpoint.
    oauth_client: Box<dyn Fn() -> Result<OAuthClient, String> + Send + Sync>,
    /// One saved sign-in per Google account for the life of the helper. It keeps the refresh token in memory, so a second value for the account would read the keychain a second time.
    sign_ins: Mutex<HashMap<String, Arc<SavedSignIn>>>,
}

impl LinkedProviders {
    pub fn new(
        drive: marlin_gdrive::Config,
        oauth_client: impl Fn() -> Result<OAuthClient, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            drive,
            oauth_client: Box::new(oauth_client),
            sign_ins: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl crate::Providers for LinkedProviders {
    async fn open(&self, provider: &str, account: &str) -> Result<Arc<dyn Provider>, MountError> {
        if provider != "gdrive" {
            return Err(MountError::UnknownProvider);
        }
        let sign_in = {
            let mut sign_ins = self.sign_ins.lock().unwrap();
            match sign_ins.get(account) {
                Some(sign_in) => Arc::clone(sign_in),
                None => {
                    // The loader's own text can hold a file path, so it is never logged or returned.
                    let client = (self.oauth_client)().map_err(|_| {
                        MountError::Provider(
                            "this build of Marlin has no Google OAuth client".into(),
                        )
                    })?;
                    let sign_in = Arc::new(SavedSignIn::new(account, client));
                    sign_ins.insert(account.to_string(), Arc::clone(&sign_in));
                    sign_in
                }
            }
        };
        Ok(Arc::new(DriveProvider::new(sign_in, self.drive.clone())))
    }
}
