use std::future::Future;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::future::{BoxFuture, FutureExt, Shared};
use futures_util::stream::{self, StreamExt, TryStreamExt};
use marlin_vfs::{Attributes, Item, ItemId, Kind, Progress, Provider, Revision, Space, VfsError};
use russh::client;
use russh::keys::PublicKey;
use russh::ChannelStream;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::RawSftpSession;
use russh_sftp::extensions::{LIMITS, STATVFS};
use russh_sftp::protocol::{FileAttributes, FileType, Handle, OpenFlags, Packet, StatusCode};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use crate::connect::{open_sftp_channel, ConnectError, SshHandler};
use crate::servers::{server_credentials, SignIn};

/// How many calls one SFTP mount runs at once: the helper sets it as the mount's `Options::max_calls`.
///
/// The app downloads one file at a time per server, because some servers fail on parallel reads. The mount core's default of 16 is for Google Drive, and one read here already keeps [`IN_FLIGHT`] requests going. Nobody has measured how fast either number is.
pub const MAX_CALLS: usize = 4;

/// How many requests one call keeps going at once: the pieces of a read, or the link lookups of a listing.
const IN_FLIGHT: usize = 8;

/// The piece a read asks for from a server without `limits@openssh.com`. Every server takes 32 KiB.
const SMALL_READ: u32 = 32 * 1024;

/// The piece a write sends to a server without `limits@openssh.com`. Every server takes 32 KiB.
const SMALL_WRITE: u32 = 32 * 1024;

const COULD_NOT_UPLOAD: &str = "the file to upload could not be read";

/// How long a connection attempt may take from the TCP connect to the SFTP channel: past the core's 30-second call limit, and long enough for the user to answer an SSH agent's prompt.
const SIGN_IN_LIMIT: Duration = Duration::from_secs(60);

/// The free space when the server cannot tell. Zero would make file managers refuse to copy, so this is 1 PiB, as the Google Drive provider answers when Google gives no limit. The usage is unknown too, so the total is the same.
const UNKNOWN_SPACE: Space = Space {
    total: 1 << 50,
    free: 1 << 50,
};

const COULD_NOT_START: &str = "the SFTP connection could not start";
const COULD_NOT_READ: &str = "the SFTP server sent an answer Marlin could not read";

/// Where the saved server and its host keys are, and how long a failed attempt waits.
#[derive(Clone)]
pub struct Config {
    /// The saved servers file: `servers_path()` in the helper.
    pub servers: PathBuf,

    /// `known_hosts`: `known_hosts_path()` in the helper.
    pub known_hosts: PathBuf,

    /// How long a refused key or agent sign-in, or an attempt that failed before the sign-in, waits before it is made again. A refused password is never sent again. The helper uses 60 seconds.
    pub retry_after: Duration,
}

/// One account, a user on a saved server's host and port, with its failed sign-in attempts. A caller keeps one per account for the life of its process and gives it to every provider of that account, so a refused sign-in stays refused across mounts. It holds no connection.
pub struct SftpAccount {
    username: String,
    hostname: String,
    port: u16,
    config: Config,
    /// The attempts the server refused, and those that failed before it judged the sign-in. An attempt holds this lock from its check through the recording of its result, so a second attempt waits and then sees that result. A connection attempt runs in a task of its own and outlives the call that started it.
    failed_attempts: tokio::sync::Mutex<Vec<FailedAttempt>>,
}

