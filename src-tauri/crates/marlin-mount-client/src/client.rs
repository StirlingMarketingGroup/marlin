//! The app's side of the helper's socket.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::sync::oneshot;

use crate::protocol::{
    methods, HelloParams, HelloResult, MountStatus, Request, Response, RpcError, JSONRPC_VERSION,
    PROTOCOL_VERSION,
};

/// How long to wait for a helper that was just started to take a connection.
const START_WAIT: Duration = Duration::from_secs(10);

/// Pause between those attempts.
const START_RETRY: Duration = Duration::from_millis(50);

/// How long to wait for the answer to `hello`.
const HELLO_WAIT: Duration = Duration::from_secs(30);

/// A log past this size is renamed aside, so the file cannot grow without limit.
const LOG_LIMIT: u64 = 1024 * 1024;

/// How to reach the helper, and how to start one.
pub struct Start {
    /// The helper binary.
    pub binary: PathBuf,
    /// The socket the helper listens on.
    pub socket: PathBuf,
    /// The mount folder. `None` leaves the helper's default, `~/Marlin`.
    pub root: Option<PathBuf>,
    /// The file that gets the helper's log lines (its stderr).
    pub log: PathBuf,
}

/// What a connection reports besides responses.
pub enum Event {
    /// A mount changed state.
    Changed(MountStatus),
    /// The connection ended. Every mount's state is unknown until the next `list`.
    Closed,
}

#[derive(Debug)]
pub enum ClientError {
    /// No helper binary at `Start::binary`.
    MissingBinary(PathBuf),
    /// The helper could not be started, or did not answer in time.
    Start(String),
    /// The socket could not be opened, for a reason that does not mean "no helper is running". Nothing was started.
    Unreachable(String),
    /// A helper that speaks another protocol version could not be replaced.
    OtherVersion(String),
    /// The helper answered a request with an error.
    Rpc(RpcError),
    /// The helper sent an answer that could not be read as the type asked for.
    BadAnswer(String),
    /// The connection ended before the answer came.
    Closed,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingBinary(path) => write!(
                f,
                "Marlin's mount helper is missing at {}. Reinstall Marlin.",
                path.display()
            ),
            Self::Start(message) | Self::Unreachable(message) | Self::OtherVersion(message) => {
                f.write_str(message)
            }
            Self::Rpc(error) => f.write_str(&error.message),
            Self::BadAnswer(message) => write!(
                f,
                "The mount helper sent an answer that could not be read: {message}"
            ),
            Self::Closed => f.write_str("The mount helper closed the connection."),
        }
    }
}

impl std::error::Error for ClientError {}

/// One connection to the helper. Requests on it run at the same time, so each answer finds its caller by id.
pub struct Client {
    writer: tokio::sync::Mutex<OwnedWriteHalf>,
    next_id: AtomicU64,
    /// `Some` while the connection is open. `None` after it ends, so a call cannot register a waiter once the reader has given up.
    waiting: Arc<std::sync::Mutex<Option<HashMap<u64, oneshot::Sender<Response>>>>>,
}

impl Client {
    /// Connects to the helper on `start.socket`, and starts one when none is running.
    ///
    /// A helper that speaks another protocol version is asked to unmount and exit, and this app's own helper takes its place. That happens once, never in a loop.
    pub async fn connect(
        start: &Start,
        on_event: impl Fn(Event) + Send + 'static,
    ) -> Result<Client, ClientError> {
        let mut connection = open_or_start(start).await?;
        let mut protocol = connection.hello().await?;
        if protocol != PROTOCOL_VERSION {
            // A helper that cannot unmount keeps running: a mount it still serves must not lose its helper.
            connection
                .stop()
                .await
                .map_err(|error| ClientError::OtherVersion(error.to_string()))?;
            connection = open_or_start(start).await?;
            protocol = connection.hello().await?;
            if protocol != PROTOCOL_VERSION {
                return Err(ClientError::OtherVersion(format!(
                    "The mount helper at {} speaks protocol {protocol}, and Marlin speaks protocol {PROTOCOL_VERSION}. Reinstall Marlin.",
                    start.binary.display()
                )));
            }
        }
        // The reader task takes the same reader that read `hello`. A notification can already sit in its buffer, and a second reader would lose it.
        let Connection {
            reader,
            writer,
            next_id,
        } = connection;

        let waiting = Arc::new(std::sync::Mutex::new(Some(HashMap::new())));
        let incoming = Arc::clone(&waiting);
        tokio::spawn(async move {
            read_incoming(reader, incoming, on_event).await;
        });
        Ok(Client {
            writer: tokio::sync::Mutex::new(writer),
            next_id: AtomicU64::new(next_id),
            waiting,
        })
    }

