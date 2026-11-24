use crate::protocol::{Request, Response, JSONRPC_VERSION};
use serde::{de::DeserializeOwned, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Mutex, TryLockError};
use std::time::{Duration, Instant};

/// Why `marlin-smb` could not start. Each text is what the user reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    /// No `marlin-smb` file was found.
    NotInstalled,
    /// `marlin-smb` exited at once because libsmbclient could not be loaded.
    LibraryMissing,
    /// `marlin-smb` could not start for another reason.
    StartFailed(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::NotInstalled => {
                f.write_str("SMB support not available. The marlin-smb helper is missing.")
            }
            StartError::LibraryMissing => f.write_str(
                "SMB support requires Samba.\nmacOS: brew install samba\nLinux: sudo apt install libsmbclient-dev",
            ),
            StartError::StartFailed(reason) => {
                write!(f, "SMB sidecar failed to start: {}", reason)
            }
        }
    }
}

impl std::error::Error for StartError {}

/// Whether `path` is a `marlin-smb` that can run. A build with no `marlin-smb`, such as one for an Intel Mac, ships an empty placeholder file in its place (`src-tauri/build.rs`).
pub fn is_installed(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|file| file.is_file() && file.len() > 0)
}

/// Why one call to `marlin-smb` failed.
#[derive(Debug, PartialEq, Eq)]
pub enum CallError {
    /// The process could not start.
    Unavailable(StartError),
    /// The call passed its time limit. The process was ended.
    TimedOut,
    /// The caller set the call's stop flag. A request already sent ends its process. One not yet sent is never sent, and the process stays.
    Stopped,
    /// The process died or a pipe broke during the call.
    Lost,
    /// The answer could not be read, or it was for another request. The process was ended,
    /// because it is out of step with the client.
    BadReply(String),
    /// `marlin-smb` answered with a JSON-RPC error. The process stays up.
    Failed { code: i32, message: String },
    /// The params could not be written as JSON, or the result is not the type the caller
    /// asked for. The process stays up.
    Encoding(String),
}

impl CallError {
    /// True when `exchange` returned this error and left the process unable to serve another call.
    fn ends_process(&self) -> bool {
        matches!(
            self,
            CallError::TimedOut | CallError::Stopped | CallError::Lost | CallError::BadReply(_)
        )
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Unavailable(error) => error.fmt(f),
            CallError::TimedOut => f.write_str("The SMB server did not answer in time."),
            CallError::Stopped => f.write_str("The SMB call was stopped."),
            CallError::Lost => f.write_str("SMB connection lost. Please try again."),
            CallError::BadReply(text) | CallError::Encoding(text) => f.write_str(text),
            CallError::Failed { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for CallError {}

/// A started `marlin-smb`. Dropping it ends its process group while `marlin-smb` is alive.
struct Running {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<String>,
}

impl Drop for Running {
    fn drop(&mut self) {
        // `try_wait` reaps a leader that has already exited, and that frees its pid.
        // Signal the group only while the leader is alive, so the pid is still that group.
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        kill_group(&mut self.child);
        let _ = self.child.wait();
    }
}

/// Client for one `marlin-smb` process. It answers one request at a time.
pub struct Sidecar {
    binary: Option<PathBuf>,
    /// The running process, if any.
    process: Mutex<Option<Running>>,
    next_id: AtomicU64,
}

impl Sidecar {
    /// `binary` is the `marlin-smb` file to run. None means it is not installed.
    pub fn new(binary: Option<PathBuf>) -> Self {
        Self {
            binary,
            process: Mutex::new(None),
            next_id: AtomicU64::new(1),
        }
    }

    /// Starts the process when none is running. It never waits behind a call. When a call
    /// holds the lock, a process is up or that call is starting one, and this returns Ok:
    /// that call reports a failed start.
    pub fn start(&self) -> Result<(), StartError> {
        let mut process = match self.process.try_lock() {
            Ok(process) => process,
            Err(TryLockError::Poisoned(err)) => err.into_inner(),
            Err(TryLockError::WouldBlock) => return Ok(()),
        };
        self.running(&mut process).map(|_| ())
    }

    /// Sends one request and waits for its answer. `limit` None waits as long as it takes.
    ///
    /// The caller waits for the call before it with no limit. `limit` starts when this
    /// request is sent.
    pub fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
        limit: Option<Duration>,
    ) -> Result<R, CallError> {
        self.call_until(method, params, limit, &AtomicBool::new(false))
    }

    /// Like [`Self::call`], and another thread can stop it with `stop`. Stopped before the request is sent, the call sends nothing and the process stays. Stopped while it waits for the answer, the call ends within 50 ms, and so does its process.
    pub fn call_until<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
        limit: Option<Duration>,
        stop: &AtomicBool,
    ) -> Result<R, CallError> {
        let mut process = self.process.lock().unwrap_or_else(|err| err.into_inner());
        let running = self.running(&mut process).map_err(CallError::Unavailable)?;
        // A stop can come before the request is sent: while the process starts, or while the caller waited for something else, such as the provider's sign-in. Nothing is sent, and the healthy process stays.
        if stop.load(Ordering::Relaxed) {
            return Err(CallError::Stopped);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let result = exchange(running, id, method, params, limit, stop);
        if result.as_ref().is_err_and(CallError::ends_process) {
            // Dropping the `Running` ends its process group.
            *process = None;
        }
        result
    }

    /// The live process, started first when there is none or the last one exited.
    fn running<'a>(&self, process: &'a mut Option<Running>) -> Result<&'a mut Running, StartError> {
        let alive = process
            .as_mut()
            .is_some_and(|running| matches!(running.child.try_wait(), Ok(None)));
        if !alive {
            *process = Some(self.spawn_process()?);
        }
        Ok(process.as_mut().expect("set above"))
    }

