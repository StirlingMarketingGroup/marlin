//! The mount table, the adapter and provider seams, and [`MountError`].

use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use marlin_vfs::{NodeId, Options, Provider, Spool, Vfs, VfsError};
use uuid::Uuid;

use crate::log::{self, redact};
use crate::protocol::{
    error_codes, DeadMount, MountState, MountStatus, ReadOnly, RpcError, UnsentFile, UnsentFiles,
    UnsentState,
};

/// The file that marks the mount folder as the helper's. The helper sweeps, and mounts in, only a mount folder that has it, so a folder of the user's with the same name is never changed: on a Mac's disk, which ignores letter case, a checkout of Marlin at `~/marlin` is `~/Marlin` (#541).
pub const MOUNT_FOLDER_MARKER: &str = ".marlin-mount-folder";

/// What an OS adapter gives the helper. `marlin-fuse` is the one for Linux and macOS. Issue #375 adds Windows.
#[async_trait::async_trait]
pub trait Adapter: Send + Sync + 'static {
    /// Serves `vfs` at `mount_point`, an empty folder that exists. Returns once the OS shows the mount.
    async fn mount(
        &self,
        vfs: Arc<Vfs>,
        mount_point: &Path,
    ) -> Result<Box<dyn Session>, MountError>;

    /// Whether a dead mount is at `mount_point`: one whose server was killed, so the OS still lists it and a listing of it fails. A live mount, a plain folder, a symbolic link, and a path that is not there are not dead mounts.
    async fn is_dead_mount(&self, mount_point: &Path) -> bool;

    /// Unmounts the dead mount at `mount_point`. The folder itself stays. Fails with [`MountError::DeadMount`] when the dead mount is still there.
    async fn unmount_dead(&self, mount_point: &Path) -> Result<(), MountError>;
}

/// One live mount.
#[async_trait::async_trait]
pub trait Session: Send + 'static {
    /// Unmounts, and returns once the OS no longer shows the mount. After an error the mount is still served.
    async fn unmount(&mut self) -> Result<(), MountError>;
}

/// Opens the storage behind an account. `LinkedProviders` is the one the helper links.
#[async_trait::async_trait]
pub trait Providers: Send + Sync + 'static {
    /// `provider` is a kind such as `gdrive`. `account` names the account and is never a secret: the provider reads its own credentials from the OS keychain. `open` need not reach the remote: the mount table makes the first call.
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
    DeadMount(String),
    Io(String),
    ShuttingDown,
    /// The account's first call said the sign-in is gone. Only a new sign-in fixes it.
    SignInNeeded,
    /// The account's first call got no answer from the remote. A later try can work.
    Unreachable,
    /// That file has no unsent bytes in this mount, or the mount id is not this mount's.
    NotUnsent,
    /// A discard waited too long for the mount's other changes.
    Busy,
    /// The mount folder holds things the helper did not put there, and no marker.
    MountFolderTaken(PathBuf),
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
            Self::Provider(text)
            | Self::Adapter(text)
            | Self::Unmount(text)
            | Self::DeadMount(text)
            | Self::Io(text) => f.write_str(&redact(text)),
            Self::ShuttingDown => f.write_str("the helper is shutting down"),
            Self::SignInNeeded => f.write_str("the account needs a new sign-in"),
            Self::Unreachable => f.write_str("the account's storage could not be reached"),
            Self::NotUnsent => f.write_str("that file has no unsent bytes in this mount"),
            Self::Busy => f.write_str("the mount is busy with another change; try again"),
            Self::MountFolderTaken(root) => write!(
                f,
                "{} holds files that Marlin did not put there; move or rename it, and Marlin makes a new one",
                redact(&root.to_string_lossy())
            ),
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
                MountError::DeadMount(_) => error_codes::DEAD_MOUNT,
                MountError::Io(_) => error_codes::IO,
                MountError::ShuttingDown => error_codes::SHUTTING_DOWN,
                MountError::SignInNeeded => error_codes::SIGN_IN_NEEDED,
                MountError::Unreachable => error_codes::UNREACHABLE,
                MountError::NotUnsent => error_codes::NOT_UNSENT,
                MountError::Busy => error_codes::BUSY,
                MountError::MountFolderTaken(_) => error_codes::MOUNT_FOLDER_TAKEN,
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

