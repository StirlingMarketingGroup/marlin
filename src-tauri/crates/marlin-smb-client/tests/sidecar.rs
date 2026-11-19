#![cfg(all(unix, feature = "client"))]

use marlin_smb_client::{CallError, Sidecar, StartError};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn script(body: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("marlin-smb");
    write_script(&path, body);
    (dir, path)
}

fn answer_line() -> &'static str {
    r#"id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')"#
}

fn call<R: serde::de::DeserializeOwned>(
    sidecar: &Sidecar,
    limit: Option<Duration>,
) -> Result<R, CallError> {
    sidecar.call("ping", serde_json::json!({}), limit)
}

#[test]
fn a_call_returns_the_scripts_result() {
    let (_dir, path) = script(&format!(
        r#"
read line
{id}
printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":42}}"
"#,
        id = answer_line()
    ));
    let sidecar = Sidecar::new(Some(path));
    let result: i64 = call(&sidecar, Some(Duration::from_secs(2))).unwrap();
    assert_eq!(result, 42);
}

#[test]
fn an_error_answer_keeps_the_process_for_the_next_call() {
    let (_dir, path) = script(&format!(
        r#"
n=0
while read line; do
  n=$((n + 1))
  {id}
  if [ "$n" -eq 1 ]; then
    printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"error\":{{\"code\":-1002,\"message\":\"nope\"}}}}"
  else
    printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":$n}}"
  fi
done
"#,
        id = answer_line()
    ));
    let sidecar = Sidecar::new(Some(path));
    let err = call::<i64>(&sidecar, Some(Duration::from_secs(2))).unwrap_err();
    assert_eq!(
        err,
        CallError::Failed {
            code: -1002,
            message: "nope".to_string(),
        }
    );
    assert_eq!(err.to_string(), "nope");
    let second: i64 = call(&sidecar, Some(Duration::from_secs(2))).unwrap();
    assert_eq!(second, 2);
}

#[test]
fn a_hung_call_times_out_and_the_next_call_starts_a_new_process() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let path = dir.path().join("marlin-smb");
    write_script(
        &path,
        &format!(
            r#"
marker={marker}
if [ -f "$marker" ]; then
  read line
  {id}
  printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":1}}"
else
  touch "$marker"
  read line
  sleep 600
fi
"#,
            marker = shell_quote(&marker),
            id = answer_line()
        ),
    );

    let sidecar = Sidecar::new(Some(path));
    let started = Instant::now();
    let err = call::<i64>(&sidecar, Some(Duration::from_millis(300))).unwrap_err();
    assert_eq!(err, CallError::TimedOut);
    assert_eq!(err.to_string(), "The SMB server did not answer in time.");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "timed out too slowly: {:?}",
        started.elapsed()
    );

    let result: i64 = call(&sidecar, Some(Duration::from_secs(2))).unwrap();
    assert_eq!(result, 1);
}

#[test]
fn a_process_that_exits_after_one_answer_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("count");
    let pid_file = dir.path().join("pid");
    let path = dir.path().join("marlin-smb");
    write_script(
        &path,
        &format!(
            r#"
echo $$ > {pid}
marker={marker}
n=0
if [ -f "$marker" ]; then
  n=$(cat "$marker")
fi
n=$((n + 1))
printf '%s\n' "$n" > "$marker"
read line
{id}
printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":$n}}"
exit 0
"#,
            pid = shell_quote(&pid_file),
            marker = shell_quote(&marker),
            id = answer_line()
        ),
    );

    let sidecar = Sidecar::new(Some(path));
    let first: i64 = call(&sidecar, Some(Duration::from_secs(2))).unwrap();
    assert_eq!(first, 1);
    // The next call sees the exit with try_wait, so wait until this process is gone.
    let pid = wait_for_file(&pid_file);
    wait_until_exited(&pid);
    let second: i64 = call(&sidecar, Some(Duration::from_secs(2))).unwrap();
    assert_eq!(second, 2);
}

