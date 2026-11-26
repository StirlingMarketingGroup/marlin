//! One saved SMB server as a mount. The mount takes writes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use marlin_vfs::{Attributes, Item, ItemId, Kind, Progress, Provider, Revision, Space, VfsError};
use serde::{de::DeserializeOwned, Serialize};

use crate::protocol::{
    error_codes, methods, CreateDirectoryParams, DeleteParams, FileMetadataResult, FileStamp,
    GetFileMetadataParams, ListSharesParams, ListSharesResult, Notice, ReadDirectoryParams,
    ReadDirectoryResult, ReadParams, ReadResult, RenameParams, SaveFileParams, ShareEntry,
    SmbCredentials, SpaceParams, SpaceResult, READ_MAX, SMBCLIENT_MISSING,
};
use crate::servers::{read_password, saved_server, CredentialsError};
use crate::sidecar::{CallError, Sidecar};

/// How many `marlin-smb` processes one mount runs at most for its calls. Each runs one call at a time, so it is also the mount's `max_calls` (decision 0006).
pub const PROCESSES: usize = 4;

/// How many `marlin-smb` processes one mount runs at most for the two byte calls. They run on processes of their own, so a slow upload never holds a process another call needs. It is also the mount's `max_uploads`, so the core never runs an upload that would wait for a process with no progress (and end it as stalled).
pub const UPLOAD_PROCESSES: usize = 4;

/// A removal of a temporary file on a dead network cannot hold an upload process forever.
const CLEANUP_LIMIT: Duration = Duration::from_secs(30);

/// One saved server's sign-in in this process. A caller keeps one per server for its whole run and gives it to every provider of that server, as the helper keeps one `marlin_gdrive::SavedSignIn` per Google account. So a sign-in the server refused stays refused across mounts.
pub struct SmbSignIn {
    host: String,
    servers: PathBuf,
    state: std::sync::Mutex<State>,
}

/// The sign-in as each request sends it.
///
/// Unread ── read ──► Untried ── the server answers ──► Accepted
///                       │ refused                          │ refused
///                       ▼                                  ▼
///                    Refused ◄─────────────────────────────┘
///
/// A request that finds Unread, Refused, or another saved account reads the sign-in again. One read back the same as the refused one is not sent. No answer from the server leaves it Untried.
enum State {
    /// Nothing read yet.
    Unread,
    /// Read, and the server has not answered a call that sent it.
    Untried(SmbCredentials),
    /// The server took it.
    Accepted(SmbCredentials),
    /// The server refused it. It is never sent again.
    Refused(SmbCredentials),
}

impl SmbSignIn {
    /// `host` is the server as the user saved it, in `servers` (the saved servers file). Reads nothing: the first call does.
    pub fn new(host: &str, servers: PathBuf) -> Self {
        Self {
            host: host.to_string(),
            servers,
            state: std::sync::Mutex::new(State::Unread),
        }
    }