    fn spawn_process(&self) -> Result<Running, StartError> {
        let Some(binary) = &self.binary else {
            log::warn!("SMB sidecar binary not found");
            return Err(StartError::NotInstalled);
        };

        log::info!("Starting SMB sidecar from: {}", binary.display());
        ensure_execute_bit(binary);

        let mut command = Command::new(binary);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // The child leads its own group, so ending it also ends the tools it starts.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);

        let mut child = command.spawn().map_err(|e| {
            log::error!("Failed to spawn sidecar: {}", e);
            StartError::StartFailed(e.to_string())
        })?;

        // Give the sidecar a moment to start or fail.
        std::thread::sleep(Duration::from_millis(100));

        match child.try_wait() {
            Ok(Some(status)) => {
                // It exited at once: most often the dynamic loader could not find libsmbclient.
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                if stderr.contains("dyld")
                    || stderr.contains("Library not loaded")
                    || stderr.contains("libsmb")
                {
                    log::warn!("Sidecar failed with dyld error: {}", stderr);
                    return Err(StartError::LibraryMissing);
                }
                log::error!(
                    "Sidecar exited immediately with status {:?}: {}",
                    status,
                    stderr
                );
                return Err(StartError::StartFailed(format!(
                    "Exited with status: {:?}",
                    status
                )));
            }
            Ok(None) => {}
            Err(e) => {
                log::error!("Failed to check sidecar status: {}", e);
                kill_group(&mut child);
                let _ = child.wait();
                return Err(StartError::StartFailed(e.to_string()));
            }
        }

        // All three are piped above, so `take` returns each one.
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");

        let (tx, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        // Read stderr for the life of the process: a full pipe would block `marlin-smb`.
        // Its lines already start with "[marlin-smb]".
        std::thread::spawn(move || {
            // Split on bytes: `lines()` would stop at the first line that is not UTF-8.
            for line in BufReader::new(stderr).split(b'\n').map_while(Result::ok) {
                log::debug!("{}", String::from_utf8_lossy(&line));
            }
        });

        Ok(Running {
            child,
            stdin,
            replies,
        })
    }
}

/// A stop takes effect within this time.
const STOP_SLICE: Duration = Duration::from_millis(50);

/// Sends one request to a live process and reads its answer.
fn exchange<P: Serialize, R: DeserializeOwned>(
    running: &mut Running,
    id: u64,
    method: &str,
    params: P,
    limit: Option<Duration>,
    stop: &AtomicBool,
) -> Result<R, CallError> {
    let request = Request {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id,
        method: method.to_string(),
        params: serde_json::to_value(&params)
            .map_err(|e| CallError::Encoding(format!("Failed to serialize params: {}", e)))?,
    };
    let line = serde_json::to_string(&request)
        .map_err(|e| CallError::Encoding(format!("Failed to serialize request: {}", e)))?;

    if writeln!(running.stdin, "{line}").is_err() || running.stdin.flush().is_err() {
        log::warn!("Failed to write to sidecar stdin");
        return Err(CallError::Lost);
    }

    let deadline = limit.map(|limit| Instant::now() + limit);
    let line = loop {
        let slice = match deadline {
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(CallError::TimedOut);
                }
                left.min(STOP_SLICE)
            }
            None => STOP_SLICE,
        };
        match running.replies.recv_timeout(slice) {
            Ok(line) => break line,
            Err(RecvTimeoutError::Timeout) if stop.load(Ordering::Relaxed) => {
                return Err(CallError::Stopped);
            }
            // The next turn ends the call when the limit has passed.
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                log::warn!("Failed to read from sidecar stdout");
                return Err(CallError::Lost);
            }
        }
    };

    let response: Response = serde_json::from_str(&line)
        .map_err(|e| CallError::BadReply(format!("Failed to parse response: {}", e)))?;
    if response.id != id {
        log::error!("Response ID mismatch: expected {}, got {}", id, response.id);
        return Err(CallError::BadReply(
            "SMB protocol error: response ID mismatch".to_string(),
        ));
    }
    if let Some(error) = response.error {
        return Err(CallError::Failed {
            code: error.code,
            message: error.message,
        });
    }
    // A method with nothing to return sends `"result": null`, which reads back as `None`.
    serde_json::from_value(response.result.unwrap_or_default())
        .map_err(|e| CallError::Encoding(format!("Failed to parse result: {}", e)))
}

fn ensure_execute_bit(binary: &Path) {
    // Bundling or an update can strip the bit.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(binary) {
            let mode = metadata.permissions().mode();
            if mode & 0o100 == 0 {
                let mut new_perms = metadata.permissions();
                new_perms.set_mode(mode | 0o111);
                match std::fs::set_permissions(binary, new_perms) {
                    Ok(()) => log::warn!(
                        "Sidecar binary lacked execute permission (mode {:o}), fixed to {:o}",
                        mode,
                        mode | 0o111
                    ),
                    Err(e) => log::warn!("Failed to set execute permission on sidecar: {}", e),
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = binary;
}

fn kill_group(child: &mut Child) {
    // `child` leads the group made at spawn, so -pid reaches it and every process it started.
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: kill(2) takes plain integers and touches no memory of ours.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = child.kill();
}