impl SftpAccount {
    pub fn new(username: &str, hostname: &str, port: u16, config: Config) -> Self {
        Self {
            username: username.to_string(),
            hostname: hostname.to_string(),
            port,
            config,
            failed_attempts: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    /// Signs in and opens the SFTP channel. It reads the saved server each time, and the keychain for a password or a key's passphrase, so a changed sign-in works with no unmount.
    pub(crate) async fn sign_in(
        &self,
    ) -> Result<(client::Handle<SshHandler>, ChannelStream<client::Msg>), VfsError> {
        let mut failed_attempts = self.failed_attempts.lock().await;
        let servers = self.config.servers.clone();
        let hostname = self.hostname.clone();
        let port = self.port;
        let known_hosts = self.config.known_hosts.clone();
        let username = self.username.clone();
        // A keychain read can wait on a prompt.
        let (creds, host_keys) = tokio::task::spawn_blocking(move || {
            let creds = server_credentials(&servers, &hostname, port)
                .map_err(|_| VfsError::SignInNeeded)?;
            // The account names this user, so a saved server that now holds another user is not signed in as.
            if creds.username != username {
                return Err(VfsError::SignInNeeded);
            }
            let host_keys = crate::host_key::saved_host_keys(&hostname, port, &known_hosts)
                .map_err(|_| VfsError::Other(COULD_NOT_START.into()))?;
            Ok((creds, host_keys))
        })
        .await
        .map_err(|_| VfsError::Other(COULD_NOT_START.into()))??;

        // What a failed attempt holds back lasts `retry_after`, except a refused password, which lasts for good: servers lock an account after a few wrong passwords. A refused key or agent waits because the fix is often on the server, as a key added to `authorized_keys`. A failure before the sign-in waits because OpenSSH 9.8 and later penalize an address for such connections (`PerSourcePenalties`), and a mount makes a call for every file the OS touches.
        failed_attempts.retain(|failed| {
            matches!(
                (&failed.ended, &failed.sign_in),
                (Ended::Refused, SignIn::Password(_))
            ) || failed.at.elapsed() < self.config.retry_after
        });
        // Another password, key path, or passphrase is another attempt, tried at once. So is the same one after the user trusted the server's key, since `known_hosts` then holds other keys. A fixed key file under the same path, or an agent that now runs, waits.
        let blocked = failed_attempts.iter().any(|failed| {
            failed.sign_in == creds.sign_in
                && match &failed.ended {
                    Ended::Refused => true,
                    Ended::BeforeSignIn { host_keys: then } => then == &host_keys,
                }
        });
        if blocked {
            return Err(VfsError::SignInNeeded);
        }

        // A connection that dies during the sign-in would otherwise hold every call waiting on this attempt until TCP gives up. The keychain read above has no limit, since its prompt waits for the user.
        let opened = tokio::time::timeout(
            SIGN_IN_LIMIT,
            open_sftp_channel(
                &self.hostname,
                port,
                &creds,
                &self.config.known_hosts,
                host_keys.clone(),
            ),
        )
        .await
        .map_err(|_| VfsError::TimedOut)?;
        let error = match opened {
            Ok(opened) => return Ok(opened),
            Err(error) => error,
        };
        let (ended, answer) = match error {
            ConnectError::SignInRejected(_) => (Some(Ended::Refused), VfsError::SignInNeeded),
            ConnectError::UnknownHostKey(_)
            | ConnectError::ChangedHostKey(_)
            | ConnectError::PassphraseNeeded(_)
            // An agent that could not sign, as when the user declined its prompt: the next call would bring the prompt back at once.
            | ConnectError::SignInUnusable(_) => (
                Some(Ended::BeforeSignIn { host_keys }),
                VfsError::SignInNeeded,
            ),
            ConnectError::Unreachable(_) => (None, VfsError::Offline),
            // Its text can hold a path.
            ConnectError::Failed(_) => (None, VfsError::Other(COULD_NOT_START.into())),
        };
        if let Some(ended) = ended {
            failed_attempts.push(FailedAttempt {
                sign_in: creds.sign_in,
                at: Instant::now(),
                ended,
            });
        }
        Err(answer)
    }
}

/// A sign-in attempt that the server refused, or that failed before the server judged it. The account holds the user, so the attempt is told apart by its sign-in: the password, the key with its passphrase, or the agent.
struct FailedAttempt {
    sign_in: SignIn,
    at: Instant,
    ended: Ended,
}

/// How the attempt ended.
enum Ended {
    /// The server said no to the sign-in.
    Refused,
    /// The attempt failed before the server judged the sign-in: the host key, the passphrase, the key file, or the agent.
    BeforeSignIn {
        /// What `known_hosts` held for the server then.
        host_keys: Vec<PublicKey>,
    },
}

/// One saved SFTP server. An item's id is its absolute path.
///
/// A provider holds one connection. Dropping the provider closes it once no call uses it.
///
/// The top of a mount is `/`, not the home folder. A server's useful folders are often outside home, and a mount's top cannot move. The app's `sftp://` view also starts at `/`.
pub struct SftpProvider {
    inner: Arc<Inner>,
}

/// What a connection attempt needs. An attempt runs in a task of its own, so it holds this, not the provider.
struct Inner {
    account: Arc<SftpAccount>,
    slot: Mutex<Slot>,
    /// Temporary files of uploads whose connection was lost before they could be removed. The next upload removes them.
    strays: Mutex<Vec<String>>,
}

/// The provider's one connection.
///
/// An attempt runs in a task of its own, not in the call that started it. The core cancels a call by dropping it. An attempt inside that call would end with it: a cancel during a keychain prompt would let the next call start a second prompt, and a cancel while the server judges a password would lose the refusal, so the password would be sent again.
enum Slot {
    Empty,
    /// Every call that needs a connection waits on this attempt. Only the attempt changes the slot, when it ends.
    Connecting(Shared<BoxFuture<'static, Result<Arc<Connection>, VfsError>>>),
    Ready(Arc<Connection>),
}

struct Connection {
    sftp: RawSftpSession,
    /// Cancelled when the SFTP channel's stream ends (see [`Watched`]), when the server stops answering (see [`ask_whether_there`]), and when this connection drops.
    ended: CancellationToken,
    /// The provider whose slot may hold this connection. Weak, since the slot holds the connection.
    provider: Weak<Inner>,
    /// The most one read request asks for.
    read_len: u32,
    /// The most one write request sends.
    write_len: u32,
    /// The server has `statvfs@openssh.com`.
    statvfs: bool,
    /// The server has `posix-rename@openssh.com`, so one request can replace an item.
    posix_rename: bool,
    /// The server has `fsync@openssh.com`, so an upload can put the new file on disk before publishing it.
    fsync: bool,
}

impl Connection {
    /// Takes this connection out of the slot, so no new call uses it. The calls already on it finish, and the last one to drop it closes it, which frees every handle the server holds for it.
    fn retire(&self) {
        let Some(provider) = self.provider.upgrade() else {
            return;
        };
        let mut slot = provider.slot.lock().unwrap();
        if matches!(&*slot, Slot::Ready(conn) if std::ptr::eq(&**conn, self)) {
            *slot = Slot::Empty;
        }
    }
}

impl Drop for Connection {
    /// Ends [`ask_whether_there`], which holds the session open.
    fn drop(&mut self) {
        self.ended.cancel();
    }
}

/// Asks the server every 5 seconds whether it is there, and cancels `ended` when the server has been quiet for 15 seconds: no answer to the ask, and no SFTP bytes since the ask or since the last bytes before it. A connection that died silently, as after a laptop sleeps, would otherwise look open until TCP gives up, and every call on it would wait out the core's 30-second limit. It ends within 20 seconds, so a call waiting on it ends too: a read runs again on a new connection (see [`SftpProvider::call`]), and a change or an upload answers `Offline` ([`SftpProvider::run_once`]).
///
/// Bytes count as an answer, because on a slow link the answer can queue behind file data already on its way. russh's own keepalive is not used: before the sign-in ends it lets its timer fire again and again with no wait, so a slow agent prompt would keep a thread busy.
async fn ask_whether_there(
    ssh: client::Handle<SshHandler>,
    ended: CancellationToken,
    heard: Arc<Mutex<Instant>>,
) {
    loop {
        tokio::select! {
            () = ended.cancelled() => return,
            () = tokio::time::sleep(Duration::from_secs(5)) => {}
        }
        let asked = Instant::now();
        let ping = ssh.send_ping();
        tokio::pin!(ping);
        loop {
            let quiet_until = asked.max(*heard.lock().unwrap()) + Duration::from_secs(15);
            if quiet_until <= Instant::now() {
                ended.cancel();
                return;
            }
            tokio::select! {
                () = ended.cancelled() => return,
                answered = &mut ping => {
                    if answered.is_err() {
                        ended.cancel();
                        return;
                    }
                    break;
                }
                () = tokio::time::sleep_until(quiet_until.into()) => {}
            }
        }
    }
}

/// The read half of the SFTP channel's stream. It notes in `heard` when SFTP bytes last came, and cancels `ended` when a read finds the stream ended or failed. When the connection ends, the read half ends too.
///
/// russh-sftp keeps a request that never got its answer waiting until its own time limit, even after the connection ended. `ended` ends that wait at once.
struct Watched<R> {
    read: R,
    ended: CancellationToken,
    heard: Arc<Mutex<Instant>>,
}

impl<R: AsyncRead + Unpin> AsyncRead for Watched<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let room = buf.remaining() > 0;
        let polled = Pin::new(&mut self.read).poll_read(cx, buf);
        match &polled {
            Poll::Ready(Ok(())) if room && buf.filled().len() == before => self.ended.cancel(),
            Poll::Ready(Ok(())) => *self.heard.lock().unwrap() = Instant::now(),
            Poll::Ready(Err(_)) => self.ended.cancel(),
            Poll::Pending => {}
        }
        polled
    }
}

/// What the server answered to one request: the value, or a status other than `Ok`, `NoConnection`, and `ConnectionLost`.
type Answer<T> = Result<T, StatusCode>;

/// Why a request or a call got no answer it can use.
enum Failure {
    /// The connection ended under the request.
    Lost,
    Error(VfsError),
}

/// A status the call passes on.
impl From<StatusCode> for Failure {
    fn from(status: StatusCode) -> Self {
        Failure::Error(match status {
            StatusCode::NoSuchFile => VfsError::NotFound,
            StatusCode::PermissionDenied => VfsError::AccessDenied,
            // A status answers one path or handle, so it is a failure of that item. `Other` would count against the whole mount.
            _ => VfsError::NotSupported,
        })
    }
}

impl From<Failure> for VfsError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Lost => VfsError::Offline,
            Failure::Error(error) => error,
        }
    }
}