    /// Runs `send` with the saved sign-in, as the protocol takes it, and learns from its answer.
    pub(crate) fn send<T>(
        &self,
        send: impl FnOnce(SmbCredentials) -> Result<T, VfsError>,
    ) -> Result<T, VfsError> {
        // Read at every request: the user may save another user name or domain for the server at any time, and the old account may still work on the server. The same account keeps its password, so this reads no keychain.
        let saved = saved_server(&self.servers, &self.host);
        let saved_account = |held: &SmbCredentials| {
            saved.as_ref().is_ok_and(|server| {
                server.username == held.username && server.domain == held.domain
            })
        };
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        // Accepted: every call sends at once, and only a refusal changes the state.
        if let State::Accepted(held) = &*state {
            if saved_account(held) {
                let held = held.clone();
                drop(state);
                let answer = send(held.clone());
                if matches!(answer, Err(VfsError::SignInNeeded)) {
                    *self.state.lock().unwrap_or_else(|err| err.into_inner()) =
                        State::Refused(held);
                }
                return answer;
            }
        }

        // Any other state: one call at a time sends, and holds the lock until the server answers.
        let held = match &*state {
            State::Untried(held) if saved_account(held) => held.clone(),
            // Nothing read yet, a refusal, or another saved account.
            _ => {
                // The keychain, not the copy `servers.rs` keeps for the app: that copy could be one the server refused, and its lock is held across a read for any server, so a macOS prompt for one server would hold up the others.
                let read = saved.and_then(|server| {
                    Ok(SmbCredentials {
                        password: read_password(&server.hostname)?,
                        hostname: self.host.clone(),
                        username: server.username,
                        domain: server.domain,
                    })
                });
                let read = match read {
                    Ok(read) => read,
                    Err(CredentialsError::NotSaved(_)) => return Err(VfsError::SignInNeeded),
                    Err(CredentialsError::Failed(_)) => {
                        return Err(VfsError::Other(
                            "Marlin could not read the saved SMB sign-in".into(),
                        ));
                    }
                };
                if matches!(&*state, State::Refused(refused) if *refused == read) {
                    return Err(VfsError::SignInNeeded);
                }
                read
            }
        };

        // Four calls with a wrong new password cost one refusal, not four.
        let answer = send(held.clone());
        *state = match &answer {
            Err(VfsError::SignInNeeded) => State::Refused(held),
            // These answers come after the server took the sign-in.
            Ok(_)
            | Err(
                VfsError::NotFound
                | VfsError::AccessDenied
                | VfsError::IsAFolder
                | VfsError::NotAFolder
                | VfsError::Conflict
                | VfsError::NotEmpty
                | VfsError::NoSpace,
            ) => State::Accepted(held),
            // No answer from the server.
            Err(_) => State::Untried(held),
        };
        answer
    }
}

/// One saved SMB server as a provider for a native mount (decision 0006). It takes writes. Its shares are the top folders.
pub struct SmbProvider {
    shared: Arc<Shared>,
    calls: Pool,
    uploads: Pool,
}

/// One pool of `marlin-smb` processes. A call holds one permit and one process until it ends.
struct Pool {
    permits: Arc<tokio::sync::Semaphore>,
    /// The processes no call holds. `Sidecar::new` starts nothing, so a process starts only when a call needs one.
    idle: Arc<std::sync::Mutex<Vec<Sidecar>>>,
}

impl Pool {
    fn new(size: usize, binary: &Option<PathBuf>) -> Self {
        let idle = (0..size).map(|_| Sidecar::new(binary.clone())).collect();
        Self {
            permits: Arc::new(tokio::sync::Semaphore::new(size)),
            idle: Arc::new(std::sync::Mutex::new(idle)),
        }
    }
}

/// What every call's blocking thread shares.
struct Shared {
    sign_in: Arc<SmbSignIn>,
    /// Temporary files a removal could not delete. The next upload tries them again. One left when the provider ends stays on the server.
    strays: std::sync::Mutex<Vec<Stray>>,
}

/// A temporary file whose removal failed. A process stopped after it made the file but before the `staged` notice arrived leaves a file the provider never hears of, and that file is not here.
struct Stray {
    share: String,
    path: String,
}

/// One process, held by one call until the call ends.
struct Lease {
    shared: Arc<Shared>,
    /// The pool this process goes back to.
    idle: Arc<std::sync::Mutex<Vec<Sidecar>>>,
    sidecar: Option<Sidecar>,
    /// Set when the core drops the call. Every request of the call passes it to `Sidecar::call_until`.
    stop: Arc<AtomicBool>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

/// Sets the call's stop flag when the core drops the call's future.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(sidecar) = self.sidecar.take() {
            self.idle
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .push(sidecar);
        }
    }
}

impl Lease {
    /// The server's disk shares, as `list_shares` answers.
    fn shares(&self) -> Result<Vec<ShareEntry>, VfsError> {
        let listed: ListSharesResult = self.request(methods::LIST_SHARES, |credentials| {
            ListSharesParams { credentials }
        })?;
        Ok(listed.shares)
    }

