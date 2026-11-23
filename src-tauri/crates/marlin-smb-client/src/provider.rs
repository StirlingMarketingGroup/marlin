//! One saved SMB server as a read-only mount.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use marlin_vfs::{Attributes, Item, ItemId, Kind, Provider, Space, VfsError};
use serde::{de::DeserializeOwned, Serialize};

use crate::protocol::{
    error_codes, methods, FileMetadataResult, GetFileMetadataParams, ListSharesParams,
    ListSharesResult, ReadDirectoryParams, ReadDirectoryResult, ReadParams, ReadResult, ShareEntry,
    SmbCredentials, SpaceParams, SpaceResult, READ_MAX, SMBCLIENT_MISSING,
};
use crate::servers::{read_password, saved_server, CredentialsError};
use crate::sidecar::{CallError, Sidecar};

/// How many `marlin-smb` processes one mount runs at most. Each runs one call at a time, so it is also the mount's `max_calls` (decision 0006).
pub const PROCESSES: usize = 4;

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
                | VfsError::NotAFolder,
            ) => State::Accepted(held),
            // No answer from the server.
            Err(_) => State::Untried(held),
        };
        answer
    }
}

/// One saved SMB server as a read-only provider for a native mount (decision 0006). Its shares are the top folders.
pub struct SmbProvider {
    shared: Arc<Shared>,
    /// One permit per process. A call holds one while it holds a process.
    permits: Arc<tokio::sync::Semaphore>,
}

/// What a call's blocking thread needs.
struct Shared {
    sign_in: Arc<SmbSignIn>,
    /// The processes no call holds. `Sidecar::new` starts nothing, so a process starts only when a call needs one.
    idle: std::sync::Mutex<Vec<Sidecar>>,
}

/// One process, held by one call until the call ends.
struct Lease {
    shared: Arc<Shared>,
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
            self.shared
                .idle
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
        let sidecar = self.sidecar.as_ref().expect("held until the lease drops");
        // A missing `marlin-smb`, or one that cannot load Samba, says so before the keychain is read.
        sidecar
            .start()
            .map_err(|start| vfs_error(CallError::Unavailable(start)))?;
        self.shared.sign_in.send(|credentials| {
            sidecar
                .call_until(method, params(credentials), None, &self.stop)
                .map_err(vfs_error)
        })
    }
}

impl SmbProvider {
    /// `binary` is the `marlin-smb` file to run, None when it is missing. Reads nothing and starts no process: a call starts one when it needs it.
    pub fn new(sign_in: Arc<SmbSignIn>, binary: Option<PathBuf>) -> Self {
        let idle = (0..PROCESSES)
            .map(|_| Sidecar::new(binary.clone()))
            .collect();
        Self {
            shared: Arc::new(Shared {
                sign_in,
                idle: std::sync::Mutex::new(idle),
            }),
            permits: Arc::new(tokio::sync::Semaphore::new(PROCESSES)),
        }
    }

    /// Takes one process, runs `run` on it, and puts the process back when `run` ends.
    ///
    /// The core limits every call and drops this future when the limit passes, waiting included. The guard turns that drop into the stop flag, so the process the call uses ends when the core gives up.
    async fn on_process<T: Send + 'static>(
        &self,
        run: impl FnOnce(&Lease) -> Result<T, VfsError> + Send + 'static,
    ) -> Result<T, VfsError> {
        // No time limit here: the core's covers the wait for a free process.
        let permit = self
            .permits
            .clone()
            .acquire_owned()
            .await
            .expect("never closed");
        let sidecar = self
            .shared
            .idle
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .pop()
            .expect("a permit means one is there");
        let lease = Lease {
            shared: Arc::clone(&self.shared),
            sidecar: Some(sidecar),
            stop: Arc::new(AtomicBool::new(false)),
            _permit: permit,
        };
        let _stop_on_drop = StopOnDrop(Arc::clone(&lease.stop));
        tokio::task::spawn_blocking(move || run(&lease))
            .await
            .unwrap_or_else(|_| Err(VfsError::Other("an SMB call stopped unexpectedly".into())))
    }
}

#[async_trait::async_trait]
impl Provider for SmbProvider {
    fn root(&self) -> ItemId {
        ItemId("/".to_string())
    }

    async fn item(&self, id: &ItemId) -> Result<Item, VfsError> {
        let id = id.clone();
        self.on_process(move |lease| {
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
        self.on_process(move |lease| {
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
        self.on_process(move |lease| {
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
        self.on_process(|lease| {
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
    if folder.0 == "/" {
        ItemId(format!("/{name}"))
    } else {
        ItemId(format!("{}/{name}", folder.0))
    }
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
            revision: None,
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
            error_codes::SMB_LIBRARY_MISSING => VfsError::Other(SMBCLIENT_MISSING.to_string()),
            code => VfsError::Other(format!("marlin-smb answered error code {code}")),
        },
    }
}