/// Sends one request, and ends it at once when the connection ends. An answer that already came wins.
async fn ask<T>(
    ended: &CancellationToken,
    request: impl Future<Output = Result<T, SftpError>>,
) -> Result<Answer<T>, Failure> {
    let answer = tokio::select! {
        biased;
        answer = request => answer,
        _ = ended.cancelled() => return Err(Failure::Lost),
    };
    match answer {
        Ok(value) => Ok(Ok(value)),
        Err(SftpError::Status(status)) => match status.status_code {
            StatusCode::NoConnection | StatusCode::ConnectionLost => Err(Failure::Lost),
            // A raw call that expects a handle, data, a name, or attributes returns any status as an error, `Ok` included.
            StatusCode::Ok => Err(Failure::Error(VfsError::Other(COULD_NOT_READ.into()))),
            code => Ok(Err(code)),
        },
        // A request sent after the stream ended fails at once, before `ended` wins the race.
        Err(_) if ended.is_cancelled() => Err(Failure::Lost),
        Err(SftpError::Timeout) => Err(Failure::Error(VfsError::TimedOut)),
        Err(_) => Err(Failure::Error(VfsError::Other(COULD_NOT_READ.into()))),
    }
}

/// Signs in, starts the SFTP session, and asks the server its limits.
async fn open_connection(inner: &Arc<Inner>) -> Result<Arc<Connection>, VfsError> {
    let (ssh, stream) = inner.account.sign_in().await?;

    let ended = CancellationToken::new();
    let heard = Arc::new(Mutex::new(Instant::now()));
    let (read, write) = tokio::io::split(stream);
    let watched = Watched {
        read,
        ended: ended.clone(),
        heard: Arc::clone(&heard),
    };
    let sftp = RawSftpSession::new_with_config(
        tokio::io::join(watched, write),
        // The core's limits decide how long a request waits: its call time limit for a call, and its stall rule for an upload. russh-sftp starts each request's clock when the request is queued, so its own limit sits far above both: an upload keeps 8 writes queued, and on a slow uplink the last waits a long time for its turn. Its default of 10 seconds would fail a slow read of a large piece.
        russh_sftp::client::Config {
            request_timeout_secs: 600,
            ..Default::default()
        },
    );
    let version = ask(&ended, sftp.init())
        .await?
        .map_err(|_| VfsError::Other(COULD_NOT_START.into()))?;
    let offers = |name: &str, value: &str| version.extensions.get(name).is_some_and(|v| v == value);
    // One request for both lengths. Capped, so the pieces one call keeps going hold at most 2 MiB.
    let mut read_len = SMALL_READ;
    let mut write_len = SMALL_WRITE;
    if offers(LIMITS, "1") {
        if let Ok(limits) = ask(&ended, sftp.limits()).await? {
            if limits.max_read_len > 0 {
                read_len = limits.max_read_len.min(256 * 1024) as u32;
            }
            if limits.max_write_len > 0 {
                write_len = limits.max_write_len.min(256 * 1024) as u32;
            }
        }
    }
    let conn = Arc::new(Connection {
        statvfs: offers(STATVFS, "2"),
        posix_rename: offers("posix-rename@openssh.com", "1"),
        fsync: offers("fsync@openssh.com", "1"),
        sftp,
        ended,
        provider: Arc::downgrade(inner),
        read_len,
        write_len,
    });
    tokio::spawn(ask_whether_there(ssh, conn.ended.clone(), heard));
    Ok(conn)
}

/// An open file or folder handle. Dropping it sends the close from a task of its own.
struct OpenHandle {
    conn: Arc<Connection>,
    handle: String,
}

impl OpenHandle {
    /// Closes the handle and waits for the server's answer. `Drop` sends nothing after this.
    async fn close(mut self) -> Result<Answer<()>, Failure> {
        let handle = std::mem::take(&mut self.handle);
        let answer = ask(&self.conn.ended, self.conn.sftp.close(handle)).await?;
        Ok(answer.map(|_| ()))
    }
}