    /// One request on this process, signed in with the saved server's sign-in.
    fn request<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: impl FnOnce(SmbCredentials) -> P,
    ) -> Result<R, VfsError> {
        self.request_hearing(method, params, &mut |_| {})
    }

    /// [`Self::request`], and `heard` hears each notice of this request.
    fn request_hearing<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: impl FnOnce(SmbCredentials) -> P,
        heard: &mut dyn FnMut(Notice),
    ) -> Result<R, VfsError> {
        let sidecar = self.sidecar.as_ref().expect("held until the lease drops");
        // A missing `marlin-smb`, or one that cannot load Samba, says so before the keychain is read.
        sidecar
            .start()
            .map_err(|start| vfs_error(CallError::Unavailable(start)))?;
        self.shared.sign_in.send(|credentials| {
            sidecar
                .call_with_notices(method, params(credentials), &self.stop, heard)
                .map_err(vfs_error)
        })
    }

    /// Removes an upload's temporary file. The call's own stop flag is set when the core dropped it, and a request that sees it sends nothing, so this uses a fresh flag. `Ok` or `NotFound` means the file is gone.
    fn remove_temporary(&self, share: &str, path: &str) -> bool {
        let sidecar = self.sidecar.as_ref().expect("held until the lease drops");
        let removed = self.shared.sign_in.send(|credentials| {
            sidecar
                .call_until(
                    methods::DELETE,
                    DeleteParams {
                        credentials,
                        share: share.to_string(),
                        path: path.to_string(),
                    },
                    Some(CLEANUP_LIMIT),
                    &AtomicBool::new(false),
                )
                .map_err(vfs_error)
        });
        matches!(removed, Ok(()) | Err(VfsError::NotFound))
    }

    /// Tries again to remove every temporary file an earlier upload left behind.
    fn clear_strays(&self) {
        let pending = std::mem::take(
            &mut *self
                .shared
                .strays
                .lock()
                .unwrap_or_else(|err| err.into_inner()),
        );
        let mut failed = Vec::new();
        for stray in pending {
            if !self.remove_temporary(&stray.share, &stray.path) {
                failed.push(stray);
            }
        }
        if failed.is_empty() {
            return;
        }
        self.shared
            .strays
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .extend(failed);
    }
}

impl SmbProvider {
    /// `binary` is the `marlin-smb` file to run, None when it is missing. Reads nothing and starts no process: a call starts one when it needs it.
    pub fn new(sign_in: Arc<SmbSignIn>, binary: Option<PathBuf>) -> Self {
        Self {
            shared: Arc::new(Shared {
                sign_in,
                strays: std::sync::Mutex::new(Vec::new()),
            }),
            calls: Pool::new(PROCESSES, &binary),
            uploads: Pool::new(UPLOAD_PROCESSES, &binary),
        }
    }