#[test]
fn a_process_that_exits_during_a_call_is_lost() {
    let (_dir, path) = script(
        r#"
read line
exit 0
"#,
    );
    let sidecar = Sidecar::new(Some(path));
    let err = call::<i64>(&sidecar, Some(Duration::from_secs(2))).unwrap_err();
    assert_eq!(err, CallError::Lost);
    assert_eq!(err.to_string(), "SMB connection lost. Please try again.");
}

#[test]
fn a_missing_binary_is_not_installed() {
    let sidecar = Sidecar::new(None);
    let err = call::<i64>(&sidecar, Some(Duration::from_millis(300))).unwrap_err();
    assert_eq!(err, CallError::Unavailable(StartError::NotInstalled));
    assert_eq!(
        err.to_string(),
        "SMB support not available. The marlin-smb helper is missing."
    );
}

#[test]
fn a_library_load_failure_is_reported() {
    let (_dir, path) = script(
        r#"
echo "Library not loaded: libsmbclient" >&2
exit 1
"#,
    );
    let sidecar = Sidecar::new(Some(path));
    assert_eq!(sidecar.start(), Err(StartError::LibraryMissing));
}

#[test]
fn a_wrong_id_ends_the_process_and_the_next_call_starts_a_new_one() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("wrong");
    let path = dir.path().join("marlin-smb");
    write_script(
        &path,
        &format!(
            r#"
marker={marker}
if [ -f "$marker" ]; then
  read line
  {id}
  printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":7}}"
else
  touch "$marker"
  read line
  printf '%s\n' '{{"jsonrpc":"2.0","id":99999,"result":1}}'
  sleep 600
fi
"#,
            marker = shell_quote(&marker),
            id = answer_line()
        ),
    );

    let sidecar = Sidecar::new(Some(path));
    let err = call::<i64>(&sidecar, Some(Duration::from_secs(2))).unwrap_err();
    assert_eq!(
        err,
        CallError::BadReply("SMB protocol error: response ID mismatch".to_string())
    );
    let result: i64 = call(&sidecar, Some(Duration::from_secs(2))).unwrap();
    assert_eq!(result, 7);
}

#[test]
fn a_hung_call_kills_the_child_process_too() {
    let dir = tempfile::tempdir().unwrap();
    let child_pid = dir.path().join("child.pid");
    let path = dir.path().join("marlin-smb");
    write_script(
        &path,
        &format!(
            r#"
sleep 600 &
echo $! > {pid}
read line
sleep 600
"#,
            pid = shell_quote(&child_pid)
        ),
    );

    let sidecar = Sidecar::new(Some(path));
    let err = call::<i64>(&sidecar, Some(Duration::from_millis(300))).unwrap_err();
    assert_eq!(err, CallError::TimedOut);

    // A killed child that its new parent has not reaped yet still answers `kill -0`.
    wait_until_exited(&wait_for_file(&child_pid));
}

/// A `sh` child writes the script, so this test process never holds it open for writing.
/// On Linux a process that another test is starting at that moment would inherit the
/// handle, and running the script would then fail with "Text file busy".
fn write_script(path: &Path, body: &str) {
    let mut writer = Command::new("/bin/sh")
        .args(["-c", r#"cat > "$1" && chmod 755 "$1""#, "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    write!(writer.stdin.take().unwrap(), "#!/bin/sh\n{body}").unwrap();
    assert!(
        writer.wait().unwrap().success(),
        "could not write {}",
        path.display()
    );
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn wait_for_file(path: &Path) -> String {
    for _ in 0..50 {
        if let Ok(text) = fs::read_to_string(path) {
            let text = text.trim().to_string();
            if !text.is_empty() {
                return text;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("nothing was written to {}", path.display());
}

/// A dead child stays visible to `kill -0` until its parent waits. `ps` shows `Z` once it has exited.
fn wait_until_exited(pid: &str) {
    for _ in 0..50 {
        let output = Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() || stat.trim().is_empty() || stat.contains('Z') {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("process {pid} did not exit");
}