impl Drop for OpenHandle {
    fn drop(&mut self) {
        if self.handle.is_empty() {
            return;
        }
        // Outside a runtime there is nothing to send the close with.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let conn = Arc::clone(&self.conn);
        let handle = std::mem::take(&mut self.handle);
        runtime.spawn(async move {
            // A close with no answer leaves the handle open or not: see `open_handle`.
            if ask(&conn.ended, conn.sftp.close(handle)).await.is_err() {
                conn.retire();
            }
        });
    }
}

/// Sends an open or an opendir.
///
/// Only an answer, a handle or a status, says for certain whether the server holds a handle. When the open ends any other way, as when the core cancels the call, the server may hold one that nothing can close. So the connection is retired, and when it closes, the server frees all its handles.
async fn open_handle(
    conn: &Arc<Connection>,
    open: impl Future<Output = Result<Handle, SftpError>>,
) -> Result<Answer<OpenHandle>, Failure> {
    struct Unanswered<'a>(&'a Connection);
    impl Drop for Unanswered<'_> {
        fn drop(&mut self) {
            self.0.retire();
        }
    }

    let unanswered = Unanswered(conn);
    let answer = ask(&conn.ended, open).await?;
    std::mem::forget(unanswered);
    Ok(answer.map(|handle| OpenHandle {
        conn: Arc::clone(conn),
        handle: handle.handle,
    }))
}

/// The item a mount shows, or `None` for a kind it cannot show: a link, a FIFO, a socket, a device. A read of a FIFO would hang the server.
fn item_of(id: String, attrs: &FileAttributes) -> Option<Item> {
    // Not `is_dir()` or `is_regular()`: they test bits with `contains`, so a link reads as a regular file and a socket as a folder.
    let kind = match attrs.file_type() {
        FileType::Dir => Kind::Folder,
        FileType::File => Kind::File,
        FileType::Symlink | FileType::Other => return None,
    };
    Some(Item {
        name: id.rsplit_once('/').map_or("", |(_, name)| name).to_string(),
        attributes: Attributes {
            kind,
            size: match kind {
                Kind::File => attrs.size.unwrap_or(0),
                Kind::Folder => 0,
            },
            modified: UNIX_EPOCH + Duration::from_secs(attrs.mtime.unwrap_or(0).into()),
            revision: match kind {
                Kind::File => Some(revision_of(attrs)),
                Kind::Folder => None,
            },
            read_only: false,
        },
        id: ItemId(id),
    })
}

/// A file's revision: its size and its modified time. SFTP gives the time only to the second, so two changes within one second that leave the size the same look like no change.
fn revision_of(attrs: &FileAttributes) -> Revision {
    Revision(format!(
        "{}:{}",
        attrs.size.unwrap_or(0),
        attrs.mtime.unwrap_or(0)
    ))
}

/// `/{name}` when `folder` is `/`, and `{folder}/{name}` otherwise.
fn child(folder: &str, name: &str) -> String {
    match folder {
        "/" => format!("/{name}"),
        _ => format!("{folder}/{name}"),
    }
}

/// The folder part of a path. `/` for an item in the top folder.
fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) | None => "/",
        Some(at) => &path[..at],
    }
}

/// A name that would not be the item the caller asked for. A path built from an empty name, `.`, `..`, or a name with `/` or NUL would name another item, and `list` hides a name with U+FFFD (russh-sftp decodes names lossily, #637), so a file made under one would vanish from the mount.
fn check_name(name: &str) -> Result<(), VfsError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\0')
        || name.contains('\u{FFFD}')
    {
        Err(VfsError::NotSupported)
    } else {
        Ok(())
    }
}

/// What a generic `Failure` meant for a change of the folder `path`. OpenSSH answers it for a folder that holds anything: `NotEmpty` when the folder holds a name other than `.` and `..`, and `NotSupported` when it holds none. Pages are read until a name or the end, because one page of only `.` and `..` does not show a folder is empty.
async fn folder_failure(conn: &Arc<Connection>, path: &str) -> Failure {
    let holds_any = async {
        let handle = open_handle(conn, conn.sftp.opendir(path)).await??;
        loop {
            match ask(&conn.ended, conn.sftp.readdir(handle.handle.as_str())).await? {
                Ok(name) => {
                    if name
                        .files
                        .iter()
                        .any(|file| file.filename != "." && file.filename != "..")
                    {
                        return Ok(true);
                    }
                }
                Err(StatusCode::Eof) => return Ok(false),
                Err(status) => return Err(Failure::from(status)),
            }
        }
    };
    match holds_any.await {
        Ok(true) => Failure::Error(VfsError::NotEmpty),
        Ok(false) => Failure::Error(VfsError::NotSupported),
        Err(failure) => failure,
    }
}

/// The temporary file an upload made. Unless [`Temporary::keep`] ran, dropping it removes the file, so a dropped future or a call that fails leaves nothing behind. When the connection is lost first, the provider keeps the path, and its next upload removes the file.
struct Temporary {
    conn: Arc<Connection>,
    path: String,
    kept: bool,
}

impl Temporary {
    /// The upload published the file, so dropping the guard must not remove it.
    fn keep(mut self) {
        self.kept = true;
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if self.kept {
            return;
        }
        // Outside a runtime there is nothing to send the remove with.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(remove_temporary(
            Arc::clone(&self.conn),
            std::mem::take(&mut self.path),
        ));
    }
}

/// Removes an upload's temporary file. When the connection is lost first, the provider keeps the path, and its next upload tries again.
async fn remove_temporary(conn: Arc<Connection>, path: String) {
    if let Err(Failure::Lost) = ask(&conn.ended, conn.sftp.remove(path.as_str())).await {
        if let Some(provider) = conn.provider.upgrade() {
            provider.strays.lock().unwrap().push(path);
        }
    }
}

/// What a generic `Failure` meant for a change that makes `path`. OpenSSH answers it when the name is taken, so a lookup that finds the name is `Conflict`. A lookup that finds nothing means the failure was something else: `NotSupported`. Any other answer to the lookup maps as usual.
async fn name_failure(conn: &Connection, path: &str) -> Failure {
    match ask(&conn.ended, conn.sftp.lstat(path)).await {
        Ok(Ok(_)) => Failure::Error(VfsError::Conflict),
        Ok(Err(StatusCode::NoSuchFile)) => Failure::Error(VfsError::NotSupported),
        Ok(Err(status)) => status.into(),
        Err(failure) => failure,
    }
}

