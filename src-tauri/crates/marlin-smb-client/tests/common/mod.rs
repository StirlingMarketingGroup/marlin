//! Helpers for the tests that run fake `marlin-smb` scripts.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

/// The shell line that sets `id` to the request's id.
pub fn answer_line() -> &'static str {
    r#"id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')"#
}

/// A `sh` child writes the script, so this test process never holds it open for writing.
/// On Linux a process that another test is starting at that moment would inherit the
/// handle, and running the script would then fail with "Text file busy".
pub fn write_script(path: &Path, body: &str) {
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

pub fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

/// Waits up to 5 seconds, so a slow CI runner has time to start the script.
pub fn wait_for_file(path: &Path) -> String {
    for _ in 0..500 {
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

/// A dead child stays visible to `kill -0` until its parent waits. `ps` shows `Z` once it has exited. Waits up to 5 seconds.
pub fn wait_until_exited(pid: &str) {
    for _ in 0..500 {
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