/// How long a mount waits before it logs the same failure text again.
const LOG_SAME_TEXT_EVERY: Duration = Duration::from_secs(60);

/// How often the helper looks at each mount's unsent files. It bounds the rate of `unsentChanged`, and it is how often an unmount looks whether its uploads are done.
const UNSENT_EVERY: Duration = Duration::from_millis(500);

/// What the helper tells every client on its protocol version.
#[derive(Clone)]
pub(crate) enum Notice {
    Mount(MountStatus),
    Unsent(UnsentFiles),
}

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

    /// Which mount attempt made this entry, and its `mountId` on the wire. A provider call from an earlier attempt of the same account must not change this one. A random ID, because a list kept across a remount or a helper restart must never name another mount's files: a discard drops bytes for good.
    attempt: Uuid,
    health: Health,

    writing: Writing,

    /// Provider calls that failed in a row.
    failures: u32,

    /// The text of the last `VfsError::Other` this mount logged, and when. A mount gets a call for every file the OS touches, so the same text is logged once in `LOG_SAME_TEXT_EVERY`, not once per call.
    logged_other: Option<(String, tokio::time::Instant)>,
}

struct Table {
    entries: HashMap<Key, MountEntry>,

    /// One operation lock per key. A lock belongs to the key, not to the entry, and is never removed, so a queued operation can never run against an entry that another operation removed or replaced.
    locks: HashMap<Key, Arc<tokio::sync::Mutex<()>>>,

    /// True from the moment a shutdown starts until it fails or the helper exits.
    closing: bool,
}

/// How a mount takes writes. Decided when the mount comes up, and turned off when its unmount stops the uploads.
enum Writing {
    /// Not now: the mount is starting, it failed, or its unmount has stopped the uploads.
    Off,
    /// Never, for this reason.
    ReadOnly(ReadOnly),
    /// Into its spool, from the moment the mount is up until its unmount has stopped the uploads.
    Spooled(Writes),
}

/// A mount's spool at work: the core that holds it, the two tasks that run beside the mount, and what keeps the spool folder this mount's alone.
struct Writes {
    vfs: Arc<Vfs>,
    /// `Vfs::run_uploads`. Aborted to stop uploading; the spool files stay.
    uploads: tokio::task::JoinHandle<()>,
    /// Sends `unsentChanged` when the mount's unsent files change.
    watch: tokio::task::JoinHandle<()>,
    /// Each retry and discard holds it for reading while it runs. The unmount takes it for writing before it lets the spool go, so no call of this mount touches the spool folder after a new mount of the account has it.
    calls: Arc<tokio::sync::RwLock<()>>,
    /// The lock `open_spool` took on the account's spool, held for the life of this value.
    _lock: File,
}