/// Moves `from` to `to` and replaces the item there, in one step: `posix-rename@openssh.com`. SFTP's own rename never replaces, and doing it in steps (moving the old item aside first) would leave the path with no item if the connection or the call ended between them, which `Provider::rename` forbids. A server without the extension cannot replace: `NotSupported`.
async fn swap(conn: &Connection, from: &str, to: &str) -> Result<Answer<()>, Failure> {
    if !conn.posix_rename {
        return Err(Failure::Error(VfsError::NotSupported));
    }
    // `posix-rename@openssh.com` has the same body as `hardlink@openssh.com`: two SSH strings, the old path and the new one.
    let data = russh_sftp::extensions::HardlinkExtension {
        oldpath: from.to_string(),
        newpath: to.to_string(),
    }
    .try_into()
    .map_err(|_| Failure::Error(VfsError::Other(COULD_NOT_READ.into())))?;
    let packet = ask(
        &conn.ended,
        conn.sftp.extended("posix-rename@openssh.com", data),
    )
    .await??;
    match packet {
        Packet::Status(status) => match status.status_code {
            StatusCode::Ok => Ok(Ok(())),
            StatusCode::NoConnection | StatusCode::ConnectionLost => Err(Failure::Lost),
            code => Ok(Err(code)),
        },
        _ => Err(Failure::Error(VfsError::Other(COULD_NOT_READ.into()))),
    }
}

/// Writes the local file `from` to a new file under a temporary name in `folder`, sets its modified time, puts it on the server's disk when the server can (`fsync@openssh.com`), and reads its attributes back. It returns the file still open: the caller closes it, and a replace first sets the owner and the mode through the handle, which stays on the file whatever happens to its name. A `private` file has mode `0600` until its caller sets another. Pieces of `write_len` go out `IN_FLIGHT` at a time, and `progress` hears the bytes the server has taken, in order. Once its open answered, the temporary file is removed when this or the caller fails or is dropped (see [`Temporary`]).
async fn upload(
    conn: &Arc<Connection>,
    folder: &str,
    from: &Path,
    modified: SystemTime,
    private: bool,
    progress: &Progress,
) -> Result<(Temporary, OpenHandle, FileAttributes), Failure> {
    if let Some(provider) = conn.provider.upgrade() {
        let strays = std::mem::take(&mut *provider.strays.lock().unwrap());
        for path in strays {
            tokio::spawn(remove_temporary(Arc::clone(conn), path));
        }
    }
    // The process id, the time, and a counter keep names apart, and the open's `EXCLUDE` refuses a name that exists. It is short, so it fits next to any name a folder already holds.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let name = format!(".marlin-{}-{nanos}-{n}", std::process::id());
    let path = child(folder, &name);
    let mut open_attrs = FileAttributes::empty();
    open_attrs.permissions = private.then_some(0o600);
    let opened = conn.sftp.open(
        path.as_str(),
        OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
        open_attrs,
    );
    let handle = open_handle(conn, opened).await??;
    let temporary = Temporary {
        conn: Arc::clone(conn),
        path,
        kept: false,
    };

    let from = from.to_path_buf();
    let length = tokio::task::spawn_blocking({
        let from = from.clone();
        move || std::fs::metadata(from).map(|meta| meta.len())
    })
    .await
    .map_err(|_| Failure::Error(VfsError::Other(COULD_NOT_UPLOAD.into())))?
    .map_err(|_| Failure::Error(VfsError::Other(COULD_NOT_UPLOAD.into())))?;

    let write_len = conn.write_len as usize;
    let handle_name = handle.handle.clone();
    let mut writes = stream::iter((0..length).step_by(write_len))
        .map(|offset| {
            let conn = Arc::clone(conn);
            let from = from.clone();
            let handle_name = handle_name.clone();
            async move {
                let len = (length - offset).min(write_len as u64);
                // The app links this crate on Windows too, so the piece is read with `std::fs`, not a Unix call.
                let piece = tokio::task::spawn_blocking(move || {
                    let mut file = std::fs::File::open(&from)?;
                    file.seek(SeekFrom::Start(offset))?;
                    let mut piece = Vec::new();
                    file.take(len).read_to_end(&mut piece)?;
                    Ok::<_, std::io::Error>(piece)
                })
                .await
                .map_err(|_| Failure::Error(VfsError::Other(COULD_NOT_UPLOAD.into())))?
                .map_err(|_| Failure::Error(VfsError::Other(COULD_NOT_UPLOAD.into())))?;
                let wrote = piece.len();
                if wrote > 0 {
                    match ask(&conn.ended, conn.sftp.write(handle_name, offset, piece)).await? {
                        Ok(_) => {}
                        // OpenSSH answers a generic failure to a write when the disk or the quota is full.
                        Err(StatusCode::Failure) => return Err(Failure::Error(VfsError::NoSpace)),
                        Err(status) => return Err(status.into()),
                    }
                }
                Ok::<usize, Failure>(wrote)
            }
        })
        .buffered(IN_FLIGHT);
    let mut sent = 0u64;
    while let Some(wrote) = writes.try_next().await? {
        sent += wrote as u64;
        progress(sent);
    }

    // SFTP sets the access time and the modified time together.
    let secs = modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as u32;
    let mut attrs = FileAttributes::empty();
    attrs.atime = Some(secs);
    attrs.mtime = Some(secs);
    // A server that refuses to set times keeps its own: the `fstat` below reads what the file has.
    let _ = ask(
        &conn.ended,
        conn.sftp.fsetstat(handle.handle.as_str(), attrs),
    )
    .await?;
    if conn.fsync {
        match ask(&conn.ended, conn.sftp.fsync(handle.handle.as_str())).await? {
            Ok(_) | Err(StatusCode::OpUnsupported) => {}
            Err(status) => return Err(status.into()),
        }
    }
    // Read back before the rename that publishes the file, so nothing has to be asked after it. It is a file, whatever type bits the server leaves out.
    let mut attrs = ask(&conn.ended, conn.sftp.fstat(handle.handle.as_str()))
        .await??
        .attrs;
    attrs.set_regular(true);
    Ok((temporary, handle, attrs))
}