    /// Takes one process from `pool`, runs `run` on it, and puts the process back when `run` ends.
    ///
    /// The core limits every call and drops this future when the limit passes, waiting included. The guard turns that drop into the stop flag, so the process the call uses ends when the core gives up. An upload has no time limit: the core drops it when progress stops moving.
    async fn on_process<T: Send + 'static>(
        &self,
        pool: &Pool,
        run: impl FnOnce(&Lease) -> Result<T, VfsError> + Send + 'static,
    ) -> Result<T, VfsError> {
        // No time limit here: the core's covers the wait for a free process.
        let permit = pool
            .permits
            .clone()
            .acquire_owned()
            .await
            .expect("never closed");
        let sidecar = pool
            .idle
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .pop()
            .expect("a permit means one is there");
        let lease = Lease {
            shared: Arc::clone(&self.shared),
            idle: Arc::clone(&pool.idle),
            sidecar: Some(sidecar),
            stop: Arc::new(AtomicBool::new(false)),
            _permit: permit,
        };
        let _stop_on_drop = StopOnDrop(Arc::clone(&lease.stop));
        tokio::task::spawn_blocking(move || run(&lease))
            .await
            .unwrap_or_else(|_| Err(VfsError::Other("an SMB call stopped unexpectedly".into())))
    }

    /// Sends `from` to `path` in `share`. `base` of `None` makes a new file, and `Some` replaces the file that still has that revision. `progress` hears the bytes sent, in order.
    async fn upload(
        &self,
        share: String,
        path: String,
        from: &Path,
        modified: SystemTime,
        base: Option<Revision>,
        progress: &Progress,
    ) -> Result<FileMetadataResult, VfsError> {
        let replaces = base.as_ref().map(stamp_of).transpose()?;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let from = from.to_path_buf();
        let modified = chrono::DateTime::<chrono::Utc>::from(modified)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let work = self.on_process(&self.uploads, move |lease| {
            lease.clear_strays();
            // An untried sign-in holds `SmbSignIn`'s lock for the whole call that sends it, so that call is this short lookup, never the upload.
            let current: Result<FileMetadataResult, VfsError> =
                lease.request(methods::GET_FILE_METADATA, |credentials| {
                    GetFileMetadataParams {
                        credentials,
                        share: share.clone(),
                        path: path.clone(),
                    }
                });
            match (&base, current) {
                (None, Ok(_)) => return Err(VfsError::Conflict),
                (None, Err(VfsError::NotFound)) => {}
                (None, Err(error)) => return Err(error),
                (Some(_), Ok(meta)) if meta.is_directory => return Err(VfsError::IsAFolder),
                (Some(base), Ok(meta))
                    if revision_of(meta.size, self::modified(&meta.modified)) != *base =>
                {
                    return Err(VfsError::Conflict);
                }
                (Some(_), Ok(_)) => {}
                (Some(_), Err(error)) => return Err(error),
            }

            static NEXT: AtomicU64 = AtomicU64::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let temporary = join_name(
                parent_of(&path),
                &format!(".marlin-{}-{nanos}-{n}", std::process::id()),
            );
            let mut staged = false;
            let saved = lease.request_hearing(
                methods::SAVE_FILE,
                |credentials| SaveFileParams {
                    credentials,
                    share: share.clone(),
                    source_path: from.to_string_lossy().into_owned(),
                    path,
                    temporary_path: temporary.clone(),
                    modified,
                    replaces,
                },
                &mut |notice| match notice {
                    Notice::Staged { .. } => staged = true,
                    Notice::Progress { bytes, .. } => {
                        let _ = tx.send(bytes);
                    }
                },
            );
            // From the staged notice on, a failed upload removes its temporary file, and it does that before it lets the process go.
            if saved.is_err() && staged && !lease.remove_temporary(&share, &temporary) {
                lease
                    .shared
                    .strays
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .push(Stray {
                        share,
                        path: temporary,
                    });
            }
            saved
        });
        let mut work = std::pin::pin!(work);
        let mut hearing = true;
        loop {
            tokio::select! {
                biased;
                bytes = rx.recv(), if hearing => {
                    match bytes {
                        Some(bytes) => progress(bytes),
                        None => hearing = false,
                    }
                }
                result = &mut work => {
                    while let Ok(bytes) = rx.try_recv() {
                        progress(bytes);
                    }
                    return result;
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl Provider for SmbProvider {
    fn root(&self) -> ItemId {
        ItemId("/".to_string())
    }

    fn can_write(&self) -> bool {
        true
    }

    fn ids_are_paths(&self) -> bool {
        true
    }

    async fn item(&self, id: &ItemId) -> Result<Item, VfsError> {
        let id = id.clone();
        self.on_process(&self.calls, move |lease| {
            let name = id.0.rsplit('/').next().unwrap_or_default().to_string();
            let (share, path) = match place(&id) {
                // The core asks `item` for the top folder's attributes whenever the OS asks and its last answer is a few seconds old, and the helper reads every call's result as news about the server's health. So the server is asked: an answer made up without it would mark an offline mount as up.
                Place::Server => {
                    lease.shares()?;
                    return Ok(timeless_folder(id, name));
                }
                // What `list_shares` says, as in the top folder's listing: a share the user may not open is still a folder there, whose listing is refused.
                Place::Share(share) => {
                    if !lease.shares()?.iter().any(|listed| listed.name == share) {
                        return Err(VfsError::NotFound);
                    }
                    return Ok(timeless_folder(id, name));
                }
                Place::Entry { share, path } => (share.to_string(), path.to_string()),
            };
            let meta: FileMetadataResult =
                lease.request(methods::GET_FILE_METADATA, |credentials| {
                    GetFileMetadataParams {
                        credentials,
                        share,
                        path,
                    }
                })?;
            Ok(item_from(
                id,
                name,
                meta.is_directory,
                meta.size,
                modified(&meta.modified),
            ))
        })
        .await
    }

    async fn list(&self, folder: &ItemId) -> Result<Vec<Item>, VfsError> {
        let folder = folder.clone();
        self.on_process(&self.calls, move |lease| {
            let (share, path) = match place(&folder) {
                Place::Server => {
                    return Ok(lease
                        .shares()?
                        .into_iter()
                        .map(|share| timeless_folder(child(&folder, &share.name), share.name))
                        .collect());
                }
                Place::Share(share) => (share, "/"),
                Place::Entry { share, path } => (share, path),
            };
            let listed: ReadDirectoryResult =
                lease.request(methods::READ_DIRECTORY, |credentials| ReadDirectoryParams {
                    credentials,
                    share: share.to_string(),
                    path: path.to_string(),
                })?;
            // Dot names stay. The OS hides them itself, so `is_hidden` is not a reason to leave one out.
            Ok(listed
                .entries
                .into_iter()
                .map(|entry| {
                    item_from(
                        child(&folder, &entry.name),
                        entry.name,
                        entry.is_directory,
                        entry.size,
                        modified(&entry.modified),
                    )
                })
                .collect())
        })
        .await
    }

    async fn read(&self, id: &ItemId, offset: u64, length: u32) -> Result<Vec<u8>, VfsError> {
        let (share, path) = match place(id) {
            Place::Server | Place::Share(_) => return Err(VfsError::IsAFolder),
            Place::Entry { share, path } => (share.to_string(), path.to_string()),
        };
        if length == 0 {
            return Ok(Vec::new());
        }
        self.on_process(&self.calls, move |lease| {
            // `marlin-smb` takes at most `READ_MAX` bytes per request, and an adapter's read passes its own size straight through.
            let mut bytes = Vec::new();
            loop {
                let asked = (length - bytes.len() as u32).min(READ_MAX);
                let piece: ReadResult = lease.request(methods::READ, |credentials| ReadParams {
                    credentials,
                    share: share.clone(),
                    path: path.clone(),
                    offset: offset + bytes.len() as u64,
                    length: asked,
                })?;
                // A piece that comes back short is the end of the file.
                let short = piece.data.len() < asked as usize;
                bytes.extend(piece.data);
                if short || bytes.len() >= length as usize {
                    return Ok(bytes);
                }
            }
        })
        .await
    }

    async fn space(&self) -> Result<Space, VfsError> {
        self.on_process(&self.calls, |lease| {
            // A mount reports one size for all its shares. Adding them up would count a volume that several shares sit on more than once.
            for share in lease.shares()? {
                match lease.request(methods::SPACE, |credentials| SpaceParams {
                    credentials,
                    share: share.name,
                }) {
                    Ok(Some(SpaceResult { total, free })) => return Ok(Space { total, free }),
                    // A refused sign-in, or a server out of reach, is the whole server's.
                    Err(
                        error @ (VfsError::SignInNeeded | VfsError::Offline | VfsError::TimedOut),
                    ) => {
                        return Err(error);
                    }
                    // A share that reports no size, or cannot answer, such as one the user may not open, is skipped.
                    Ok(None) | Err(_) => {}
                }
            }
            // A mount must report some number. 0 free would make file managers refuse to copy. This is 1 PiB, as the Google Drive provider answers with no limit.
            let bytes = 1 << 50;
            Ok(Space {
                total: bytes,
                free: bytes,
            })
        })
        .await
    }

    async fn create_folder(&self, parent: &ItemId, name: &str) -> Result<Item, VfsError> {
        check_name(name)?;
        if matches!(place(parent), Place::Server) {
            return Err(VfsError::NotSupported);
        }
        let parent = parent.clone();
        let name = name.to_string();
        self.on_process(&self.calls, move |lease| {
            let id = child(&parent, &name);
            let (share, path) = share_path(&id)?;
            let () = lease.request(methods::CREATE_DIRECTORY, |credentials| {
                CreateDirectoryParams {
                    credentials,
                    share: share.clone(),
                    path: path.clone(),
                }
            })?;
            let meta: FileMetadataResult =
                lease.request(methods::GET_FILE_METADATA, |credentials| {
                    GetFileMetadataParams {
                        credentials,
                        share,
                        path,
                    }
                })?;
            Ok(item_from(
                id,
                name,
                meta.is_directory,
                meta.size,
                modified(&meta.modified),
            ))
        })
        .await
    }

    async fn remove(&self, id: &ItemId, parent: &ItemId) -> Result<(), VfsError> {
        if !matches!(place(id), Place::Entry { .. }) {
            return Err(VfsError::NotSupported);
        }
        if parent_of(&id.0) != parent.0 {
            return Err(VfsError::NotFound);
        }
        let id = id.clone();
        self.on_process(&self.calls, move |lease| {
            let (share, path) = share_path(&id)?;
            let () = lease.request(methods::DELETE, |credentials| DeleteParams {
                credentials,
                share,
                path,
            })?;
            Ok(())
        })
        .await
    }

    async fn rename(
        &self,
        id: &ItemId,
        from: &ItemId,
        to: &ItemId,
        name: &str,
        replace: Option<&ItemId>,
    ) -> Result<Item, VfsError> {
        check_name(name)?;
        let id_share = match place(id) {
            Place::Entry { share, .. } if parent_of(&id.0) == from.0 => share,
            Place::Entry { .. } => return Err(VfsError::NotFound),
            Place::Server | Place::Share(_) => return Err(VfsError::NotSupported),
        };
        let to_share = match place(to) {
            Place::Share(share) | Place::Entry { share, .. } => share,
            Place::Server => return Err(VfsError::NotSupported),
        };
        if to_share != id_share {
            return Err(VfsError::NotSupported);
        }
        let new_id = match replace {
            Some(replaced) => replaced.clone(),
            None => child(to, name),
        };
        // A folder moved into itself, or into a folder inside it.
        if new_id.0.starts_with(&format!("{}/", id.0)) {
            return Err(VfsError::NotSupported);
        }
        if let Some(replaced) = replace {
            if parent_of(&replaced.0) != to.0 {
                return Err(VfsError::NotFound);
            }
        }
        let id = id.clone();
        let name = name.to_string();
        let replace = replace.cloned();
        self.on_process(&self.calls, move |lease| {
            let (share, from_path) = share_path(&id)?;
            let (published, published_path) = if let Some(replaced) = replace {
                let (_, replaced_path) = share_path(&replaced)?;
                // A replace target that is gone answers `NotFound` here, and nothing is renamed.
                let _: FileMetadataResult =
                    lease.request(methods::GET_FILE_METADATA, |credentials| {
                        GetFileMetadataParams {
                            credentials,
                            share: share.clone(),
                            path: replaced_path.clone(),
                        }
                    })?;
                let () = lease.request(methods::RENAME, |credentials| RenameParams {
                    credentials,
                    share: share.clone(),
                    from_path,
                    to_path: replaced_path.clone(),
                    replace: true,
                })?;
                // On a share that ignores case, `replaced` can be spelled differently from `name`. The item now lives at `replaced`.
                (replaced, replaced_path)
            } else {
                let (_, to_path) = share_path(&new_id)?;
                let () = lease.request(methods::RENAME, |credentials| RenameParams {
                    credentials,
                    share: share.clone(),
                    from_path,
                    to_path: to_path.clone(),
                    replace: false,
                })?;
                (new_id, to_path)
            };
            let meta: FileMetadataResult =
                lease.request(methods::GET_FILE_METADATA, |credentials| {
                    GetFileMetadataParams {
                        credentials,
                        share,
                        path: published_path,
                    }
                })?;
            let published_name = published.0.rsplit('/').next().unwrap_or(&name).to_string();
            Ok(item_from(
                published,
                published_name,
                meta.is_directory,
                meta.size,
                modified(&meta.modified),
            ))
        })
        .await
    }

    async fn create_file(
        &self,
        parent: &ItemId,
        name: &str,
        from: &Path,
        modified: SystemTime,
        progress: &Progress,
    ) -> Result<Item, VfsError> {
        check_name(name)?;
        if matches!(place(parent), Place::Server) {
            return Err(VfsError::NotSupported);
        }
        let id = child(parent, name);
        let (share, path) = share_path(&id)?;
        let meta = self
            .upload(share, path, from, modified, None, progress)
            .await?;
        Ok(item_from(
            id,
            name.to_string(),
            meta.is_directory,
            meta.size,
            self::modified(&meta.modified),
        ))
    }

    async fn replace_bytes(
        &self,
        id: &ItemId,
        from: &Path,
        base: &Revision,
        modified: SystemTime,
        progress: &Progress,
    ) -> Result<Item, VfsError> {
        if matches!(place(id), Place::Server | Place::Share(_)) {
            return Err(VfsError::IsAFolder);
        }
        let (share, path) = share_path(id)?;
        let name = id.0.rsplit('/').next().unwrap_or_default().to_string();
        let meta = self
            .upload(share, path, from, modified, Some(base.clone()), progress)
            .await?;
        Ok(item_from(
            id.clone(),
            name,
            meta.is_directory,
            meta.size,
            self::modified(&meta.modified),
        ))
    }
}

/// What an item id names. An SMB name never holds `/`, so an id splits one way only.
enum Place<'a> {
    /// `/`: the server, the top of the mount.
    Server,
    /// `/{share}`: a share's top folder.
    Share(&'a str),
    /// `/{share}/{path}`: an item in a share. `path` starts with the `/` of the share's top, as `marlin-smb` takes it.
    Entry { share: &'a str, path: &'a str },
}

fn place(id: &ItemId) -> Place<'_> {
    let rest = id.0.strip_prefix('/').unwrap_or(&id.0);
    if rest.is_empty() {
        return Place::Server;
    }
    match rest.find('/') {
        None => Place::Share(rest),
        Some(slash) => Place::Entry {
            share: &rest[..slash],
            path: &rest[slash..],
        },
    }
}

fn child(folder: &ItemId, name: &str) -> ItemId {
    ItemId(join_name(&folder.0, name))
}

/// The folder that holds `path`: an id, or a path inside a share, which have the same shape. `/` for an item at the top.
fn parent_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) | None => "/",
        Some(at) => &path[..at],
    }
}