impl Writes {
    /// Stops the uploads, waits until nothing of this mount can touch the spool folder, then lets the spool lock go.
    async fn stop(mut self) {
        // Waits for every retry and discard that already started. None starts once the mount's writing is off.
        let _calls = self.calls.write().await;
        self.uploads.abort();
        self.watch.abort();
        let _ = (&mut self.uploads).await;
        let _ = (&mut self.watch).await;
        // Aborting `run_uploads` only asks its upload tasks to stop, and each try leaves the uploading state when its task ends. A try a discard already stopped no longer shows as uploading, so this does not wait for it: such a try only ends, and its spool file is already gone.
        while self
            .vfs
            .unsent()
            .iter()
            .any(|file| matches!(file.state, marlin_vfs::UnsentState::Uploading { .. }))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// Every mount the helper serves, keyed by provider and account.
///
/// A mount or unmount first takes its key's operation lock, so operations on one mount run one at a time and operations on different mounts run together. The table's own lock is never held across an `.await` or a log write. A `mountChanged` is sent under that lock, because a provider call can change a mount's health at any moment, outside the operation lock. An `unsentChanged` needs no lock: it comes from the mount's own watcher, or from its unmount once that watcher has stopped.
pub struct Mounts {
    table: Mutex<Table>,
    events: tokio::sync::broadcast::Sender<Notice>,
    /// A signal sets this. Every unmount stops waiting for uploads, now and after.
    hurry: tokio::sync::watch::Sender<bool>,
    adapter: Arc<dyn Adapter>,
    providers: Arc<dyn Providers>,
    root: PathBuf,
    /// The folder that holds one spool folder per account.
    spool: PathBuf,
    /// The limits of each mount's core.
    options: Options,
}

impl Mounts {
    pub fn new(
        root: PathBuf,
        spool: PathBuf,
        options: Options,
        adapter: Arc<dyn Adapter>,
        providers: Arc<dyn Providers>,
    ) -> Self {
        Self {
            table: Mutex::new(Table {
                entries: HashMap::new(),
                locks: HashMap::new(),
                closing: false,
            }),
            events: tokio::sync::broadcast::channel(64).0,
            hurry: tokio::sync::watch::channel(false).0,
            adapter,
            providers,
            root,
            spool,
            options,
        }
    }

    /// Every notice for every mount, from now on.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Notice> {
        self.events.subscribe()
    }

    /// A signal is an order to exit. Every unmount stops waiting for uploads, now and after, and the spool keeps what is left for the next mount of the account.
    pub fn hurry(&self) {
        self.hurry.send_replace(true);
    }

    /// Whether the mount folder is the helper's: it holds the marker.
    fn owns_root(&self) -> bool {
        std::fs::symlink_metadata(self.root.join(MOUNT_FOLDER_MARKER))
            .is_ok_and(|found| found.is_file())
    }

    /// Removes an empty mount point. An empty folder at a mount point is the helper's to remove, but only while the mount folder holds the marker: the marker can be deleted, or a linked mount folder pointed elsewhere, while a mount is up. This fails, and changes nothing, on a file, a symlink, a live mount, or a folder with something in it.
    fn remove_mount_point(&self, mount_point: &Path) {
        if self.owns_root() {
            let _ = std::fs::remove_dir(mount_point);
        }
    }

    /// The real folders in the mount folder. The type comes from the mount folder's own listing, so a symbolic link is never handed to the adapter or removed.
    fn folders(&self) -> Vec<PathBuf> {
        // A mount folder without the marker, or none at all, holds no mount of the helper's, so the sweep and `deadMounts` leave it alone.
        if !self.owns_root() {
            return Vec::new();
        }
        let Ok(found) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        found
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path())
            .collect()
    }

    /// Every dead mount under the mount folder right now, sorted by name. After the sweep, these are the ones it could not clear.
    pub async fn dead_mounts(&self) -> Vec<DeadMount> {
        let mut dead = Vec::new();
        for mount_point in self.folders() {
            if self.adapter.is_dead_mount(&mount_point).await {
                dead.push(DeadMount {
                    name: mount_point
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    mount_point: mount_point.display().to_string(),
                });
            }
        }
        dead.sort_by(|a, b| a.name.cmp(&b.name));
        dead
    }

    /// Clears what a killed helper left under the mount folder: dead mounts and empty folders. It does nothing in a mount folder without the marker, which is not the helper's. The server runs this once, before it takes a request, so no mount of this helper exists yet.
    pub async fn sweep(&self) {
        for mount_point in self.folders() {
            if self.adapter.is_dead_mount(&mount_point).await {
                if let Err(error) = self.adapter.unmount_dead(&mount_point).await {
                    log::write(
                        "error",
                        "helper.dead_mount",
                        serde_json::json!({"mountPoint": mount_point.display().to_string(), "message": error.to_string()}),
                    );
                    continue;
                }
            }
            self.remove_mount_point(&mount_point);
        }
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
        // A shutdown can hold this account's operation lock while it waits for uploads. A mount sent meanwhile is refused at once, not queued behind it.
        if self.table.lock().unwrap().closing {
            return Err(MountError::ShuttingDown);
        }
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
            let attempt = Uuid::new_v4();
            let entry = MountEntry {
                name: name.to_string(),
                lifecycle: Lifecycle::Starting,
                attempt,
                health: Health::Healthy,
                failures: 0,
                logged_other: None,
                writing: Writing::Off,
            };
            let starting = self.announce(&key, &entry);
            table.entries.insert(key.clone(), entry);
            (starting, attempt)
        };
        log_state(&starting);