impl SftpProvider {
    pub fn new(account: Arc<SftpAccount>) -> Self {
        Self {
            inner: Arc::new(Inner {
                account,
                slot: Mutex::new(Slot::Empty),
                strays: Mutex::new(Vec::new()),
            }),
        }
    }

    /// The connection a call runs on, and whether it was there before the call.
    async fn connection(&self) -> Result<(Arc<Connection>, bool), VfsError> {
        let attempt = {
            let mut slot = self.inner.slot.lock().unwrap();
            match &*slot {
                Slot::Ready(conn) if !conn.ended.is_cancelled() => {
                    return Ok((Arc::clone(conn), true));
                }
                Slot::Connecting(attempt) => attempt.clone(),
                Slot::Empty | Slot::Ready(_) => {
                    let inner = Arc::clone(&self.inner);
                    let attempt = async move {
                        let opened = open_connection(&inner).await;
                        *inner.slot.lock().unwrap() = match &opened {
                            Ok(conn) => Slot::Ready(Arc::clone(conn)),
                            Err(_) => Slot::Empty,
                        };
                        opened
                    }
                    .boxed()
                    .shared();
                    *slot = Slot::Connecting(attempt.clone());
                    tokio::spawn(attempt.clone());
                    attempt
                }
            }
        };
        Ok((attempt.await?, false))
    }

    /// Runs `body` on the connection and does not run it again: a change, and a byte call, are not safe to repeat, because the server may have acted before its answer was lost. A `Lost` retires the connection, as in [`Self::call`], and is `Offline`. The core sends a failed upload again itself.
    async fn run_once<T, Fut>(
        &self,
        body: impl FnOnce(Arc<Connection>) -> Fut,
    ) -> Result<T, VfsError>
    where
        Fut: Future<Output = Result<T, Failure>>,
    {
        let (conn, _) = self.connection().await?;
        match body(Arc::clone(&conn)).await {
            Err(Failure::Lost) => {
                conn.retire();
                Err(VfsError::Offline)
            }
            answer => answer.map_err(VfsError::from),
        }
    }

    /// Runs `body` on the connection. A `Lost` retires that connection, since a server can answer `ConnectionLost` while the connection still looks open. A `Lost` on a connection that was there before the call runs `body` once more on a new one. Every body here only reads, so a repeat is safe: a change asks its first question here and makes the change in [`Self::run_once`].
    async fn call<T, Fut>(&self, body: impl Fn(Arc<Connection>) -> Fut) -> Result<T, VfsError>
    where
        Fut: Future<Output = Result<T, Failure>>,
    {
        // The one repeat is for a connection that was there before the call, which may have died while nobody used it.
        let (mut conn, mut may_retry) = self.connection().await?;
        loop {
            match body(Arc::clone(&conn)).await {
                Err(Failure::Lost) => {
                    conn.retire();
                    if !may_retry {
                        return Err(VfsError::Offline);
                    }
                    may_retry = false;
                    (conn, _) = self.connection().await?;
                }
                answer => return answer.map_err(VfsError::from),
            }
        }
    }
}

