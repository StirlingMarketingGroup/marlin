//! The mount table, the adapter and provider seams, and [`MountError`].

use std::collections::HashMap;
use std::fmt;
use std::io::ErrorKind;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use marlin_vfs::{Options, Provider, Vfs, VfsError};

use crate::log::{self, redact};
use crate::protocol::{error_codes, MountState, MountStatus, RpcError};

/// What an OS adapter gives the helper. `marlin-fuse` is the Linux one. Issues #373 and #375 add the others.
#[async_trait::async_trait]
pub trait Adapter: Send + Sync + 'static {
    /// Serves `vfs` at `mount_point`, an empty folder that exists. Returns once the OS shows the mount.
    async fn mount(
        &self,
        vfs: Arc<Vfs>,
        mount_point: &Path,
    ) -> Result<Box<dyn Session>, MountError>;
}

/// One live mount.
#[async_trait::async_trait]
pub trait Session: Send + 'static {
    /// Unmounts, and returns once the OS no longer shows the mount. After an error the mount is still served.
    async fn unmount(&mut self) -> Result<(), MountError>;
}

/// Opens the storage behind an account. Issue #372 adds the first real one.
#[async_trait::async_trait]
pub trait Providers: Send + Sync + 'static {
    /// `provider` is a kind such as `gdrive`. `account` names the account and is never a secret: the provider reads its own credentials from the OS keychain.
    async fn open(&self, provider: &str, account: &str) -> Result<Arc<dyn Provider>, MountError>;
}

/// A failure the helper can send on the wire.
///
/// The text in a `String` variant must hold no credential or token. As a backstop, `Display` passes it through [`redact`], and `Display` is the only way the text reaches a log line, a status, or a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountError {
    InvalidName,
    NameInUse,
    MountPointNotEmpty,
    UnknownProvider,
    Provider(String),
    Adapter(String),
    Unmount(String),
    Io(String),
    ShuttingDown,
}

impl fmt::Display for MountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => f.write_str("the mount name is not one plain folder name"),
            Self::NameInUse => f.write_str("another account already uses this mount name"),
            Self::MountPointNotEmpty => {
                f.write_str("the mount point exists and is not an empty folder")
            }
            Self::UnknownProvider => f.write_str("unknown provider"),
            Self::Provider(text) | Self::Adapter(text) | Self::Unmount(text) | Self::Io(text) => {
                f.write_str(&redact(text))
            }
            Self::ShuttingDown => f.write_str("the helper is shutting down"),
        }
    }
}

impl std::error::Error for MountError {}

impl From<std::io::Error> for MountError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<MountError> for RpcError {
    fn from(error: MountError) -> Self {
        Self {
            message: error.to_string(),
            code: match error {
                MountError::InvalidName => error_codes::INVALID_NAME,
                MountError::NameInUse => error_codes::NAME_IN_USE,
                MountError::MountPointNotEmpty => error_codes::MOUNT_POINT_NOT_EMPTY,
                MountError::UnknownProvider => error_codes::UNKNOWN_PROVIDER,
                MountError::Provider(_) => error_codes::PROVIDER,
                MountError::Adapter(_) => error_codes::ADAPTER,
                MountError::Unmount(_) => error_codes::UNMOUNT,
                MountError::Io(_) => error_codes::IO,
                MountError::ShuttingDown => error_codes::SHUTTING_DOWN,
            },
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    provider: String,
    account: String,
}

/// How many provider calls must fail in a row before a mount becomes `offline` or `degraded`. One slow call or one dropped connection is not news.
const FAILURES_IN_A_ROW: u32 = 3;

/// What the last provider calls say about a mount's remote, from best to worst. A run of failed calls only moves a mount toward the worse end. The first call the remote answers moves it back to `Healthy`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Health {
    Healthy,
    Degraded,
    Offline,
    AuthRequired,
}

/// Where one mount is in its life, with what that state holds. Between operations a mount is `Mounted` or `Failed`.
enum Lifecycle {
    Starting,
    Mounted(Box<dyn Session>),
    Unmounting,
    Failed(MountError),
}

struct MountEntry {
    name: String,
    lifecycle: Lifecycle,