        let mount_point = self.root.join(name);
        let mounted = async {
            // The provider comes first, so a mount it refuses leaves no folder behind.
            let opened = self.providers.open(provider, account).await?;
            let build = || {
                let mounts = Arc::downgrade(self);
                let watched = key.clone();
                Vfs::new(Arc::clone(&opened), marlin_vfs::POSIX, self.options).watch_calls(
                    // Weak, because a session can hold the `Vfs`, and the table holds the session.
                    move |error| {
                        if let Some(mounts) = mounts.upgrade() {
                            mounts.record(&watched, attempt, error);
                        }
                    },
                )
            };
            let vfs = build();
            // A mount starts as `mounted`, and its health only changes at a provider call. So the remote answers one call before the mount comes up, and before any folder is made. A provider that kept a token from an earlier mount cannot hide a remote that is out of reach.
            vfs.space().await.map_err(|error| match error {
                VfsError::SignInNeeded => MountError::SignInNeeded,
                VfsError::Offline | VfsError::TimedOut => MountError::Unreachable,
                VfsError::AccessDenied => {
                    MountError::Provider("the account's storage does not let Marlin read it".into())
                }
                error => MountError::Provider(error.to_string()),
            })?;
            std::fs::create_dir_all(&self.root)?;
            // An empty mount folder has nothing to lose, so it becomes the helper's. A mount running at the same time writes the marker before it makes its mount point, so it never makes this folder look taken.
            if std::fs::read_dir(&self.root)?.next().is_none() {
                std::fs::write(
                    self.root.join(MOUNT_FOLDER_MARKER),
                    "Marlin keeps its mounts in this folder. It mounts here, and clears empty folders here when it starts, only while this file is here.\n",
                )?;
            } else if !self.owns_root() {
                return Err(MountError::MountFolderTaken(self.root.clone()));
            }
            // A dead mount can still be here: one the sweep could not clear, or one left after the helper started. This comes before the checks below because they fail on a dead mount, the first of them included, with an error that says nothing.
            if self.adapter.is_dead_mount(&mount_point).await {
                self.adapter.unmount_dead(&mount_point).await?;
            }
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
            // The spool lock, or why the mount takes no writes.
            let (vfs, lock) = if !opened.can_write() {
                (vfs, Err(ReadOnly::Provider))
            } else {
                let taken = self.open_spool(&key).and_then(|(spool, lock)| {
                    let vfs = vfs.take_writes(spool).map_err(|error| error.to_string())?;
                    Ok((vfs, lock))
                });
                match taken {
                    Ok((vfs, lock)) => (vfs, Ok(lock)),
                    Err(message) => {
                        // A read-only mount uploads nothing and deletes no unsent bytes, while the user can still read. `take_writes` used up the first core.
                        let message = redact(&message);
                        log::write(
                            "warn",
                            "mount.spool_failed",
                            serde_json::json!({
                                "provider": key.provider,
                                "account": key.account,
                                "message": message,
                            }),
                        );
                        (build(), Err(ReadOnly::Spool { message }))
                    }
                }
            };
            let vfs = Arc::new(vfs);
            let session = self.adapter.mount(Arc::clone(&vfs), &mount_point).await?;
            Ok((session, vfs, lock))
        }
        .await;