#[async_trait::async_trait]
impl Provider for SftpProvider {
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
        self.call(|conn| async move {
            // `stat` follows links.
            let attrs = ask(&conn.ended, conn.sftp.stat(id.0.as_str()))
                .await??
                .attrs;
            item_of(id.0.clone(), &attrs).ok_or(Failure::Error(VfsError::NotSupported))
        })
        .await
    }

    async fn list(&self, folder: &ItemId) -> Result<Vec<Item>, VfsError> {
        let folder = folder.0.as_str();
        self.call(|conn| async move {
            let handle = match open_handle(&conn, conn.sftp.opendir(folder)).await? {
                Ok(handle) => handle,
                // OpenSSH answers `NoSuchFile` for a file, and other servers answer `Failure` for a file or a folder that is not there. Only a stat tells which, and its status says more than the generic one.
                Err(status @ (StatusCode::NoSuchFile | StatusCode::Failure)) => {
                    return match ask(&conn.ended, conn.sftp.stat(folder)).await? {
                        Ok(attrs) if attrs.attrs.file_type() != FileType::Dir => {
                            Err(Failure::Error(VfsError::NotAFolder))
                        }
                        Ok(_) => Err(status.into()),
                        Err(stat_status) => Err(stat_status.into()),
                    };
                }
                Err(status) => return Err(status.into()),
            };
            let mut entries = Vec::new();
            loop {
                match ask(&conn.ended, conn.sftp.readdir(handle.handle.as_str())).await? {
                    Ok(name) => entries.extend(name.files),
                    Err(StatusCode::Eof) => break,
                    Err(status) => return Err(status.into()),
                }
            }
            drop(handle);

            // A link to a folder shows only in a folder reached with no link on the way, and only when it points neither at that folder nor above it. A walk then crosses at most one folder link, so links that point into each other's folders cannot make an endless tree. Asked once per listing, and only when it holds a link to a folder.
            let reached_directly = OnceCell::new();
            let (conn, reached_directly) = (&conn, &reached_directly);
            stream::iter(entries.into_iter().filter(|entry| {
                // russh-sftp 2.1.2 decodes names lossily, so a name with U+FFFD may stand for bytes that are not UTF-8, and two names could get one id (#637).
                entry.filename != "."
                    && entry.filename != ".."
                    && !entry.filename.contains('\u{FFFD}')
            }))
            .map(|entry| async move {
                let path = child(folder, &entry.filename);
                if entry.attrs.file_type() != FileType::Symlink {
                    return Ok(item_of(path, &entry.attrs));
                }
                // A link shows what it points to, under its own name. A broken link, a loop, and a link this user may not follow are left out.
                let Ok(target) = ask(&conn.ended, conn.sftp.stat(path.as_str())).await? else {
                    return Ok(None);
                };
                if target.attrs.file_type() == FileType::Dir {
                    let reached = reached_directly
                        .get_or_try_init(|| async {
                            let real = ask(&conn.ended, conn.sftp.realpath(folder)).await?;
                            Ok::<_, Failure>(real.is_ok_and(|real| {
                                real.files
                                    .first()
                                    .is_some_and(|real| real.filename == folder)
                            }))
                        })
                        .await?;
                    if !*reached {
                        return Ok(None);
                    }
                    let Ok(real) = ask(&conn.ended, conn.sftp.realpath(path.as_str())).await?
                    else {
                        return Ok(None);
                    };
                    let shown = real.files.first().is_some_and(|to| {
                        let to = to.filename.as_str();
                        to != "/" && to != folder && !folder.starts_with(&format!("{to}/"))
                    });
                    if !shown {
                        return Ok(None);
                    }
                }
                Ok(item_of(path, &target.attrs))
            })
            .buffered(IN_FLIGHT)
            .try_filter_map(|item| async move { Ok(item) })
            .try_collect()
            .await
        })
        .await
    }

    async fn read(&self, id: &ItemId, offset: u64, length: u32) -> Result<Vec<u8>, VfsError> {
        if length == 0 {
            return Ok(Vec::new());
        }
        self.call(|conn| async move {
            let open = conn
                .sftp
                .open(id.0.as_str(), OpenFlags::READ, FileAttributes::empty());
            let file = open_handle(&conn, open).await??;
            let (conn, file) = (&conn, &file);
            let end = offset.saturating_add(length.into());
            let mut pieces = stream::iter((offset..end).step_by(conn.read_len as usize))
                .map(|start| async move {
                    let wanted = (end - start).min(u64::from(conn.read_len)) as usize;
                    let mut piece = Vec::with_capacity(wanted);
                    // A short answer is not the end of the file: ask again for the bytes still missing. `Eof` or an empty answer is the end.
                    while piece.len() < wanted {
                        let at = start + piece.len() as u64;
                        let missing = (wanted - piece.len()) as u32;
                        let read = conn.sftp.read(file.handle.as_str(), at, missing);
                        match ask(&conn.ended, read).await? {
                            Ok(data) if data.data.is_empty() => break,
                            Ok(data) => piece.extend(data.data),
                            Err(StatusCode::Eof) => break,
                            Err(status) => return Err(Failure::from(status)),
                        }
                    }
                    piece.truncate(wanted);
                    Ok((piece, wanted))
                })
                .buffered(IN_FLIGHT);
            let mut bytes = Vec::new();
            while let Some((piece, wanted)) = pieces.try_next().await? {
                let short = piece.len() < wanted;
                bytes.extend(piece);
                // A short piece ends at the end of the file, so the pieces after it hold nothing.
                if short {
                    break;
                }
            }
            Ok(bytes)
        })
        .await
    }

    async fn space(&self) -> Result<Space, VfsError> {
        self.call(|conn| async move {
            if !conn.statvfs {
                return Ok(UNKNOWN_SPACE);
            }
            Ok(match ask(&conn.ended, conn.sftp.statvfs("/")).await? {
                Ok(stats) => Space {
                    total: stats.blocks.saturating_mul(stats.fragment_size),
                    free: stats.blocks_avail.saturating_mul(stats.fragment_size),
                },
                Err(_) => UNKNOWN_SPACE,
            })
        })
        .await
    }

    async fn create_folder(&self, parent: &ItemId, name: &str) -> Result<Item, VfsError> {
        check_name(name)?;
        let folder = parent.0.as_str();
        // Asked in a call of its own, which runs once more on a new connection when the old one died while idle: nothing has changed yet.
        let attrs = self
            .call(|conn| async move { Ok(ask(&conn.ended, conn.sftp.stat(folder)).await??.attrs) })
            .await?;
        if attrs.file_type() != FileType::Dir {
            return Err(VfsError::NotAFolder);
        }
        let path = child(folder, name);
        self.run_once(|conn| async move {
            let made = ask(
                &conn.ended,
                conn.sftp.mkdir(path.as_str(), FileAttributes::empty()),
            );
            match made.await? {
                Ok(_) => {}
                Err(StatusCode::Failure) => return Err(name_failure(&conn, &path).await),
                Err(status) => return Err(status.into()),
            }
            let attrs = ask(&conn.ended, conn.sftp.stat(path.as_str()))
                .await??
                .attrs;
            item_of(path, &attrs).ok_or(Failure::Error(VfsError::NotSupported))
        })
        .await
    }

    async fn remove(&self, id: &ItemId, folder: &ItemId) -> Result<(), VfsError> {
        if parent(&id.0) != folder.0 {
            return Err(VfsError::NotFound);
        }
        let path = id.0.as_str();
        // Asked in a call of its own, as in `create_folder`. `lstat` does not follow a link, so a link is removed itself, never what it points to.
        let attrs = self
            .call(|conn| async move { Ok(ask(&conn.ended, conn.sftp.lstat(path)).await??.attrs) })
            .await?;
        let is_folder = attrs.file_type() == FileType::Dir;
        self.run_once(|conn| async move {
            let removed = if is_folder {
                ask(&conn.ended, conn.sftp.rmdir(path)).await?
            } else {
                ask(&conn.ended, conn.sftp.remove(path)).await?
            };
            match removed {
                Ok(_) => Ok(()),
                Err(StatusCode::Failure) if is_folder => Err(folder_failure(&conn, path).await),
                Err(status) => Err(status.into()),
            }
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
        if parent(&id.0) != from.0 || replace.is_some_and(|replace| parent(&replace.0) != to.0) {
            return Err(VfsError::NotFound);
        }
        let path = id.0.as_str();
        // Asked in a call of its own, as in `create_folder`.
        self.call(|conn| async move {
            ask(&conn.ended, conn.sftp.lstat(path)).await??;
            Ok(())
        })
        .await?;
        let new_name = child(&to.0, name);
        self.run_once(|conn| async move {
            let published = match replace {
                Some(replace) => {
                    let replaced = ask(&conn.ended, conn.sftp.lstat(replace.0.as_str()))
                        .await??
                        .attrs;
                    match swap(&conn, path, &replace.0).await? {
                        Ok(()) => {}
                        Err(StatusCode::Failure) if replaced.file_type() == FileType::Dir => {
                            return Err(folder_failure(&conn, &replace.0).await);
                        }
                        Err(status) => return Err(status.into()),
                    }
                    // On a server whose disk ignores case, `replace` can differ from `new_name` only in case. The item now lives at `replace`.
                    replace.0.clone()
                }
                None => {
                    // SFTP's rename never overwrites. A folder moved into itself fails too, and ends as `NotSupported`.
                    match ask(&conn.ended, conn.sftp.rename(path, new_name.as_str())).await? {
                        Ok(_) => {}
                        Err(StatusCode::Failure) => {
                            return Err(name_failure(&conn, &new_name).await)
                        }
                        Err(status) => return Err(status.into()),
                    }
                    new_name
                }
            };
            let attrs = ask(&conn.ended, conn.sftp.stat(published.as_str()))
                .await??
                .attrs;
            item_of(published, &attrs).ok_or(Failure::Error(VfsError::NotSupported))
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
        let path = child(&parent.0, name);
        self.run_once(|conn| async move {
            match ask(&conn.ended, conn.sftp.lstat(path.as_str())).await? {
                Ok(_) => return Err(Failure::Error(VfsError::Conflict)),
                Err(StatusCode::NoSuchFile) => {}
                Err(status) => return Err(status.into()),
            }
            let (temporary, handle, attrs) =
                upload(&conn, &parent.0, from, modified, false, progress).await?;
            handle.close().await??;
            let item =
                item_of(path.clone(), &attrs).ok_or(Failure::Error(VfsError::NotSupported))?;
            // SFTP's rename never overwrites, so a file that took the name in the meantime stays.
            match ask(
                &conn.ended,
                conn.sftp.rename(temporary.path.as_str(), path.as_str()),
            )
            .await?
            {
                Ok(_) => {
                    temporary.keep();
                }
                Err(StatusCode::Failure) => return Err(name_failure(&conn, &path).await),
                Err(status) => return Err(status.into()),
            }
            Ok(item)
        })
        .await
    }

    async fn replace_bytes(
        &self,
        id: &ItemId,
        from: &Path,
        base: &Revision,
        modified: SystemTime,
        progress: &Progress,
    ) -> Result<Item, VfsError> {
        let path = id.0.as_str();
        self.run_once(|conn| async move {
            // A server that cannot replace in one step gets no upload it would refuse.
            if !conn.posix_rename {
                return Err(Failure::Error(VfsError::NotSupported));
            }
            // A replace is a new file renamed over the old one, so it does by hand what a write in place gets from the server: it follows a link, checks the user may write the file, and carries the owner, the group, and the access bits over. The revision is checked before the upload, to send nothing for a stale file, and after it, to overwrite nothing newer.
            let current = ask(&conn.ended, conn.sftp.lstat(path)).await??.attrs;
            let is_link = current.file_type() == FileType::Symlink;
            // A server whose `realpath` only tidies the path gives the link back, and its `lstat` then sees a link, not a file.
            let (target, target_attrs) = if is_link {
                let real = ask(&conn.ended, conn.sftp.realpath(path)).await??;
                let target = real
                    .files
                    .first()
                    .map(|file| file.filename.clone())
                    .unwrap_or_default();
                let attrs = ask(&conn.ended, conn.sftp.lstat(target.as_str()))
                    .await??
                    .attrs;
                (target, attrs)
            } else {
                (path.to_string(), current)
            };
            // A lossy name could be another file.
            if target_attrs.file_type() != FileType::File || target.contains('\u{FFFD}') {
                return Err(Failure::Error(VfsError::NotSupported));
            }
            if revision_of(&target_attrs) != *base {
                return Err(Failure::Error(VfsError::Conflict));
            }
            // The rename that publishes the new bytes needs only the folder's permission. This open is what keeps a file the user may not change from being changed.
            let open = conn
                .sftp
                .open(target.as_str(), OpenFlags::WRITE, FileAttributes::empty());
            open_handle(&conn, open).await??.close().await??;
            let (temporary, handle, attrs) =
                upload(&conn, parent(&target), from, modified, true, progress).await?;
            // For a link, the item keeps the link's path and shows the new bytes.
            let item =
                item_of(id.0.clone(), &attrs).ok_or(Failure::Error(VfsError::NotSupported))?;
            // The file as it is now decides: another client may have pointed the link elsewhere, or changed the file's bytes, its owner, or its mode during the upload. The link comes first, because its old target may be gone.
            if is_link {
                let real = ask(&conn.ended, conn.sftp.realpath(path)).await??;
                if real.files.first().map(|file| file.filename.as_str()) != Some(target.as_str()) {
                    return Err(Failure::Error(VfsError::Conflict));
                }
            }
            let now = ask(&conn.ended, conn.sftp.lstat(target.as_str()))
                .await??
                .attrs;
            if revision_of(&now) != *base {
                return Err(Failure::Error(VfsError::Conflict));
            }
            // The group goes first, so the access bits never open the file to a group it was not meant for. A server that refuses it refuses the replace, because the new file would let other people read it. The owner stays the signed-in user, as with an editor's safe save: only root may give a file away, and with the old group and access bits nobody can read the new file who could not read the old one. SFTP sends the owner and the group together, and one without the other would go as 0, which is root, so the new file's own owner goes with the old group.
            if let (Some(uid), Some(gid)) = (attrs.uid, now.gid) {
                if Some(gid) != attrs.gid {
                    let mut group = FileAttributes::empty();
                    group.uid = Some(uid);
                    group.gid = Some(gid);
                    ask(
                        &conn.ended,
                        conn.sftp.fsetstat(handle.handle.as_str(), group),
                    )
                    .await??;
                }
            }
            // A normal write clears set-user-ID and set-group-ID, so only the access bits carry over.
            let mut mode = FileAttributes::empty();
            mode.permissions = now.permissions.map(|mode| mode & 0o777);
            ask(
                &conn.ended,
                conn.sftp.fsetstat(handle.handle.as_str(), mode),
            )
            .await??;
            handle.close().await??;
            swap(&conn, &temporary.path, &target).await??;
            temporary.keep();
            Ok(item)
        })
        .await
    }
}