    /// Which mount attempt made this entry. A provider call from an earlier attempt of the same account must not change this one.
    attempt: u64,
    health: Health,

    /// Provider calls that failed in a row.
    failures: u32,
}

struct Table {
    entries: HashMap<Key, MountEntry>,

    /// One operation lock per key. A lock belongs to the key, not to the entry, and is never removed, so a queued operation can never run against an entry that another operation removed or replaced.
    locks: HashMap<Key, Arc<tokio::sync::Mutex<()>>>,

    /// True from the moment a shutdown starts until it fails or the helper exits.
    closing: bool,

    /// How many mount attempts this helper has started. Each new entry takes the next number.
    attempts: u64,
}

/// Every mount the helper serves, keyed by provider and account.
///
/// A mount or unmount first takes its key's operation lock, so operations on one mount run one at a time and operations on different mounts run together. The table's own lock is never held across an `.await` or a log write. Events are sent under that lock, because a provider call can change a mount's health at any moment, outside the operation lock.
pub struct Mounts {
    table: Mutex<Table>,
    events: tokio::sync::broadcast::Sender<MountStatus>,
    adapter: Arc<dyn Adapter>,
    providers: Arc<dyn Providers>,
    root: PathBuf,
}

impl Mounts {
    pub fn new(root: PathBuf, adapter: Arc<dyn Adapter>, providers: Arc<dyn Providers>) -> Self {
        Self {
            table: Mutex::new(Table {
                entries: HashMap::new(),
                locks: HashMap::new(),
                closing: false,
                attempts: 0,
            }),
            events: tokio::sync::broadcast::channel(64).0,
            adapter,
            providers,
            root,
        }
    }

    /// Every state change of every mount, from now on.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<MountStatus> {
        self.events.subscribe()
    }

    /// Mounts an account at `root/name` and returns once the OS shows it. The caller built `name` once and stores it with the account, so the helper checks it and never invents one.
    pub async fn mount(
        self: &Arc<Self>,
        provider: &str,
        account: &str,
        name: &str,
    ) -> Result<MountStatus, MountError> {
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.len() > 255
            || name.contains(['/', '\0'])
        {
            return Err(MountError::InvalidName);
        }
        let key = Key {
            provider: provider.to_string(),
            account: account.to_string(),
        };
        let operation = self.operation_lock(&key);
        let _operation = operation.lock().await;

        let (starting, attempt) = {
            let mut table = self.table.lock().unwrap();
            if table.closing {
                return Err(MountError::ShuttingDown);
            }
            // A live mount keeps its name, even when the request asks for another. This is also what makes a repeated mount safe: the adapter is called once.
            if let Some(entry) = table.entries.get(&key) {
                if matches!(entry.lifecycle, Lifecycle::Mounted(_)) {
                    return Ok(snapshot(&key, entry, &self.root));
                }
            }
            let name_in_use = table.entries.iter().any(|(other, entry)| {
                other != &key && entry.name.to_lowercase() == name.to_lowercase()
            });
            if name_in_use {
                return Err(MountError::NameInUse);
            }
            table.attempts += 1;
            let attempt = table.attempts;
            let entry = MountEntry {
                name: name.to_string(),
                lifecycle: Lifecycle::Starting,
                attempt,
                health: Health::Healthy,
                failures: 0,
            };
            let starting = self.announce(&key, &entry);
            table.entries.insert(key.clone(), entry);
            (starting, attempt)
        };
        log_state(&starting);

        let mount_point = self.root.join(name);
        let mounted = async {
            // The provider comes first, so a mount it refuses leaves no folder behind.
            let provider = self.providers.open(provider, account).await?;
            std::fs::create_dir_all(&self.root)?;
            // This protects the user's own files from a mistake. It is no defense against another process of the same user, which can already do all the helper can.
            match std::fs::symlink_metadata(&mount_point) {
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    std::fs::DirBuilder::new()
                        .mode(0o700)
                        .create(&mount_point)?;
                }
                Err(error) => return Err(error.into()),
                // A symlink is not a directory here, so the helper never mounts through one.
                Ok(found) if !found.is_dir() => return Err(MountError::MountPointNotEmpty),
                Ok(_) => {
                    if std::fs::read_dir(&mount_point)?.next().is_some() {
                        return Err(MountError::MountPointNotEmpty);
                    }
                }
            }
            let mounts = Arc::downgrade(self);
            let watched = key.clone();
            let vfs = Vfs::new(provider, marlin_vfs::POSIX, Options::default()).watch_calls(
                // Weak, because a session can hold the `Vfs`, and the table holds the session.
                move |error| {
                    if let Some(mounts) = mounts.upgrade() {
                        mounts.record(&watched, attempt, error);
                    }
                },
            );
            self.adapter.mount(Arc::new(vfs), &mount_point).await
        }
        .await;

