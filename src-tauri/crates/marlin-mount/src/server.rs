//! The Unix socket, the single-instance lock, connections, and request dispatch.

use std::fs::File;
use std::future::Future;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedReadHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc, oneshot, Notify};

use crate::log;
use crate::mounts::{Adapter, Mounts, Providers};
use crate::protocol::{
    error_codes, methods, AccountParams, HelloParams, HelloResult, MountParams, Request, Response,
    RpcError, JSONRPC_VERSION, PROTOCOL_VERSION,
};

/// The longest request line a client may send, in bytes.
const MAX_LINE: usize = 64 * 1024;

/// Where the helper listens, and the folder that holds its mount points.
pub struct Config {
    pub socket: PathBuf,
    pub root: PathBuf,
}

/// Why the helper could not take the socket.
#[derive(Debug)]
pub enum BindError {
    /// Another helper holds the lock. It owns the mounts, and a second helper must never take them over.
    AlreadyRunning,
    Io(std::io::Error),
}

impl std::fmt::Display for BindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRunning => f.write_str("another marlin-mount is already running"),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for BindError {}

impl From<std::io::Error> for BindError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

struct Shared {
    mounts: Arc<Mounts>,

    /// The user who owns the socket. Only this user may connect.
    uid: u32,

    /// Told once, when a `shutdown` request has succeeded and been answered.
    stop: Notify,
}

/// A helper that holds the lock and the socket. `run` serves it.
pub struct Server {
    listener: UnixListener,
    socket: PathBuf,
    shared: Arc<Shared>,

    /// Held for the life of the server. The OS drops the lock when the process dies, so a crashed helper never blocks the next one.
    _lock: File,
}

impl Server {
    pub fn bind(
        config: Config,
        adapter: Arc<dyn Adapter>,
        providers: Arc<dyn Providers>,
    ) -> Result<Self, BindError> {
        if let Some(folder) = config.socket.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let mut lock_path = config.socket.clone().into_os_string();
        lock_path.push(".lock");
        let lock = File::options()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(lock_path)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(BindError::AlreadyRunning),
            Err(std::fs::TryLockError::Error(error)) => return Err(BindError::Io(error)),
        }
        // With the lock held, a socket file here was left by a helper that died. Any other file is not the helper's to remove, and the bind below fails on it.
        if std::fs::symlink_metadata(&config.socket)
            .is_ok_and(|found| found.file_type().is_socket())
        {
            std::fs::remove_file(&config.socket)?;
        }
        let listener = UnixListener::bind(&config.socket)?;
        std::fs::set_permissions(&config.socket, std::fs::Permissions::from_mode(0o600))?;
        let uid = std::fs::metadata(&config.socket)?.uid();
        log::write(
            "info",
            "helper.start",
            json!({"socket": config.socket.display().to_string(), "root": config.root.display().to_string()}),
        );
        Ok(Self {
            listener,
            socket: config.socket,
            shared: Arc::new(Shared {
                mounts: Arc::new(Mounts::new(config.root, adapter, providers)),
                uid,
                stop: Notify::new(),
            }),
            _lock: lock,
        })
    }

    /// Serves connections until a `shutdown` request succeeds or `stop` resolves, then removes the socket file.
    ///
    /// The sweep of the mount folder runs first, before any request is accepted.
    ///
    /// `stop` is a signal. It is an order to exit, so the helper unmounts what it can and returns even if an unmount failed.
    pub async fn run(self, stop: impl Future<Output = ()>) {
        self.shared.mounts.sweep().await;
        tokio::select! {
            _ = stop => {
                if let Err(error) = self.shared.mounts.close().await {
                    log::write("error", "helper.stop_unmount", json!({"message": error.to_string()}));
                }
            }
            _ = self.shared.stop.notified() => {}
            _ = async {
                loop {
                    match self.listener.accept().await {
                        Ok((stream, _)) => {
                            tokio::spawn(serve(stream, Arc::clone(&self.shared)));
                        }
                        // Most likely out of file descriptors. Wait instead of spinning.
                        Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                    }
                }
            } => {}
        }
        let _ = std::fs::remove_file(&self.socket);
        log::write("info", "helper.stop", Value::Null);
    }
}