        match mounted {
            Ok((session, vfs, lock)) => {
                let writing = match lock {
                    Ok(lock) => {
                        let uploads = tokio::spawn(Arc::clone(&vfs).run_uploads());
                        let watch = tokio::spawn(watch_unsent(
                            Arc::downgrade(self),
                            key.clone(),
                            attempt,
                            Arc::clone(&vfs),
                        ));
                        Writing::Spooled(Writes {
                            vfs,
                            uploads,
                            watch,
                            calls: Arc::new(tokio::sync::RwLock::new(())),
                            _lock: lock,
                        })
                    }
                    Err(reason) => Writing::ReadOnly(reason),
                };
                let status = {
                    let mut table = self.table.lock().unwrap();
                    let entry = table.entries.get_mut(&key).unwrap();
                    entry.writing = writing;
                    entry.lifecycle = Lifecycle::Mounted(session);
                    self.announce(&key, entry)
                };
                log_state(&status);
                Ok(status)
            }
            Err(error) => {
                self.remove_mount_point(&mount_point);
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
            self.remove_mount_point(&self.root.join(&unmounting.name));

            let spooled = {
                let table = self.table.lock().unwrap();
                let entry = &table.entries[&key];
                match &entry.writing {
                    Writing::Spooled(writes) => Some((Arc::clone(&writes.vfs), entry.attempt)),
                    Writing::Off | Writing::ReadOnly(_) => None,
                }
            };
            if let Some((vfs, attempt)) = spooled {
                // Retry and discard still work while this waits.
                self.finish_uploads(&vfs).await;
                // No retry or discard starts after this. Everything that can touch the spool folder must be done before the spool lock goes, so a new mount of the account never shares the folder with this one.
                let writing = {
                    let mut table = self.table.lock().unwrap();
                    let entry = table.entries.get_mut(&key).unwrap();
                    std::mem::replace(&mut entry.writing, Writing::Off)
                };
                let Writing::Spooled(writes) = writing else {
                    unreachable!("only an unmount turns a mount's writing off");
                };
                writes.stop().await;
                let report = unsent_files(&key, attempt, &vfs);
                if !report.files.is_empty() || !report.unplaced.is_empty() {
                    log::write(
                        "info",
                        "mount.left_unsent",
                        serde_json::json!({
                            "provider": key.provider,
                            "account": key.account,
                            "files": report.files.len(),
                            "unplaced": report.unplaced.len(),
                        }),
                    );
                }
                let _ = self.events.send(Notice::Unsent(report));
            }
        }

        let unmounted = MountStatus {
            state: MountState::Unmounted,
            ..unmounting
        };
        {
            let mut table = self.table.lock().unwrap();
            table.entries.remove(&key);
            let _ = self.events.send(Notice::Mount(unmounted.clone()));
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
    ///
    /// The unmounts run together. Each one can wait for uploads, and one slow account must not keep the others mounted.
    pub async fn close(self: &Arc<Self>) -> Result<(), MountError> {
        let keys: Vec<Key> = {
            let mut table = self.table.lock().unwrap();
            table.closing = true;
            table.entries.keys().cloned().collect()
        };
        let mut tasks = tokio::task::JoinSet::new();
        for key in keys {
            let mounts = Arc::clone(self);
            tasks.spawn(async move {
                mounts
                    .unmount(&key.provider, &key.account)
                    .await
                    .map_err(|error| format!("{} {}: {error}", key.provider, key.account))
            });
        }
        let mut failed = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(Ok(())) => {}
                Ok(Err(text)) => failed.push(text),
                // A panic in an unmount reaches the caller.
                Err(error) => std::panic::resume_unwind(error.into_panic()),
            }
        }
        if failed.is_empty() {
            return Ok(());
        }
        failed.sort();
        self.table.lock().unwrap().closing = false;
        Err(MountError::Unmount(format!(
            "could not unmount {}",
            failed.join(", ")
        )))
    }

    /// The files of one mount that still have to reach the remote, or `None` when the mount takes no writes.
    pub fn unsent(&self, provider: &str, account: &str) -> Option<UnsentFiles> {
        let key = Key {
            provider: provider.to_string(),
            account: account.to_string(),
        };
        let (vfs, attempt) = {
            let table = self.table.lock().unwrap();
            let entry = table.entries.get(&key)?;
            let Writing::Spooled(writes) = &entry.writing else {
                return None;
            };
            (Arc::clone(&writes.vfs), entry.attempt)
        };
        Some(unsent_files(&key, attempt, &vfs))
    }

    /// Sends a failed or refused file now. A node with no unsent bytes, or a mount id that is not this mount's, is [`MountError::NotUnsent`].
    pub fn retry_unsent(
        &self,
        provider: &str,
        account: &str,
        mount_id: &str,
        node: u64,
    ) -> Result<(), MountError> {
        let (_guard, vfs) = self.unsent_call(provider, account, mount_id)?;
        vfs.retry_now(NodeId(node))
            .map_err(|_| MountError::NotUnsent)
    }

