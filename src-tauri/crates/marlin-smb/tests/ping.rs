use std::io::{BufRead, Write};
use std::process::{Command, Stdio};

#[test]
fn ping() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_marlin-smb"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn marlin-smb");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "ping",
    });
    writeln!(stdin, "{request}").expect("write request");

    let mut line = String::new();
    std::io::BufReader::new(stdout)
        .read_line(&mut line)
        .expect("read response");

    let response: serde_json::Value = serde_json::from_str(&line).expect("parse response");
    assert_eq!(response["id"], 1);
    assert_eq!(response["result"]["version"], env!("CARGO_PKG_VERSION"));

    drop(stdin);
    let status = child.wait().expect("wait");
    assert!(status.success());
}
