//! The storage providers the helper links.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use marlin_gdrive::{DriveProvider, OAuthClient, SavedSignIn};
use marlin_sftp::{SftpAccount, SftpProvider};

use crate::{MountError, Opened};

/// The helper links Google Drive (`gdrive`) and SFTP (`sftp`).
pub struct LinkedProviders {
    drive: marlin_gdrive::Config,
    /// Finds Marlin's OAuth client. `OAuthClient::load` in the helper. A test hands in one that points at a fake token endpoint.
    oauth_client: Box<dyn Fn() -> Result<OAuthClient, String> + Send + Sync>,
    /// One saved sign-in per Google account for the life of the helper. It keeps the refresh token in memory, so a second value for the account would read the keychain a second time.
    sign_ins: Mutex<HashMap<String, Arc<SavedSignIn>>>,
    sftp: marlin_sftp::Config,
    /// One `SftpAccount` per SFTP account for the life of the helper, so a sign-in the server refused stays refused across an unmount and a new mount. Each mount gets its own provider, so its connection closes when the mount goes. The key is the parsed account with the host in lower case: `deploy@Host` and `deploy@host` find the same saved server, so they share one account.
    sftp_accounts: Mutex<HashMap<(String, String, u16), Arc<SftpAccount>>>,
}

impl LinkedProviders {
    pub fn new(
        drive: marlin_gdrive::Config,
        oauth_client: impl Fn() -> Result<OAuthClient, String> + Send + Sync + 'static,
        sftp: marlin_sftp::Config,
    ) -> Self {
        Self {
            drive,
            oauth_client: Box::new(oauth_client),
            sign_ins: Mutex::new(HashMap::new()),
            sftp,
            sftp_accounts: Mutex::new(HashMap::new()),
        }
    }

    fn open_gdrive(&self, account: &str) -> Result<Opened, MountError> {
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
        Ok(Opened {
            provider: Arc::new(DriveProvider::new(sign_in, self.drive.clone())),
            max_calls: None,
        })
    }

    fn open_sftp(&self, account: &str) -> Result<Opened, MountError> {
        let (user, host, port) = marlin_sftp::parse_authority(account).map_err(|_| {
            MountError::Provider(
                "the account is not an SFTP server: user@host, or user@host:port".into(),
            )
        })?;
        let sftp_account = Arc::clone(
            self.sftp_accounts
                .lock()
                .unwrap()
                .entry((user.clone(), host.to_ascii_lowercase(), port))
                .or_insert_with(|| {
                    Arc::new(SftpAccount::new(&user, &host, port, self.sftp.clone()))
                }),
        );
        Ok(Opened {
            provider: Arc::new(SftpProvider::new(sftp_account)),
            max_calls: Some(marlin_sftp::MAX_CALLS),
        })
    }
}

#[async_trait::async_trait]
impl crate::Providers for LinkedProviders {
    async fn open(&self, provider: &str, account: &str) -> Result<Opened, MountError> {
        match provider {
            "gdrive" => self.open_gdrive(account),
            "sftp" => self.open_sftp(account),
            _ => Err(MountError::UnknownProvider),
        }
    }
}