    /// Drops one file's unsent bytes. [`MountError::NotUnsent`] when the node has none, and [`MountError::Busy`] when the mount's other changes hold the call up.
    pub async fn discard_unsent(
        &self,
        provider: &str,
        account: &str,
        mount_id: &str,
        node: u64,
    ) -> Result<(), MountError> {
        let (_guard, vfs) = self.unsent_call(provider, account, mount_id)?;
        match vfs.discard_unsent(NodeId(node)).await {
            Ok(()) => Ok(()),
            Err(VfsError::NotFound) => Err(MountError::NotUnsent),
            Err(_) => Err(MountError::Busy),
        }
    }

    /// The mount's core, and a guard that keeps the unmount from letting the spool go while this call runs.
    fn unsent_call(
        &self,
        provider: &str,
        account: &str,
        mount_id: &str,
    ) -> Result<(tokio::sync::OwnedRwLockReadGuard<()>, Arc<Vfs>), MountError> {
        let key = Key {
            provider: provider.to_string(),
            account: account.to_string(),
        };
        let table = self.table.lock().unwrap();
        let writes = table
            .entries
            .get(&key)
            .filter(|entry| entry.attempt.to_string() == mount_id)
            .and_then(|entry| match &entry.writing {
                Writing::Spooled(writes) => Some(writes),
                Writing::Off | Writing::ReadOnly(_) => None,
            })
            .ok_or(MountError::NotUnsent)?;
        let guard = Arc::clone(&writes.calls)
            .try_read_owned()
            .expect("the unmount turns writing off before it waits for the calls");
        Ok((guard, Arc::clone(&writes.vfs)))
    }

    /// Waits until no unsent file of `vfs` is waiting or uploading, or until `hurry`. A failed, a refused, and an open file do not hold it: a failed one waits for its retry, a refused one waits to be asked, and nothing can close an open one after the unmount.
    async fn finish_uploads(&self, vfs: &Vfs) {
        let mut hurry = self.hurry.subscribe();
        loop {
            let busy = vfs.unsent().iter().any(|file| {
                matches!(
                    file.state,
                    marlin_vfs::UnsentState::Waiting | marlin_vfs::UnsentState::Uploading { .. }
                )
            });
            if *hurry.borrow() || !busy {
                return;
            }
            tokio::select! {
                _ = tokio::time::sleep(UNSENT_EVERY) => {}
                _ = hurry.changed() => {}
            }
        }
    }

    /// The account's spool folder and the lock that keeps it this mount's alone.
    fn open_spool(&self, key: &Key) -> Result<(Spool, File), String> {
        if key.account.is_empty() {
            return Err("the account has no name".to_string());
        }
        let name = spool_folder_name(&key.account);
        let folder = self.spool.join(&key.provider).join(&name);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&folder)
            .map_err(|error| error.to_string())?;
        let found = std::fs::symlink_metadata(&folder).map_err(|error| error.to_string())?;
        if !found.file_type().is_dir() {
            return Err("the account's spool is not a folder".to_string());
        }
        // `mode` only applies to a folder this call makes. The spool holds the user's unsaved work, and the mount needs to write in it.
        if found.permissions().mode() & 0o777 != 0o700 {
            std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| error.to_string())?;
        }
        // The helper's own lock is per socket, so two helpers on two sockets could both recover and upload one spool.
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .mode(0o600)
            .open(folder.with_file_name(format!("{name}.lock")))
            .map_err(|error| error.to_string())?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err("another Marlin helper is using this account's spool".to_string());
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.to_string()),
        }
        let limit = Spool::default_limit(&folder).map_err(|error| error.to_string())?;
        Ok((
            Spool {
                folder,
                limit,
                stays_local: marlin_fuse::stays_local,
            },
            lock,
        ))
    }

    fn operation_lock(&self, key: &Key) -> Arc<tokio::sync::Mutex<()>> {
        let mut table = self.table.lock().unwrap();
        table.locks.entry(key.clone()).or_default().clone()
    }

    /// Applies how one provider call ended to its mount's health, and logs the text of an `Other`. The rules are in `docs/mount-helper.md`, under "Health" and "Logs and secrets".
    fn record(&self, key: &Key, attempt: Uuid, error: Option<&VfsError>) {
        let (to_log, changed) = {
            let mut table = self.table.lock().unwrap();
            let Some(entry) = table.entries.get_mut(key) else {
                return;
            };
            if entry.attempt != attempt {
                return;
            }
            let to_log = match error {
                Some(VfsError::Other(text))
                    if !entry.logged_other.as_ref().is_some_and(|(logged, at)| {
                        logged == text && at.elapsed() < LOG_SAME_TEXT_EVERY
                    }) =>
                {
                    entry.logged_other = Some((text.clone(), tokio::time::Instant::now()));
                    Some(text.clone())
                }
                _ => None,
            };
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
            let changed =
                if entry.health == before || !matches!(entry.lifecycle, Lifecycle::Mounted(_)) {
                    None
                } else {
                    Some(self.announce(key, entry))
                };
            (to_log, changed)
        };
        if let Some(text) = to_log {
            log::write(
                "warn",
                "mount.call_failed",
                serde_json::json!({
                    "provider": key.provider,
                    "account": key.account,
                    "message": redact(&text),
                }),
            );
        }
        if let Some(changed) = changed {
            log_state(&changed);
        }
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
        let _ = self.events.send(Notice::Mount(status.clone()));
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
        read_only: match &entry.writing {
            Writing::ReadOnly(reason) => Some(reason.clone()),
            Writing::Off | Writing::Spooled(_) => None,
        },
    }
}