        match mounted {
            Ok(session) => Ok(self.transition(&key, Lifecycle::Mounted(session))),
            Err(error) => {
                // An empty folder at a mount point is the helper's to remove. This fails, and changes nothing, on a file, a symlink, or a folder with something in it.
                let _ = std::fs::remove_dir(&mount_point);
                self.transition(&key, Lifecycle::Failed(error.clone()));
                Err(error)
            }
        }
    }

    /// Unmounts an account. An account with no mount is a success.
    pub async fn unmount(&self, provider: &str, account: &str) -> Result<(), MountError> {
        let key = Key {
            provider: provider.to_string(),
            account: account.to_string(),
        };
        let operation = self.operation_lock(&key);
        let _operation = operation.lock().await;

        let (unmounting, previous) = {
            let mut table = self.table.lock().unwrap();
            let Some(entry) = table.entries.get_mut(&key) else {
                return Ok(());
            };
            let previous = std::mem::replace(&mut entry.lifecycle, Lifecycle::Unmounting);
            // Only a mount that was up announces `unmounting`.
            let unmounting = if matches!(previous, Lifecycle::Mounted(_)) {
                self.announce(&key, entry)
            } else {
                snapshot(&key, entry, &self.root)
            };
            (unmounting, previous)
        };

        // A failed mount has no session and no folder of the helper's: only its entry goes.
        if let Lifecycle::Mounted(mut session) = previous {
            log_state(&unmounting);
            if let Err(error) = session.unmount().await {
                self.transition(&key, Lifecycle::Mounted(session));
                return Err(MountError::Unmount(error.to_string()));
            }
            // The folder goes before the entry does, so no other account can be given this name while the folder is being removed.
            let _ = std::fs::remove_dir(self.root.join(&unmounting.name));
        }

        let unmounted = MountStatus {
            state: MountState::Unmounted,
            ..unmounting
        };
        {
            let mut table = self.table.lock().unwrap();
            table.entries.remove(&key);
            let _ = self.events.send(unmounted.clone());
        }
        log_state(&unmounted);
        Ok(())
    }

    /// Every mount, sorted by name.
    pub fn list(&self) -> Vec<MountStatus> {
        let table = self.table.lock().unwrap();
        let mut statuses: Vec<_> = table
            .entries
            .iter()
            .map(|(key, entry)| snapshot(key, entry, &self.root))
            .collect();
        statuses.sort_by(|a, b| a.name.cmp(&b.name));
        statuses
    }

    pub fn status(&self, provider: &str, account: &str) -> Option<MountStatus> {
        let key = Key {
            provider: provider.to_string(),
            account: account.to_string(),
        };
        let table = self.table.lock().unwrap();
        let entry = table.entries.get(&key)?;
        Some(snapshot(&key, entry, &self.root))
    }

    /// Shuts down: refuses new mounts and unmounts every mount.
    ///
    /// `closing` is set and the keys are copied under one lock. A mount admitted before that has its entry in the copy, and a mount that arrives after it is refused, so no mount slips past a shutdown. If an unmount fails, the helper takes mounts again: a mount that is still served must not lose its helper.
    pub async fn close(&self) -> Result<(), MountError> {
        let keys: Vec<Key> = {
            let mut table = self.table.lock().unwrap();
            table.closing = true;
            table.entries.keys().cloned().collect()
        };
        let mut failed = Vec::new();
        for key in keys {
            if let Err(error) = self.unmount(&key.provider, &key.account).await {
                failed.push(format!("{} {}: {error}", key.provider, key.account));
            }
        }
        if failed.is_empty() {
            return Ok(());
        }
        self.table.lock().unwrap().closing = false;
        Err(MountError::Unmount(format!(
            "could not unmount {}",
            failed.join(", ")
        )))
    }

    fn operation_lock(&self, key: &Key) -> Arc<tokio::sync::Mutex<()>> {
        let mut table = self.table.lock().unwrap();
        table.locks.entry(key.clone()).or_default().clone()
    }

    /// Applies how one provider call ended to its mount's health. The rules are in `docs/mount-helper.md`, under "Health".
    fn record(&self, key: &Key, attempt: u64, error: Option<&VfsError>) {
        let changed = {
            let mut table = self.table.lock().unwrap();
            let Some(entry) = table.entries.get_mut(key) else {
                return;
            };
            if entry.attempt != attempt {
                return;
            }
            let before = entry.health;
            let says = match error {
                Some(VfsError::SignInNeeded) => Health::AuthRequired,
                Some(VfsError::Offline) => Health::Offline,
                Some(VfsError::TimedOut | VfsError::Other(_)) => Health::Degraded,
                // Any other result is an answer from the remote, even when the answer is "not found".
                _ => Health::Healthy,
            };
            if says == Health::Healthy {
                entry.failures = 0;
                entry.health = Health::Healthy;
            } else {
                entry.failures += 1;
                // The remote itself said the sign-in is gone, so there is nothing to wait for.
                if says == Health::AuthRequired || entry.failures >= FAILURES_IN_A_ROW {
                    entry.health = entry.health.max(says);
                }
            }
            // Health is kept in every lifecycle, but only a mount that is up shows it.
            if entry.health == before || !matches!(entry.lifecycle, Lifecycle::Mounted(_)) {
                return;
            }
            self.announce(key, entry)
        };
        log_state(&changed);
    }

    /// Moves a mount to a new state and tells every listener.
    fn transition(&self, key: &Key, lifecycle: Lifecycle) -> MountStatus {
        let status = {
            let mut table = self.table.lock().unwrap();
            let entry = table.entries.get_mut(key).unwrap();
            entry.lifecycle = lifecycle;
            self.announce(key, entry)
        };
        log_state(&status);
        status
    }

    /// Builds a mount's status and tells every listener. The caller holds the table's lock, so listeners hear changes in the order the table made them.
    fn announce(&self, key: &Key, entry: &MountEntry) -> MountStatus {
        let status = snapshot(key, entry, &self.root);
        let _ = self.events.send(status.clone());
        status
    }
}

fn log_state(status: &MountStatus) {
    let level = if status.state == MountState::Failed {
        "error"
    } else {
        "info"
    };
    log::write(level, "mount.state", serde_json::json!(status));
}

fn snapshot(key: &Key, entry: &MountEntry, root: &Path) -> MountStatus {
    let (state, error) = match &entry.lifecycle {
        Lifecycle::Starting => (MountState::Starting, None),
        Lifecycle::Mounted(_) => (
            match entry.health {
                Health::Healthy => MountState::Mounted,
                Health::Degraded => MountState::Degraded,
                Health::Offline => MountState::Offline,
                Health::AuthRequired => MountState::AuthRequired,
            },
            None,
        ),
        Lifecycle::Unmounting => (MountState::Unmounting, None),
        Lifecycle::Failed(error) => (MountState::Failed, Some(error.clone().into())),
    };
    MountStatus {
        provider: key.provider.clone(),
        account: key.account.clone(),
        name: entry.name.clone(),
        mount_point: root.join(&entry.name).display().to_string(),
        state,
        error,
    }
}