/// The share and the path inside it. A share's own id is the path `/`.
fn share_path(id: &ItemId) -> Result<(String, String), VfsError> {
    match place(id) {
        Place::Entry { share, path } => Ok((share.to_string(), path.to_string())),
        Place::Share(share) => Ok((share.to_string(), "/".to_string())),
        Place::Server => Err(VfsError::NotSupported),
    }
}

/// `name` in `folder`: an id, or a path inside a share.
fn join_name(folder: &str, name: &str) -> String {
    if folder == "/" {
        format!("/{name}")
    } else {
        format!("{folder}/{name}")
    }
}

/// A name SMB cannot hold. Samba answers some of these with a generic error, which would count against the whole mount.
fn check_name(name: &str) -> Result<(), VfsError> {
    let refused = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
    if name.is_empty()
        || name == "."
        || name == ".."
        || refused.iter().any(|character| name.contains(*character))
        || name
            .chars()
            .any(|character| character < '\u{0020}' || character == '\u{FFFD}')
    {
        Err(VfsError::NotSupported)
    } else {
        Ok(())
    }
}

/// A file's revision: its size and its modified time in whole seconds, the same seconds `FileStamp` compares.
fn revision_of(size: u64, modified: SystemTime) -> Revision {
    let seconds = chrono::DateTime::<chrono::Utc>::from(modified).timestamp();
    Revision(format!("{size}:{seconds}"))
}