/// Sends `unsentChanged` for one mount attempt when its unsent files differ from the last ones sent. Looks every [`UNSENT_EVERY`].
async fn watch_unsent(mounts: std::sync::Weak<Mounts>, key: Key, attempt: Uuid, vfs: Arc<Vfs>) {
    let mut last = UnsentFiles {
        provider: key.provider.clone(),
        account: key.account.clone(),
        mount_id: attempt.to_string(),
        files: Vec::new(),
        unplaced: Vec::new(),
    };
    loop {
        tokio::time::sleep(UNSENT_EVERY).await;
        let Some(mounts) = mounts.upgrade() else {
            return;
        };
        let report = unsent_files(&key, attempt, &vfs);
        if report != last {
            let _ = mounts.events.send(Notice::Unsent(report.clone()));
            last = report;
        }
    }
}

fn unsent_files(key: &Key, attempt: Uuid, vfs: &Vfs) -> UnsentFiles {
    UnsentFiles {
        provider: key.provider.clone(),
        account: key.account.clone(),
        mount_id: attempt.to_string(),
        files: vfs
            .unsent()
            .into_iter()
            .map(|file| UnsentFile {
                node: file.node.0,
                name: file.name,
                size: file.size,
                state: match file.state {
                    marlin_vfs::UnsentState::Open => UnsentState::Open,
                    marlin_vfs::UnsentState::Waiting => UnsentState::Waiting,
                    marlin_vfs::UnsentState::Uploading { sent } => UnsentState::Uploading { sent },
                    marlin_vfs::UnsentState::Failed(error) => UnsentState::Failed {
                        error: redact(&error.to_string()),
                    },
                    marlin_vfs::UnsentState::Refused(error) => UnsentState::Refused {
                        error: redact(&error.to_string()),
                    },
                },
            })
            .collect(),
        unplaced: vfs
            .unplaced_spool_files()
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
    }
}

/// The folder name of an account's spool, under `<spool>/<provider>/`.
///
/// The name must map each account to its own folder on a filesystem that ignores letter case, so upper-case letters are escaped too. The mount name cannot be used, because a later account can get the name of a removed one.
fn spool_folder_name(account: &str) -> String {
    let mut out = String::new();
    for (index, byte) in account.bytes().enumerate() {
        let plain = matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'@' | b'.' | b'_' | b'+' | b'-');
        let leading_dot = index == 0 && byte == b'.';
        if plain && !leading_dot {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::spool_folder_name;

    #[test]
    fn spool_folder_names_keep_accounts_apart() {
        assert_eq!(spool_folder_name("ana@example.com"), "ana@example.com");
        assert_eq!(spool_folder_name("Ana"), "%41na");
        assert_eq!(spool_folder_name(".x"), "%2Ex");
        assert_eq!(spool_folder_name("a/b"), "a%2Fb");
        assert_eq!(spool_folder_name("a%b"), "a%25b");
        assert_eq!(spool_folder_name("é"), "%C3%A9");
    }
}