/// Serves one connection until the client closes it.
async fn serve(stream: UnixStream, shared: Arc<Shared>) {
    // The socket's 0600 mode keeps other users out. This check covers the moment between bind and chmod.
    match stream.peer_cred() {
        Ok(peer) if peer.uid() == shared.uid => {}
        Ok(_) => {
            log::write("warn", "connection.refused", Value::Null);
            return;
        }
        // The client hung up before the check.
        Err(_) => return,
    }
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // The handshake runs in wire order: no other request is answered until a `hello` has been.
    let matched = loop {
        let Some(text) = read_line(&mut reader).await else {
            return;
        };
        let mut protocol = None;
        let response = match parse(&text) {
            Err(response) => response,
            Ok(request) if request.method != methods::HELLO => Response::error(
                request.id,
                error_codes::HELLO_REQUIRED,
                "hello is required first",
            ),
            Ok(request) => match serde_json::from_value::<HelloParams>(request.params) {
                Err(_) => {
                    Response::error(request.id, error_codes::INVALID_PARAMS, "invalid params")
                }
                Ok(params) => {
                    protocol = Some(params.protocol);
                    Response::success(request.id, hello())
                }
            },
        };
        if writer.write_all(line(&response).as_bytes()).await.is_err() {
            return;
        }
        if let Some(protocol) = protocol {
            break protocol == PROTOCOL_VERSION;
        }
    };

    // Every later line goes out through one writer task, so responses and notifications never interleave. A sender that asks is told when its line has been written.
    let (outgoing, mut queued) = mpsc::channel::<(String, Option<oneshot::Sender<()>>)>(64);
    let writer_task = tokio::spawn(async move {
        while let Some((line, written)) = queued.recv().await {
            if writer.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if let Some(written) = written {
                let _ = written.send(());
            }
        }
    });

    // Only a client on this protocol version gets notifications.
    let events_task = matched.then(|| {
        let mut events = shared.mounts.subscribe();
        let outgoing = outgoing.clone();
        tokio::spawn(async move {
            loop {
                let status = match events.recv().await {
                    Ok(status) => status,
                    // A client this far behind has missed changes. It can ask for `list`.
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return,
                };
                let notification = json!({"jsonrpc": JSONRPC_VERSION, "method": methods::MOUNT_CHANGED, "params": status});
                if outgoing.send((line(&notification), None)).await.is_err() {
                    return;
                }
            }
        })
    });

    while let Some(text) = read_line(&mut reader).await {
        let shared = Arc::clone(&shared);
        let outgoing = outgoing.clone();
        // One task per request, so a slow mount does not hold up a `list`. Nothing cancels the task, so an operation finishes even when its client has gone.
        tokio::spawn(async move {
            let (response, stop) = match parse(&text) {
                Err(response) => (response, false),
                Ok(Request {
                    id, method, params, ..
                }) => match answer(&shared.mounts, matched, &method, params).await {
                    Ok(result) => (Response::success(id, result), method == methods::SHUTDOWN),
                    Err(error) => (Response::error(id, error.code, error.message), false),
                },
            };
            let (written, was_written) = oneshot::channel();
            let _ = outgoing.send((line(&response), Some(written))).await;
            if stop {
                // The helper stops only after the client has its answer, or has gone.
                let _ = was_written.await;
                shared.stop.notify_one();
            }
        });
    }

    if let Some(events_task) = events_task {
        events_task.abort();
    }
    // The writer ends when every request task has sent its response.
    drop(outgoing);
    let _ = writer_task.await;
}

/// Runs one request and returns its result.
async fn answer(
    mounts: &Arc<Mounts>,
    matched: bool,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    let invalid_params = |_| RpcError {
        code: error_codes::INVALID_PARAMS,
        message: "invalid params".to_string(),
    };
    Ok(match method {
        methods::SHUTDOWN => {
            mounts.close().await?;
            Value::Null
        }
        // A client on another protocol version may only ask the helper to leave (decision 0001, choice 4).
        _ if !matched => {
            return Err(RpcError {
                code: error_codes::PROTOCOL_MISMATCH,
                message: "protocol version mismatch".to_string(),
            })
        }
        methods::HELLO => json!(hello()),
        methods::MOUNT => {
            let MountParams {
                provider,
                account,
                name,
            } = serde_json::from_value(params).map_err(invalid_params)?;
            json!(mounts.mount(&provider, &account, &name).await?)
        }
        methods::UNMOUNT => {
            let AccountParams { provider, account } =
                serde_json::from_value(params).map_err(invalid_params)?;
            mounts.unmount(&provider, &account).await?;
            Value::Null
        }
        methods::LIST => json!(mounts.list()),
        methods::DEAD_MOUNTS => json!(mounts.dead_mounts().await),
        methods::STATUS => {
            let AccountParams { provider, account } =
                serde_json::from_value(params).map_err(invalid_params)?;
            json!(mounts.status(&provider, &account))
        }
        _ => {
            return Err(RpcError {
                code: error_codes::METHOD_NOT_FOUND,
                message: format!("Method not found: {method}"),
            })
        }
    })
}

fn hello() -> HelloResult {
    HelloResult {
        protocol: PROTOCOL_VERSION,
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

fn parse(text: &str) -> Result<Request, Response> {
    serde_json::from_str(text).map_err(|error| {
        Response::error(0, error_codes::PARSE_ERROR, format!("Parse error: {error}"))
    })
}

fn line(message: &impl serde::Serialize) -> String {
    let mut line = serde_json::to_string(message).unwrap();
    line.push('\n');
    line
}

/// The next line, or `None` when the connection is done: closed, broken, not UTF-8, or sending a line over the cap.
async fn read_line(reader: &mut BufReader<OwnedReadHalf>) -> Option<String> {
    let mut line = String::new();
    // One byte past the cap is enough to tell a line that is too long from one that just fits, and it bounds the buffer.
    let read = reader
        .take(MAX_LINE as u64 + 1)
        .read_line(&mut line)
        .await
        .ok()?;
    if read == 0 {
        return None;
    }
    if line.len() > MAX_LINE && !line.ends_with('\n') {
        log::write("warn", "connection.line_too_long", Value::Null);
        return None;
    }
    Some(line)
}