    /// Sends one request and waits for its answer. There is no time limit: a `mount` can take long.
    pub async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: impl Serialize,
    ) -> Result<T, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let line = encode(id, method, &params);
        let (sender, receiver) = oneshot::channel();
        {
            let mut waiting = self.waiting.lock().unwrap();
            let Some(waiting) = waiting.as_mut() else {
                return Err(ClientError::Closed);
            };
            waiting.insert(id, sender);
        }
        {
            let mut writer = self.writer.lock().await;
            if writer.write_all(&line).await.is_err() {
                if let Some(waiting) = self.waiting.lock().unwrap().as_mut() {
                    waiting.remove(&id);
                }
                return Err(ClientError::Closed);
            }
        }
        let response = receiver.await.map_err(|_| ClientError::Closed)?;
        if let Some(error) = response.error {
            return Err(ClientError::Rpc(error));
        }
        // A JSON `null` result is `None` here, which is the success of `unmount` and of a `status` with no mount.
        serde_json::from_value(response.result.unwrap_or(Value::Null))
            .map_err(|error| ClientError::BadAnswer(error.to_string()))
    }

    pub fn is_closed(&self) -> bool {
        self.waiting.lock().unwrap().is_none()
    }
}

/// Asks the helper on `socket` to stop. Does not start one.
///
/// `Ok(false)` means nothing is listening. `Ok(true)` means it stopped.
pub async fn shutdown(socket: &Path) -> Result<bool, ClientError> {
    let Some(mut connection) = Connection::open(socket).await? else {
        return Ok(false);
    };
    connection.hello().await?;
    connection.stop().await?;
    Ok(true)
}

/// A connection in its first steps, before a `Client` takes it over. It sends one request at a time.
struct Connection {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    /// The id of the next request. The helper answers a line it cannot parse with id 0, so ids start at 1.
    next_id: u64,
}

impl Connection {
    /// `None` means no helper is running: the socket file is missing, or a killed helper left it behind. Any other error is no reason to start a second process.
    async fn open(socket: &Path) -> Result<Option<Connection>, ClientError> {
        match UnixStream::connect(socket).await {
            Ok(stream) => {
                let (reader, writer) = stream.into_split();
                Ok(Some(Connection {
                    reader: BufReader::new(reader),
                    writer,
                    next_id: 1,
                }))
            }
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::NotFound | ErrorKind::ConnectionRefused
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(ClientError::Unreachable(format!(
                "The mount helper's socket at {} could not be opened: {error}",
                socket.display()
            ))),
        }
    }

    /// Sends one request and reads lines until the one with its `id`. After a matching `hello` the helper mixes `mountChanged` notifications in with its answers, so one can sit in front of the answer.
    async fn request(
        &mut self,
        method: &str,
        params: &impl Serialize,
    ) -> Result<Value, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        self.writer
            .write_all(&encode(id, method, params))
            .await
            .map_err(|_| ClientError::Closed)?;
        let mut line = String::new();
        loop {
            line.clear();
            match self.reader.read_line(&mut line).await {
                Ok(0) | Err(_) => return Err(ClientError::Closed),
                Ok(_) => {}
            }
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            let response: Response = serde_json::from_value(value)
                .map_err(|error| ClientError::BadAnswer(error.to_string()))?;
            return match response.error {
                Some(error) => Err(ClientError::Rpc(error)),
                None => Ok(response.result.unwrap_or(Value::Null)),
            };
        }
    }

    /// Sends `hello` and returns the helper's protocol version. The helper answers only after its start-up sweep.
    async fn hello(&mut self) -> Result<u32, ClientError> {
        let params = HelloParams {
            protocol: PROTOCOL_VERSION,
        };
        let result = tokio::time::timeout(HELLO_WAIT, self.request(methods::HELLO, &params))
            .await
            .map_err(|_| {
                ClientError::Start("The mount helper did not answer hello.".to_string())
            })??;
        let hello: HelloResult = serde_json::from_value(result)
            .map_err(|error| ClientError::BadAnswer(error.to_string()))?;
        Ok(hello.protocol)
    }

    /// Asks the helper to unmount everything and exit, which any protocol version may ask. Returns once the helper has closed the connection: its lock is free by then, so another helper can start.
    async fn stop(mut self) -> Result<(), ClientError> {
        self.request(methods::SHUTDOWN, &()).await?;
        let mut line = String::new();
        while self
            .reader
            .read_line(&mut line)
            .await
            .is_ok_and(|read| read > 0)
        {
            line.clear();
        }
        Ok(())
    }
}

