use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, UNIX_EPOCH};

use futures_util::future::{BoxFuture, FutureExt, Shared};
use futures_util::stream::{self, StreamExt, TryStreamExt};
use marlin_vfs::{Attributes, Item, ItemId, Kind, Provider, Space, VfsError};
use russh::client;
use russh::keys::PublicKey;
use russh::ChannelStream;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::RawSftpSession;
use russh_sftp::extensions::{LIMITS, STATVFS};
use russh_sftp::protocol::{FileAttributes, FileType, Handle, OpenFlags, StatusCode};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use crate::connect::{open_sftp_channel, ConnectError, SshHandler};
use crate::servers::{server_credentials, SignIn};

/// How many calls one SFTP mount runs at once: the `Options::max_calls` for a mount of this provider. The helper is to set it (#477).
///
/// The app downloads one file at a time per server, because some servers fail on parallel reads. The mount core's default of 16 is for Google Drive, and one read here already keeps [`IN_FLIGHT`] requests going. Nobody has measured how fast either number is.
pub const MAX_CALLS: usize = 4;

/// How many requests one call keeps going at once: the pieces of a read, or the link lookups of a listing.
const IN_FLIGHT: usize = 8;

/// The piece a read asks for from a server without `limits@openssh.com`. Every server takes 32 KiB.
const SMALL_READ: u32 = 32 * 1024;

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
pub struct Config {
    /// The saved servers file. The helper is to pass `servers_path()` (#477).
    pub servers: PathBuf,

    /// `known_hosts`. The helper is to pass `known_hosts_path()` (#477).
    pub known_hosts: PathBuf,

    /// How long a refused key or agent sign-in, or an attempt that failed before the sign-in, waits before it is made again. A refused password is never sent again. The helper is to use 60 seconds (#477).
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

/// One saved SFTP server, read-only. An item's id is its absolute path.
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
    /// The server has `statvfs@openssh.com`.
    statvfs: bool,
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

/// Asks the server every 5 seconds whether it is there, and cancels `ended` when the server has been quiet for 15 seconds: no answer to the ask, and no SFTP bytes since the ask or since the last bytes before it. A connection that died silently, as after a laptop sleeps, would otherwise look open until TCP gives up, and every call on it would wait out the core's 30-second limit. It ends within 20 seconds, so the call waiting on it runs again on a new connection.
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
        // The core's call time limit decides how long a request waits. russh-sftp's own default of 10 seconds would fail a slow read of a large piece.
        russh_sftp::client::Config {
            request_timeout_secs: 60,
            ..Default::default()
        },
    );
    let version = ask(&ended, sftp.init())
        .await?
        .map_err(|_| VfsError::Other(COULD_NOT_START.into()))?;
    let offers = |name: &str, value: &str| version.extensions.get(name).is_some_and(|v| v == value);
    let read_len = if offers(LIMITS, "1") {
        match ask(&ended, sftp.limits()).await? {
            // Capped, so the pieces one read keeps going hold at most 2 MiB.
            Ok(limits) if limits.max_read_len > 0 => limits.max_read_len.min(256 * 1024) as u32,
            _ => SMALL_READ,
        }
    } else {
        SMALL_READ
    };
    let conn = Arc::new(Connection {
        statvfs: offers(STATVFS, "2"),
        sftp,
        ended,
        provider: Arc::downgrade(inner),
        read_len,
    });
    tokio::spawn(ask_whether_there(ssh, conn.ended.clone(), heard));
    Ok(conn)
}

/// An open file or folder handle. Dropping it sends the close from a task of its own.
struct OpenHandle {
    conn: Arc<Connection>,
    handle: String,
}

impl Drop for OpenHandle {
    fn drop(&mut self) {
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
            revision: None,
            read_only: false,
        },
        id: ItemId(id),
    })
}

impl SftpProvider {
    pub fn new(account: Arc<SftpAccount>) -> Self {
        Self {
            inner: Arc::new(Inner {
                account,
                slot: Mutex::new(Slot::Empty),
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

    /// Runs `body` on the connection. A `Lost` retires that connection, since a server can answer `ConnectionLost` while the connection still looks open. A `Lost` on a connection that was there before the call runs `body` once more on a new one. Every call here only reads, so a repeat is safe.
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
                let path = match folder {
                    "/" => format!("/{}", entry.filename),
                    _ => format!("{folder}/{}", entry.filename),
                };
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
}
