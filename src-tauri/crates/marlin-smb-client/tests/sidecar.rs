#![cfg(all(unix, feature = "client"))]

mod common;

use common::{answer_line, shell_quote, wait_for_file, wait_until_exited, write_script};
use marlin_smb_client::protocol::Notice;
use marlin_smb_client::{CallError, Sidecar, StartError};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn script(body: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("marlin-smb");
    write_script(&path, body);
    (dir, path)
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
fn a_stopped_call_ends_its_process_and_the_next_call_starts_a_new_one() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let pid_file = dir.path().join("pid");
    let path = dir.path().join("marlin-smb");
    write_script(
        &path,
        &format!(
            r#"
marker={marker}
pid={pid}
if [ -f "$marker" ]; then
  read line
  {id}
  printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":1}}"
else
  touch "$marker"
  read line
  echo $$ > "$pid"
  sleep 600
fi
"#,
            marker = shell_quote(&marker),
            pid = shell_quote(&pid_file),
            id = answer_line()
        ),
    );

    let sidecar = Sidecar::new(Some(path));
    let stop = AtomicBool::new(false);
    let (err, pid) = std::thread::scope(|scope| {
        // The stop comes once the script has the request, so it ends a call in flight.
        let stopper = scope.spawn(|| {
            let pid = wait_for_file(&pid_file);
            stop.store(true, Ordering::Relaxed);
            (pid, Instant::now())
        });
        // A limit of its own, so a stop that does not work fails as `TimedOut` and never waits out the script.
        let err = sidecar
            .call_until::<serde_json::Value, i64>(
                "ping",
                serde_json::json!({}),
                Some(Duration::from_secs(10)),
                &stop,
            )
            .unwrap_err();
        let (pid, stopped) = stopper.join().unwrap();
        assert!(
            stopped.elapsed() < Duration::from_secs(3),
            "stopped too slowly: {:?}",
            stopped.elapsed()
        );
        (err, pid)
    });
    assert_eq!(err, CallError::Stopped);
    assert_eq!(err.to_string(), "The SMB call was stopped.");
    wait_until_exited(&pid);

    let result: i64 = call(&sidecar, Some(Duration::from_secs(2))).unwrap();
    assert_eq!(result, 1);
}

#[test]
fn a_call_stopped_before_it_is_sent_keeps_the_process() {
    let dir = tempfile::tempdir().unwrap();
    let requests = dir.path().join("requests");
    let path = dir.path().join("marlin-smb");
    // Each answer is the pid of the process that read the request.
    write_script(
        &path,
        &format!(
            r#"
while read line; do
  printf '%s\n' "$line" >> {requests}
  {id}
  printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":$$}}"
done
"#,
            requests = shell_quote(&requests),
            id = answer_line()
        ),
    );

    let sidecar = Sidecar::new(Some(path));
    let first: i64 = call(&sidecar, Some(Duration::from_secs(2))).unwrap();
    let stopped = sidecar.call_until::<serde_json::Value, i64>(
        "ping",
        serde_json::json!({}),
        Some(Duration::from_secs(2)),
        &AtomicBool::new(true),
    );
    assert_eq!(stopped.unwrap_err(), CallError::Stopped);
    let third: i64 = call(&sidecar, Some(Duration::from_secs(2))).unwrap();
    assert_eq!(third, first);
    assert_eq!(fs::read_to_string(&requests).unwrap().lines().count(), 2);
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

#[test]
fn notices_reach_the_caller() {
    let (_dir, path) = script(&format!(
        r#"
read line
{id}
printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"method\":\"staged\",\"params\":{{\"id\":$id}}}}"
printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"method\":\"progress\",\"params\":{{\"id\":$id,\"bytes\":3}}}}"
printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"method\":\"progress\",\"params\":{{\"id\":99,\"bytes\":99}}}}"
printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"method\":\"progress\",\"params\":{{\"id\":$id,\"bytes\":7}}}}"
printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":42}}"
"#,
        id = answer_line()
    ));
    let sidecar = Sidecar::new(Some(path));
    let stop = AtomicBool::new(false);
    let mut heard = Vec::new();
    let result: i64 = sidecar
        .call_with_notices("ping", serde_json::json!({}), &stop, &mut |notice| {
            heard.push(notice);
        })
        .unwrap();
    assert_eq!(result, 42);
    assert_eq!(
        heard,
        [
            Notice::Staged { id: 1 },
            Notice::Progress { id: 1, bytes: 3 },
            Notice::Progress { id: 1, bytes: 7 }
        ]
    );
}

/// The stop flag is read before every line, so a stop set while one notice is heard ends the call even with the next notice and the answer already waiting.
#[test]
fn a_stop_ends_a_call_that_keeps_sending_lines() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let path = dir.path().join("marlin-smb");
    write_script(
        &path,
        &format!(
            r#"
read line
echo $$ > {pid}
{id}
printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"method\":\"progress\",\"params\":{{\"id\":$id,\"bytes\":1}}}}"
printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"method\":\"progress\",\"params\":{{\"id\":$id,\"bytes\":2}}}}"
printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":42}}"
sleep 600
"#,
            pid = shell_quote(&pid_file),
            id = answer_line()
        ),
    );

    let sidecar = Sidecar::new(Some(path));
    let stop = AtomicBool::new(false);
    let mut heard = 0;
    let err = sidecar
        .call_with_notices::<serde_json::Value, i64>(
            "ping",
            serde_json::json!({}),
            &stop,
            &mut |_| {
                heard += 1;
                stop.store(true, Ordering::Relaxed);
            },
        )
        .unwrap_err();
    assert_eq!(err, CallError::Stopped);
    assert_eq!(heard, 1);
    wait_until_exited(&wait_for_file(&pid_file));
}