/// Connects to the helper, and starts one first when none is running.
async fn open_or_start(start: &Start) -> Result<Connection, ClientError> {
    if let Some(connection) = Connection::open(&start.socket).await? {
        return Ok(connection);
    }
    start_helper(start)?;
    let deadline = tokio::time::Instant::now() + START_WAIT;
    loop {
        if let Some(connection) = Connection::open(&start.socket).await? {
            return Ok(connection);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ClientError::Start(format!(
                "The mount helper did not take a connection at {}.",
                start.socket.display()
            )));
        }
        tokio::time::sleep(START_RETRY).await;
    }
}

/// Starts the helper and reaps it. A helper that finds another one running exits with status 0 at once; that exit is reaped here, and the connect retry reaches the one that is running.
fn start_helper(start: &Start) -> Result<(), ClientError> {
    if !start.binary.is_file() {
        return Err(ClientError::MissingBinary(start.binary.clone()));
    }
    if let Some(folder) = start.log.parent() {
        std::fs::create_dir_all(folder).map_err(|error| {
            ClientError::Start(format!(
                "The mount helper's log folder could not be created: {error}"
            ))
        })?;
    }
    if std::fs::metadata(&start.log).is_ok_and(|found| found.len() > LOG_LIMIT) {
        let mut old = start.log.clone().into_os_string();
        old.push(".old");
        // A log that cannot be moved aside keeps growing. That must not stop the helper from starting.
        let _ = std::fs::rename(&start.log, old);
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&start.log)
        .map_err(|error| {
            ClientError::Start(format!(
                "The mount helper's log file could not be opened: {error}"
            ))
        })?;

    let mut command = Command::new(&start.binary);
    command.arg("--socket").arg(&start.socket);
    if let Some(root) = &start.root {
        command.arg("--root").arg(root);
    }
    // Stderr is the log file, never a pipe: a full pipe that nobody reads would block the helper. Its own process group keeps a Ctrl+C in the dev terminal from reaching it, and `kill_on_drop` stays off so dropping this client does not kill it.
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(|error| {
            ClientError::Start(format!("The mount helper could not be started: {error}"))
        })?;
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(())
}

async fn read_incoming(
    mut reader: BufReader<OwnedReadHalf>,
    waiting: Arc<std::sync::Mutex<Option<HashMap<u64, oneshot::Sender<Response>>>>>,
    on_event: impl Fn(Event),
) {
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            let Ok(response) = serde_json::from_value::<Response>(value) else {
                continue;
            };
            let sender = waiting
                .lock()
                .unwrap()
                .as_mut()
                .and_then(|waiting| waiting.remove(&id));
            if let Some(sender) = sender {
                let _ = sender.send(response);
            }
            continue;
        }
        if value.get("method").and_then(Value::as_str) == Some(methods::MOUNT_CHANGED) {
            let Some(params) = value.get("params").cloned() else {
                continue;
            };
            if let Ok(status) = serde_json::from_value::<MountStatus>(params) {
                on_event(Event::Changed(status));
            }
        }
    }
    // Closes the connection. Dropping the senders makes every waiting call return `Closed`.
    waiting.lock().unwrap().take();
    on_event(Event::Closed);
}

fn encode(id: u64, method: &str, params: &impl Serialize) -> Vec<u8> {
    let request = Request {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id,
        method: method.to_string(),
        // A null params value is left off the line. `()` serializes as null, which is a request with no params.
        params: serde_json::to_value(params).unwrap(),
    };
    let mut line = serde_json::to_string(&request).unwrap();
    line.push('\n');
    line.into_bytes()
}