/// The stamp a revision names. A base this provider did not make does not parse, and that is a `Conflict`.
fn stamp_of(base: &Revision) -> Result<FileStamp, VfsError> {
    let Some((size, seconds)) = base.0.split_once(':') else {
        return Err(VfsError::Conflict);
    };
    Ok(FileStamp {
        size: size.parse().map_err(|_| VfsError::Conflict)?,
        modified: seconds.parse().map_err(|_| VfsError::Conflict)?,
    })
}

/// The server, or a share. `list_shares` gives no times, so a share is a folder from the start of 1970, whether listed or asked for alone.
fn timeless_folder(id: ItemId, name: String) -> Item {
    item_from(id, name, true, 0, UNIX_EPOCH)
}

/// A time as `marlin-smb` writes it. One that does not parse is the start of 1970.
fn modified(time: &str) -> SystemTime {
    chrono::DateTime::parse_from_rfc3339(time).map_or(UNIX_EPOCH, SystemTime::from)
}

fn item_from(
    id: ItemId,
    name: String,
    is_directory: bool,
    size: u64,
    modified: SystemTime,
) -> Item {
    Item {
        id,
        name,
        attributes: Attributes {
            kind: if is_directory {
                Kind::Folder
            } else {
                Kind::File
            },
            size: if is_directory { 0 } else { size },
            modified,
            revision: if is_directory {
                None
            } else {
                Some(revision_of(size, modified))
            },
            read_only: false,
        },
    }
}

fn vfs_error(error: CallError) -> VfsError {
    match error {
        CallError::Unavailable(start) => VfsError::Other(start.to_string()),
        CallError::TimedOut | CallError::Stopped => VfsError::TimedOut,
        // A process that dies in the middle of a call has crashed. A lost network answers `SMB_CONNECTION_LOST`, which is `Offline`.
        CallError::Lost => VfsError::Other("marlin-smb stopped during a call".into()),
        CallError::BadReply(_) | CallError::Encoding(_) => {
            VfsError::Other("marlin-smb sent an answer Marlin could not read".into())
        }
        CallError::Failed { code, .. } => match code {
            error_codes::SMB_AUTH_FAILED => VfsError::SignInNeeded,
            error_codes::SMB_PATH_NOT_FOUND => VfsError::NotFound,
            error_codes::SMB_PERMISSION_DENIED => VfsError::AccessDenied,
            error_codes::SMB_CONNECTION_LOST => VfsError::Offline,
            error_codes::SMB_IS_A_FOLDER => VfsError::IsAFolder,
            error_codes::SMB_NOT_A_FOLDER => VfsError::NotAFolder,
            error_codes::SMB_ALREADY_EXISTS => VfsError::Conflict,
            error_codes::SMB_NOT_EMPTY => VfsError::NotEmpty,
            error_codes::SMB_CHANGED => VfsError::Conflict,
            error_codes::SMB_NO_SPACE => VfsError::NoSpace,
            // A name `marlin-smb` cannot send fails this item. It is not a failure of the mount.
            error_codes::INVALID_PARAMS => VfsError::NotSupported,
            error_codes::SMB_LIBRARY_MISSING => VfsError::Other(SMBCLIENT_MISSING.to_string()),
            code => VfsError::Other(format!("marlin-smb answered error code {code}")),
        },
    }
}
