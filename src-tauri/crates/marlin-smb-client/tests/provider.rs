#![cfg(all(unix, feature = "client"))]

mod common;

use common::{answer_line, shell_quote, wait_for_file, wait_until_exited, write_script};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use marlin_smb_client::protocol::{error_codes, READ_MAX, SMBCLIENT_MISSING};
use marlin_smb_client::{
    add_server, SmbProvider, SmbSignIn, StartError, PROCESSES, UPLOAD_PROCESSES,
};
use marlin_vfs::{ItemId, Kind, Options, Provider, Revision, Space, Vfs, VfsError, POSIX};

const WHEN: &str = "2020-01-02T03:04:05Z";

struct Saved {
    _dir: tempfile::TempDir,
    path: PathBuf,
    sign_in: Arc<SmbSignIn>,
}

fn save(host: &str, password: &str) -> Saved {
    marlin_test_keychain::install();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("smb-servers.json");
    add_server(&path, host.into(), "ana".into(), password.into(), None).unwrap();
    let sign_in = Arc::new(SmbSignIn::new(host, path.clone()));
    Saved {
        _dir: dir,
        path,
        sign_in,
    }
}

struct Fake {
    _dir: tempfile::TempDir,
    binary: PathBuf,
    requests: PathBuf,
}

fn fake(arms: &str) -> Fake {
    let dir = tempfile::tempdir().unwrap();
    let requests = dir.path().join("requests");
    let binary = dir.path().join("marlin-smb");
    let body = format!(
        r#"
requests={requests}
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$requests"
  {id}
  case "$line" in
{arms}
    *)
      {fallback}
      ;;
  esac
done
"#,
        requests = shell_quote(&requests),
        id = answer_line(),
        fallback = print_error(error_codes::SMB_ERROR),
    );
    write_script(&binary, &body);
    Fake {
        _dir: dir,
        binary,
        requests,
    }
}

fn print_result(json: &str) -> String {
    format!(r#"printf '{{"jsonrpc":"2.0","id":%s,"result":%s}}\n' "$id" '{json}'"#)
}

fn print_error(code: i32) -> String {
    format!(
        r#"printf '{{"jsonrpc":"2.0","id":%s,"error":{{"code":{code},"message":"failed"}}}}\n' "$id""#
    )
}

fn shares_json(names: &[&str]) -> String {
    let body = names
        .iter()
        .map(|name| format!(r#"{{"name":"{name}","comment":null}}"#))
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"shares":[{body}]}}"#)
}

fn arm(pattern: &str, body: &str) -> String {
    format!(
        r#"
    {pattern})
      {body}
      ;;"#,
    )
}

fn logged(fake: &Fake) -> String {
    fs::read_to_string(&fake.requests).unwrap_or_default()
}

fn requests(fake: &Fake) -> Vec<serde_json::Value> {
    logged(fake)
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap_or_else(|err| panic!("{err} in {line}")))
        .collect()
}

fn open(saved: &Saved, fake: &Fake) -> SmbProvider {
    SmbProvider::new(Arc::clone(&saved.sign_in), Some(fake.binary.clone()))
}

/// The first call moves the sign-in to Accepted, so later calls can run at once.
async fn accepted(provider: &SmbProvider) {
    provider
        .list(&ItemId("/".into()))
        .await
        .expect("the first call is accepted");
}

fn root_shares(names: &[&str]) -> String {
    arm(
        r#"*'"method":"list_shares"'*"#,
        &print_result(&shares_json(names)),
    )
}

async fn wait_for_lines(path: &Path, count: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = fs::read_to_string(path).unwrap_or_default();
        let lines: Vec<_> = text
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect();
        if lines.len() >= count {
            return lines;
        }
        assert!(
            Instant::now() < deadline,
            "saw {} lines in {}: {text}",
            lines.len(),
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn distinct(mut lines: Vec<String>) -> Vec<String> {
    lines.sort();
    lines.dedup();
    lines
}

#[tokio::test]
async fn the_server_lists_its_shares_as_folders() {
    let listing = fake(&root_shares(&["data", "media"]));
    let saved = save("shares.example", "pw");
    let provider = open(&saved, &listing);

    assert_eq!(provider.root(), ItemId("/".into()));
    let listed = provider.list(&ItemId("/".into())).await.unwrap();
    assert_eq!(listed.len(), 2);
    for (item, name) in listed.iter().zip(["data", "media"]) {
        assert_eq!(item.id, ItemId(format!("/{name}")));
        assert_eq!(item.name, name);
        assert_eq!(item.attributes.kind, Kind::Folder);
        assert_eq!(item.attributes.size, 0);
        assert_eq!(item.attributes.modified, UNIX_EPOCH);
        assert!(item.attributes.revision.is_none());
        assert!(!item.attributes.read_only);
    }

    let before = requests(&listing).len();
    let root = provider.item(&ItemId("/".into())).await.unwrap();
    let sent = requests(&listing);
    assert_eq!(sent.len(), before + 1);
    assert_eq!(sent.last().unwrap()["method"], "list_shares");
    assert_eq!(root.attributes.kind, Kind::Folder);
    assert_eq!(root.name, "");
    assert_eq!(root.attributes.modified, UNIX_EPOCH);

    // A share is what `list_shares` says, as in the listing.
    let share = provider.item(&ItemId("/data".into())).await.unwrap();
    assert_eq!(requests(&listing).last().unwrap()["method"], "list_shares");
    assert_eq!(share, listed[0]);
    assert_eq!(
        provider.item(&ItemId("/gone".into())).await.unwrap_err(),
        VfsError::NotFound
    );

    let down = fake(&arm(
        r#"*'"method":"list_shares"'*"#,
        &print_error(error_codes::SMB_CONNECTION_LOST),
    ));
    let saved = save("shares-down.example", "pw");
    let provider = open(&saved, &down);
    assert_eq!(
        provider.item(&ItemId("/".into())).await.unwrap_err(),
        VfsError::Offline
    );
}

#[tokio::test]
async fn a_folder_lists_its_files_and_folders() {
    let entries = format!(
        r#"{{"entries":[{{"name":"notes.txt","is_directory":false,"is_hidden":false,"size":12,"modified":"{WHEN}","extension":"txt"}},{{"name":"sub","is_directory":true,"is_hidden":false,"size":4096,"modified":"{WHEN}","extension":null}},{{"name":".hidden","is_directory":false,"is_hidden":true,"size":1,"modified":"{WHEN}","extension":null}}]}}"#
    );
    let meta = format!(
        r#"{{"name":"notes.txt","is_directory":false,"is_hidden":false,"size":12,"modified":"{WHEN}","extension":"txt"}}"#
    );
    let script = fake(&format!(
        "{sub}{dir}{file}{shares}",
        sub = arm(r#"*'"path":"/sub"'*"#, &print_result(r#"{"entries":[]}"#)),
        dir = arm(r#"*'"method":"read_directory"'*"#, &print_result(&entries),),
        file = arm(r#"*'"method":"get_file_metadata"'*"#, &print_result(&meta),),
        shares = root_shares(&["data"]),
    ));
    let saved = save("folder.example", "pw");
    let provider = open(&saved, &script);

    let listed = provider.list(&ItemId("/data".into())).await.unwrap();
    assert_eq!(listed.len(), 3);
    assert_eq!(listed[0].id, ItemId("/data/notes.txt".into()));
    assert_eq!(listed[0].attributes.kind, Kind::File);
    assert_eq!(listed[0].attributes.size, 12);
    // WHEN, 2020-01-02T03:04:05Z.
    assert_eq!(
        listed[0].attributes.modified,
        UNIX_EPOCH + Duration::from_secs(1_577_934_245)
    );
    assert_eq!(
        listed[0].attributes.revision,
        Some(Revision(format!("12:{}", 1_577_934_245)))
    );
    assert!(!listed[0].attributes.read_only);
    assert_eq!(listed[1].id, ItemId("/data/sub".into()));
    assert_eq!(listed[1].attributes.kind, Kind::Folder);
    assert_eq!(listed[1].attributes.size, 0);
    assert_eq!(listed[2].id, ItemId("/data/.hidden".into()));
    assert_eq!(listed[2].name, ".hidden");
    assert_eq!(listed[2].attributes.kind, Kind::File);

    let before = requests(&script).len();
    let nested = provider.list(&ItemId("/data/sub".into())).await.unwrap();
    assert!(nested.is_empty());
    let sent = &requests(&script)[before];
    assert_eq!(sent["method"], "read_directory");
    assert_eq!(sent["params"]["share"], "data");
    assert_eq!(sent["params"]["path"], "/sub");

    let notes = provider.item(&listed[0].id).await.unwrap();
    let sent = requests(&script).last().unwrap().clone();
    assert_eq!(sent["method"], "get_file_metadata");
    assert_eq!(sent["params"]["share"], "data");
    assert_eq!(sent["params"]["path"], "/notes.txt");
    assert_eq!(notes.attributes, listed[0].attributes);
    assert_eq!(notes.name, "notes.txt");
}

#[tokio::test]
async fn a_read_longer_than_the_limit_is_split() {
    let script = fake(&format!(
        r#"
    *'"path":"/short"'*)
      data=$(head -c 100 /dev/zero | base64 | tr -d '\n')
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"data":"%s"}}}}\n' "$id" "$data"
      ;;
    *'"method":"read"'*)
      len=$(printf '%s\n' "$line" | sed -n 's/.*"length":\([0-9][0-9]*\).*/\1/p')
      data=$(head -c "$len" /dev/zero | base64 | tr -d '\n')
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"data":"%s"}}}}\n' "$id" "$data"
      ;;
    {shares}
"#,
        shares = root_shares(&["data"]),
    ));
    let saved = save("reads.example", "pw");
    let provider = open(&saved, &script);
    accepted(&provider).await;

    let before = requests(&script).len();
    let empty = provider
        .read(&ItemId("/data/big".into()), 5, 0)
        .await
        .unwrap();
    assert!(empty.is_empty());
    // The server and a share's top are folders, and nothing is asked.
    for folder in ["/", "/data"] {
        assert_eq!(
            provider
                .read(&ItemId(folder.into()), 0, 10)
                .await
                .unwrap_err(),
            VfsError::IsAFolder
        );
    }
    assert_eq!(requests(&script).len(), before);

    let got = provider
        .read(&ItemId("/data/big".into()), 5, READ_MAX + 10)
        .await
        .unwrap();
    // Not `assert_eq!` on the bytes: a failure would print 16 MiB twice.
    assert_eq!(got.len(), READ_MAX as usize + 10);
    assert!(got.iter().all(|byte| *byte == 0));
    let reads: Vec<_> = requests(&script)
        .into_iter()
        .filter(|line| line["method"] == "read")
        .map(|line| {
            (
                line["params"]["offset"].as_u64().unwrap(),
                line["params"]["length"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        reads,
        vec![(5, u64::from(READ_MAX)), (5 + u64::from(READ_MAX), 10)]
    );

    let short = provider
        .read(&ItemId("/data/short".into()), 0, 1000)
        .await
        .unwrap();
    assert_eq!(short, vec![0; 100]);
    let reads: Vec<_> = requests(&script)
        .into_iter()
        .filter(|line| line["method"] == "read" && line["params"]["path"] == "/short")
        .collect();
    assert_eq!(reads.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_call_ends_its_process_and_holds_up_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let script = fake(&format!(
        r#"
    *'"path":"/hang"'*)
      echo $$ > {pid}
      sleep 600
      ;;
    *'"method":"read_directory"'*)
      {entries}
      ;;
    {shares}
"#,
        pid = shell_quote(&pid_file),
        entries = print_result(
            r#"{"entries":[{"name":"notes.txt","is_directory":false,"is_hidden":false,"size":1,"modified":"2020-01-02T03:04:05Z","extension":"txt"}]}"#,
        ),
        shares = root_shares(&["data"]),
    ));
    let saved = save("dropped.example", "pw");
    let provider = Arc::new(open(&saved, &script));
    accepted(&provider).await;

    let hung = {
        let provider = Arc::clone(&provider);
        tokio::spawn(async move { provider.list(&ItemId("/data/hang".into())).await })
    };
    let pid = wait_for_file(&pid_file);
    // The hung call still holds its process while this one runs.
    let listed = tokio::time::timeout(
        Duration::from_secs(5),
        provider.list(&ItemId("/data".into())),
    )
    .await
    .expect("the hung call held up another call")
    .unwrap();
    assert_eq!(listed[0].name, "notes.txt");
    // Aborting the task drops the call, as the core does when the call's limit passes.
    hung.abort();
    wait_until_exited(&pid);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_core_ends_a_hung_call() {
    let dir = tempfile::tempdir().unwrap();
    let answered = dir.path().join("answered");
    let pid_file = dir.path().join("pid");
    // The first `list_shares` answers, so the process is up before the call that hangs.
    let script = fake(&format!(
        r#"
    *'"method":"list_shares"'*)
      if [ -f {answered} ]; then
        echo $$ > {pid}
        sleep 600
      fi
      touch {answered}
      {shares}
      ;;
    *'"method":"space"'*)
      {size}
      ;;
"#,
        answered = shell_quote(&answered),
        pid = shell_quote(&pid_file),
        shares = print_result(&shares_json(&["data"])),
        size = print_result(r#"{"total":1000,"free":100}"#),
    ));
    let saved = save("core-hang.example", "pw");
    let provider = open(&saved, &script);
    let vfs = Vfs::new(
        Arc::new(provider),
        POSIX,
        Options {
            // The warm-up call runs inside this limit too.
            call_timeout: Duration::from_secs(5),
            max_calls: PROCESSES,
            ..Default::default()
        },
    );
    vfs.space().await.unwrap();
    vfs.refresh();
    assert_eq!(vfs.space().await.unwrap_err(), VfsError::TimedOut);
    wait_until_exited(&wait_for_file(&pid_file));
}

fn hang_and_barrier(pid_hang: &Path, pid_barrier: &Path) -> String {
    format!(
        r#"
    *'"path":"/hang"'*)
      echo $$ >> {hang}
      sleep 600
      ;;
    *'"path":"/die"'*)
      exit 1
      ;;
    *'"path":"/barrier"'*)
      echo $$ >> {barrier}
      i=0
      while [ "$(wc -l < {barrier} | tr -d ' ')" -lt 4 ]; do
        i=$((i + 1))
        if [ "$i" -gt 200 ]; then
          exit 1
        fi
        sleep 0.05
      done
      {answer}
      ;;
    {shares}
"#,
        hang = shell_quote(pid_hang),
        barrier = shell_quote(pid_barrier),
        answer = print_result(r#"{"entries":[]}"#),
        shares = root_shares(&["data"]),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn all_processes_busy_does_not_wedge_the_mount() {
    let dir = tempfile::tempdir().unwrap();
    let hang_pids = dir.path().join("hang");
    let barrier_pids = dir.path().join("barrier");
    let script = fake(&hang_and_barrier(&hang_pids, &barrier_pids));
    let saved = save("busy.example", "pw");
    let provider = Arc::new(open(&saved, &script));
    accepted(&provider).await;

    let mut hung = Vec::new();
    for _ in 0..4 {
        let provider = Arc::clone(&provider);
        hung.push(tokio::spawn(async move {
            provider.list(&ItemId("/data/hang".into())).await
        }));
    }
    let pids = distinct(wait_for_lines(&hang_pids, 4).await);
    assert_eq!(pids.len(), 4, "{pids:?}");

    // A fifth call waits for a free process.
    assert_eq!(PROCESSES, 4);
    let mut fifth = {
        let provider = Arc::clone(&provider);
        tokio::spawn(async move { provider.list(&ItemId("/".into())).await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut fifth)
            .await
            .is_err(),
        "a fifth call ran while 4 processes were busy"
    );
    hung[0].abort();
    tokio::time::timeout(Duration::from_secs(5), fifth)
        .await
        .expect("the fifth call got the freed process")
        .unwrap()
        .unwrap();

    for task in &hung {
        task.abort();
    }
    for pid in &pids {
        wait_until_exited(pid);
    }

    let mut checks = Vec::new();
    for _ in 0..4 {
        let provider = Arc::clone(&provider);
        checks.push(tokio::spawn(async move {
            provider
                .list(&ItemId("/data/barrier".into()))
                .await
                .unwrap();
        }));
    }
    for task in checks {
        task.await.unwrap();
    }
    let served = distinct(wait_for_lines(&barrier_pids, 4).await);
    assert_eq!(served.len(), 4, "{served:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_process_that_dies_during_a_call_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let barrier_pids = dir.path().join("barrier");
    let script = fake(&hang_and_barrier(&dir.path().join("hang"), &barrier_pids));
    let saved = save("died.example", "pw");
    let provider = Arc::new(open(&saved, &script));
    accepted(&provider).await;

    assert_eq!(
        provider
            .list(&ItemId("/data/die".into()))
            .await
            .unwrap_err(),
        VfsError::Other("marlin-smb stopped during a call".into())
    );

    let mut checks = Vec::new();
    for _ in 0..4 {
        let provider = Arc::clone(&provider);
        checks.push(tokio::spawn(async move {
            provider
                .list(&ItemId("/data/barrier".into()))
                .await
                .unwrap();
        }));
    }
    for task in checks {
        task.await.unwrap();
    }
    let served = distinct(wait_for_lines(&barrier_pids, 4).await);
    assert_eq!(served.len(), 4, "{served:?}");
}

#[tokio::test]
async fn each_error_code_maps_to_a_vfs_error() {
    let script = fake(&format!(
        "{read}{missing}{denied}{down}{folder}{library}{taken}{holding}{changed}{full}{params}{other}{bad}{shares}",
        read = arm(
            r#"*'"method":"read"'*"#,
            &print_error(error_codes::SMB_IS_A_FOLDER),
        ),
        missing = arm(
            r#"*'"path":"/missing"'*"#,
            &print_error(error_codes::SMB_PATH_NOT_FOUND),
        ),
        denied = arm(
            r#"*'"path":"/denied"'*"#,
            &print_error(error_codes::SMB_PERMISSION_DENIED),
        ),
        down = arm(
            r#"*'"path":"/down"'*"#,
            &print_error(error_codes::SMB_CONNECTION_LOST),
        ),
        folder = arm(
            r#"*'"path":"/notes.txt"'*"#,
            &print_error(error_codes::SMB_NOT_A_FOLDER),
        ),
        library = arm(
            r#"*'"path":"/lib"'*"#,
            &print_error(error_codes::SMB_LIBRARY_MISSING),
        ),
        taken = arm(
            r#"*'"path":"/taken"'*"#,
            &print_error(error_codes::SMB_ALREADY_EXISTS),
        ),
        holding = arm(
            r#"*'"path":"/holding"'*"#,
            &print_error(error_codes::SMB_NOT_EMPTY),
        ),
        changed = arm(
            r#"*'"path":"/changed"'*"#,
            &print_error(error_codes::SMB_CHANGED),
        ),
        full = arm(
            r#"*'"path":"/full"'*"#,
            &print_error(error_codes::SMB_NO_SPACE),
        ),
        params = arm(
            r#"*'"path":"/params"'*"#,
            &print_error(error_codes::INVALID_PARAMS),
        ),
        other = arm(r#"*'"path":"/other"'*"#, &print_error(-1000)),
        bad = arm(r#"*'"path":"/bad"'*"#, "printf 'not-json\\n'"),
        shares = root_shares(&["data"]),
    ));
    let saved = save("errors.example", "pw");
    let provider = open(&saved, &script);

    assert_eq!(
        provider
            .list(&ItemId("/data/missing".into()))
            .await
            .unwrap_err(),
        VfsError::NotFound
    );
    assert_eq!(
        provider
            .list(&ItemId("/data/denied".into()))
            .await
            .unwrap_err(),
        VfsError::AccessDenied
    );
    assert_eq!(
        provider
            .list(&ItemId("/data/down".into()))
            .await
            .unwrap_err(),
        VfsError::Offline
    );
    assert_eq!(
        provider
            .read(&ItemId("/data/folder".into()), 0, 4)
            .await
            .unwrap_err(),
        VfsError::IsAFolder
    );
    assert_eq!(
        provider
            .list(&ItemId("/data/notes.txt".into()))
            .await
            .unwrap_err(),
        VfsError::NotAFolder
    );
    assert_eq!(
        provider
            .list(&ItemId("/data/lib".into()))
            .await
            .unwrap_err(),
        VfsError::Other(SMBCLIENT_MISSING.into())
    );
    assert_eq!(
        provider
            .list(&ItemId("/data/taken".into()))
            .await
            .unwrap_err(),
        VfsError::Conflict
    );
    assert_eq!(
        provider
            .list(&ItemId("/data/holding".into()))
            .await
            .unwrap_err(),
        VfsError::NotEmpty
    );
    assert_eq!(
        provider
            .list(&ItemId("/data/changed".into()))
            .await
            .unwrap_err(),
        VfsError::Conflict
    );
    assert_eq!(
        provider
            .list(&ItemId("/data/full".into()))
            .await
            .unwrap_err(),
        VfsError::NoSpace
    );
    assert_eq!(
        provider
            .list(&ItemId("/data/params".into()))
            .await
            .unwrap_err(),
        VfsError::NotSupported
    );
    assert_eq!(
        provider
            .list(&ItemId("/data/other".into()))
            .await
            .unwrap_err(),
        VfsError::Other("marlin-smb answered error code -1000".into())
    );
    assert_eq!(
        provider
            .list(&ItemId("/data/bad".into()))
            .await
            .unwrap_err(),
        VfsError::Other("marlin-smb sent an answer Marlin could not read".into())
    );

    // A missing helper says so before the sign-in is read: this server is not even saved.
    marlin_test_keychain::install();
    let dir = tempfile::tempdir().unwrap();
    let sign_in = SmbSignIn::new("missing-bin.example", dir.path().join("smb-servers.json"));
    let provider = SmbProvider::new(Arc::new(sign_in), None);
    assert_eq!(
        provider.list(&ItemId("/".into())).await.unwrap_err(),
        VfsError::Other(StartError::NotInstalled.to_string())
    );

    // So does a `marlin-smb` that cannot load Samba.
    let broken = fake("");
    write_script(
        &broken.binary,
        "echo 'Library not loaded: libsmbclient' >&2\nexit 1\n",
    );
    let sign_in = SmbSignIn::new("no-samba.example", dir.path().join("smb-servers.json"));
    let provider = SmbProvider::new(Arc::new(sign_in), Some(broken.binary.clone()));
    assert_eq!(
        provider.list(&ItemId("/".into())).await.unwrap_err(),
        VfsError::Other(StartError::LibraryMissing.to_string())
    );
}

#[tokio::test]
async fn a_refused_sign_in_is_never_sent_again() {
    let script = fake(&format!(
        "{old}{shares}",
        old = arm(
            r#"*'"password":"old"'*"#,
            &print_error(error_codes::SMB_AUTH_FAILED),
        ),
        shares = root_shares(&["data"]),
    ));
    let saved = save("refused.example", "old");
    let provider = open(&saved, &script);

    assert_eq!(
        provider.list(&ItemId("/".into())).await.unwrap_err(),
        VfsError::SignInNeeded
    );
    assert_eq!(requests(&script).len(), 1);
    assert_eq!(
        provider.list(&ItemId("/".into())).await.unwrap_err(),
        VfsError::SignInNeeded
    );
    assert_eq!(requests(&script).len(), 1);

    marlin_test_keychain::set("marlin-smb", "refused.example", "new");
    let listed = provider.list(&ItemId("/".into())).await.unwrap();
    assert_eq!(listed[0].name, "data");
    assert!(logged(&script).contains(r#""password":"new""#));
}

#[tokio::test]
async fn a_refusal_is_remembered_across_providers() {
    let script = fake(&arm(
        r#"*'"method":"list_shares"'*"#,
        &print_error(error_codes::SMB_AUTH_FAILED),
    ));
    let saved = save("remembered.example", "old");
    let first = open(&saved, &script);
    assert_eq!(
        first.list(&ItemId("/".into())).await.unwrap_err(),
        VfsError::SignInNeeded
    );
    drop(first);
    assert_eq!(requests(&script).len(), 1);

    let second = open(&saved, &script);
    assert_eq!(
        second.list(&ItemId("/".into())).await.unwrap_err(),
        VfsError::SignInNeeded
    );
    assert_eq!(requests(&script).len(), 1);
}

#[tokio::test]
async fn an_accepted_sign_in_that_the_server_refuses_later_is_never_sent_again() {
    let dir = tempfile::tempdir().unwrap();
    let changed = dir.path().join("changed");
    // Once `changed` exists the server refuses `old`, as after a password change on the server.
    let script = fake(&format!(
        "{old}{shares}",
        old = arm(
            r#"*'"password":"old"'*"#,
            &format!(
                "if [ -f {changed} ]; then\n        {refuse}\n      else\n        {list}\n      fi",
                changed = shell_quote(&changed),
                refuse = print_error(error_codes::SMB_AUTH_FAILED),
                list = print_result(&shares_json(&["data"])),
            ),
        ),
        shares = root_shares(&["data"]),
    ));
    let saved = save("refused-later.example", "old");
    let provider = open(&saved, &script);
    accepted(&provider).await;

    fs::write(&changed, "").unwrap();
    let before = requests(&script).len();
    assert_eq!(
        provider.list(&ItemId("/".into())).await.unwrap_err(),
        VfsError::SignInNeeded
    );
    assert_eq!(requests(&script).len(), before + 1);
    assert_eq!(
        provider.list(&ItemId("/".into())).await.unwrap_err(),
        VfsError::SignInNeeded
    );
    assert_eq!(requests(&script).len(), before + 1);

    marlin_test_keychain::set("marlin-smb", "refused-later.example", "new");
    provider.list(&ItemId("/".into())).await.unwrap();
    assert_eq!(
        requests(&script).last().unwrap()["params"]["credentials"]["password"],
        "new"
    );
}

#[tokio::test]
async fn a_switched_account_is_read_again_and_the_same_account_reads_no_keychain() {
    let script = fake(&root_shares(&["data"]));
    let saved = save("switched.example", "pw");
    let provider = open(&saved, &script);
    provider.list(&ItemId("/".into())).await.unwrap();

    // The same account: the kept password, with no keychain read.
    marlin_test_keychain::remove("marlin-smb", "switched.example");
    provider.list(&ItemId("/".into())).await.unwrap();
    let sent = requests(&script).last().unwrap()["params"]["credentials"].clone();
    assert_eq!(sent["username"], "ana");
    assert_eq!(sent["password"], "pw");

    // The user saved another account, and the old one still works on the server.
    // A user name with a backslash, as Windows writes `DOMAIN\user`, goes out as saved.
    add_server(
        &saved.path,
        "switched.example".into(),
        "CORP\\bo".into(),
        "pw2".into(),
        None,
    )
    .unwrap();
    provider.list(&ItemId("/".into())).await.unwrap();
    let sent = requests(&script).last().unwrap()["params"]["credentials"].clone();
    assert_eq!(sent["username"], "CORP\\bo");
    assert_eq!(sent["password"], "pw2");
}

#[tokio::test]
async fn a_switched_account_is_read_again_after_a_call_the_server_never_answered() {
    // The NAS was asleep for `ana`, so her sign-in was never tried.
    let script = fake(&format!(
        "{asleep}{shares}",
        asleep = arm(
            r#"*'"username":"ana"'*"#,
            &print_error(error_codes::SMB_CONNECTION_LOST),
        ),
        shares = root_shares(&["data"]),
    ));
    let saved = save("switched-asleep.example", "pw");
    assert_eq!(
        open(&saved, &script)
            .list(&ItemId("/".into()))
            .await
            .unwrap_err(),
        VfsError::Offline
    );

    add_server(
        &saved.path,
        "switched-asleep.example".into(),
        "bo".into(),
        "pw2".into(),
        None,
    )
    .unwrap();
    open(&saved, &script)
        .list(&ItemId("/".into()))
        .await
        .unwrap();
    let sent = requests(&script).last().unwrap()["params"]["credentials"].clone();
    assert_eq!(sent["username"], "bo");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_sign_in_goes_out_on_one_call_at_a_time() {
    let script = fake(&arm(
        r#"*'"password":"old"'*"#,
        &format!(
            "sleep 0.3\n      {}",
            print_error(error_codes::SMB_AUTH_FAILED)
        ),
    ));
    let saved = save("one-at-a-time.example", "old");
    let provider = Arc::new(open(&saved, &script));
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let provider = Arc::clone(&provider);
        tasks.push(tokio::spawn(async move {
            provider.list(&ItemId("/".into())).await.unwrap_err()
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap(), VfsError::SignInNeeded);
    }
    assert_eq!(requests(&script).len(), 1);
}

#[tokio::test]
async fn a_new_sign_in_reads_the_keychain_not_the_kept_copy() {
    let script = fake(&root_shares(&["data"]));
    let saved = save("keychain-not-cache.example", "old");
    marlin_test_keychain::set("marlin-smb", "keychain-not-cache.example", "new");
    let sign_in = Arc::new(SmbSignIn::new(
        "keychain-not-cache.example",
        saved.path.clone(),
    ));
    let provider = SmbProvider::new(sign_in, Some(script.binary.clone()));
    provider.list(&ItemId("/".into())).await.unwrap();
    assert!(logged(&script).contains(r#""password":"new""#));
    assert!(!logged(&script).contains(r#""password":"old""#));
}

#[tokio::test]
async fn a_new_user_name_is_a_new_sign_in() {
    let script = fake(&format!(
        "{refused}{shares}",
        refused = arm(
            r#"*'"username":"ana"'*"#,
            &print_error(error_codes::SMB_AUTH_FAILED),
        ),
        shares = root_shares(&["data"]),
    ));
    let saved = save("new-user.example", "same");
    let provider = open(&saved, &script);
    assert_eq!(
        provider.list(&ItemId("/".into())).await.unwrap_err(),
        VfsError::SignInNeeded
    );

    add_server(
        &saved.path,
        "new-user.example".into(),
        "bo".into(),
        "same".into(),
        None,
    )
    .unwrap();
    provider.list(&ItemId("/".into())).await.unwrap();
    assert!(logged(&script).contains(r#""username":"bo""#));
}

#[tokio::test]
async fn a_server_that_is_not_saved_needs_a_sign_in() {
    marlin_test_keychain::install();
    let dir = tempfile::tempdir().unwrap();
    let sign_in = Arc::new(SmbSignIn::new(
        "not-saved.example",
        dir.path().join("smb-servers.json"),
    ));
    let script = fake("");
    let provider = SmbProvider::new(sign_in, Some(script.binary.clone()));
    assert_eq!(
        provider.list(&ItemId("/".into())).await.unwrap_err(),
        VfsError::SignInNeeded
    );
    assert!(logged(&script).is_empty());
}

#[tokio::test]
async fn an_unreadable_servers_file_sends_nothing_and_names_nothing() {
    marlin_test_keychain::install();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("smb-servers.json");
    fs::write(&path, "not json").unwrap();
    let script = fake(&root_shares(&["data"]));
    let provider = SmbProvider::new(
        Arc::new(SmbSignIn::new("unreadable.example", path)),
        Some(script.binary.clone()),
    );
    assert_eq!(
        provider.list(&ItemId("/".into())).await.unwrap_err(),
        VfsError::Other("Marlin could not read the saved SMB sign-in".into())
    );
    assert!(logged(&script).is_empty());
}

#[tokio::test]
async fn space_uses_the_first_share_that_reports_a_size() {
    let script = fake(&format!(
        "{shares}{denied}{broken}{none}{data}{later}",
        shares = root_shares(&["denied", "broken", "nosize", "data", "later"]),
        denied = arm(
            r#"*'"share":"denied"'*"#,
            &print_error(error_codes::SMB_PERMISSION_DENIED),
        ),
        broken = arm(
            r#"*'"share":"broken"'*"#,
            &print_error(error_codes::SMB_ERROR),
        ),
        none = arm(r#"*'"share":"nosize"'*"#, &print_result("null")),
        data = arm(
            r#"*'"share":"data"'*"#,
            &print_result(r#"{"total":1000,"free":100}"#),
        ),
        later = arm(
            r#"*'"share":"later"'*"#,
            &print_result(r#"{"total":5000,"free":300}"#),
        ),
    ));
    let saved = save("space-size.example", "pw");
    let provider = open(&saved, &script);
    assert_eq!(
        provider.space().await.unwrap(),
        Space {
            total: 1000,
            free: 100
        }
    );
    assert!(!logged(&script).contains(r#""share":"later""#));

    let script = fake(&format!(
        "{shares}{none}",
        shares = root_shares(&["nosize"]),
        none = arm(r#"*'"share":"nosize"'*"#, &print_result("null")),
    ));
    let saved = save("space-none.example", "pw");
    let provider = open(&saved, &script);
    let pib = 1 << 50;
    assert_eq!(
        provider.space().await.unwrap(),
        Space {
            total: pib,
            free: pib
        }
    );

    // A share that answers with a refused sign-in or a lost server fails the call, and no later share is asked.
    for (code, error) in [
        (error_codes::SMB_AUTH_FAILED, VfsError::SignInNeeded),
        (error_codes::SMB_CONNECTION_LOST, VfsError::Offline),
    ] {
        let script = fake(&format!(
            "{shares}{first}{later}",
            shares = root_shares(&["first", "later"]),
            first = arm(r#"*'"share":"first"'*"#, &print_error(code)),
            later = arm(
                r#"*'"share":"later"'*"#,
                &print_result(r#"{"total":5000,"free":300}"#),
            ),
        ));
        let saved = save(&format!("space-fails{code}.example"), "pw");
        let provider = open(&saved, &script);
        assert_eq!(provider.space().await.unwrap_err(), error);
        assert!(!logged(&script).contains(r#""share":"later""#));
    }
}

fn local_file(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("upload");
    fs::write(&path, bytes).unwrap();
    (dir, path)
}

/// `get_file_metadata` answers not found, so a new file goes on to `save_file`.
fn missing_metadata() -> String {
    arm(
        r#"*'"method":"get_file_metadata"'*"#,
        &print_error(error_codes::SMB_PATH_NOT_FOUND),
    )
}

fn empty_directory() -> String {
    arm(
        r#"*'"method":"read_directory"'*"#,
        &print_result(r#"{"entries":[]}"#),
    )
}

async fn wait_for_requests(
    fake: &Fake,
    ready: impl Fn(&[serde_json::Value]) -> bool,
) -> Vec<serde_json::Value> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let sent = requests(fake);
        if ready(&sent) {
            return sent;
        }
        assert!(
            Instant::now() < deadline,
            "requests stayed:\n{}",
            logged(fake)
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn temporary_path(request: &serde_json::Value) -> String {
    request["params"]["temporary_path"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upload_does_not_hold_up_other_calls() {
    let script = fake(&format!(
        "{save}{meta}{dir}{shares}",
        save = arm(r#"*'"method":"save_file"'*"#, "sleep 600"),
        meta = missing_metadata(),
        dir = empty_directory(),
        shares = root_shares(&["data"]),
    ));
    let saved = save("uploads-aside.example", "pw");
    let provider = Arc::new(open(&saved, &script));
    accepted(&provider).await;
    let (_file_dir, file) = local_file(b"bytes");

    let mut uploads = Vec::new();
    for n in 0..UPLOAD_PROCESSES {
        let provider = Arc::clone(&provider);
        let file = file.clone();
        uploads.push(tokio::spawn(async move {
            let _ = provider
                .create_file(
                    &ItemId("/data".into()),
                    &format!("n{n}.txt"),
                    &file,
                    UNIX_EPOCH,
                    &|_| {},
                )
                .await;
        }));
    }
    wait_for_requests(&script, |sent| {
        sent.iter()
            .filter(|line| line["method"] == "save_file")
            .count()
            >= UPLOAD_PROCESSES
    })
    .await;
    let listed = provider.list(&ItemId("/data".into())).await.unwrap();
    assert!(listed.is_empty());
    assert!(uploads.iter().all(|upload| !upload.is_finished()));
    for upload in uploads {
        upload.abort();
    }
}

#[tokio::test]
async fn progress_reaches_the_core_and_a_stopped_upload_removes_its_temporary_file() {
    let notices = r#"printf '{"jsonrpc":"2.0","method":"staged","params":{"id":%s}}\n' "$id"
      printf '{"jsonrpc":"2.0","method":"progress","params":{"id":%s,"bytes":3}}\n' "$id"
      printf '{"jsonrpc":"2.0","method":"progress","params":{"id":%s,"bytes":8}}\n' "$id"
      sleep 600"#;
    let script = fake(&format!(
        "{save}{meta}{shares}",
        save = arm(r#"*'"method":"save_file"'*"#, notices),
        meta = missing_metadata(),
        shares = root_shares(&["data"]),
    ));
    let saved = save("progress.example", "pw");
    let provider = Arc::new(open(&saved, &script));
    accepted(&provider).await;
    let (_file_dir, file) = local_file(b"12345678");
    let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
    let upload = tokio::spawn({
        let provider = Arc::clone(&provider);
        let heard = Arc::clone(&heard);
        let file = file.clone();
        async move {
            let _ = provider
                .create_file(
                    &ItemId("/data".into()),
                    "note.txt",
                    &file,
                    UNIX_EPOCH,
                    &move |bytes| heard.lock().unwrap().push(bytes),
                )
                .await;
        }
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if heard.lock().unwrap().len() >= 2 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "progress: {:?}",
            heard.lock().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(*heard.lock().unwrap(), vec![3, 8]);
    let temporary = temporary_path(
        requests(&script)
            .iter()
            .find(|line| line["method"] == "save_file")
            .unwrap(),
    );
    upload.abort();
    wait_for_requests(&script, |sent| {
        sent.iter()
            .any(|line| line["method"] == "delete" && line["params"]["path"] == temporary.as_str())
    })
    .await;
}

#[tokio::test]
async fn a_failed_save_after_staged_removes_its_temporary_file() {
    let body = format!(
        "{staged}\n      {error}",
        staged = r#"printf '{"jsonrpc":"2.0","method":"staged","params":{"id":%s}}\n' "$id""#,
        error = print_error(error_codes::SMB_CHANGED),
    );
    let script = fake(&format!(
        "{save}{meta}{shares}",
        save = arm(r#"*'"method":"save_file"'*"#, &body),
        meta = missing_metadata(),
        shares = root_shares(&["data"]),
    ));
    let saved = save("staged-fails.example", "pw");
    let provider = open(&saved, &script);
    accepted(&provider).await;
    let (_file_dir, file) = local_file(b"abc");
    assert_eq!(
        provider
            .create_file(
                &ItemId("/data".into()),
                "note.txt",
                &file,
                UNIX_EPOCH,
                &|_| {},
            )
            .await
            .unwrap_err(),
        VfsError::Conflict
    );
    let sent = requests(&script);
    let save_at = sent
        .iter()
        .position(|line| line["method"] == "save_file")
        .unwrap();
    let temporary = temporary_path(&sent[save_at]);
    assert_eq!(sent[save_at + 1]["method"], "delete");
    assert_eq!(sent[save_at + 1]["params"]["path"], temporary);
}

#[tokio::test]
async fn a_removal_that_fails_is_tried_again_before_the_next_upload() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("deleted-once");
    let succeed = r#"printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id""#;
    let delete = format!(
        "if [ ! -f {marker} ]; then\n        touch {marker}\n        {error}\n      else\n        {succeed}\n      fi",
        marker = shell_quote(&marker),
        error = print_error(error_codes::SMB_CONNECTION_LOST),
        succeed = succeed,
    );
    let save_body = format!(
        "{staged}\n      {error}",
        staged = r#"printf '{"jsonrpc":"2.0","method":"staged","params":{"id":%s}}\n' "$id""#,
        error = print_error(error_codes::SMB_CHANGED),
    );
    let script = fake(&format!(
        "{save}{delete}{meta}{shares}",
        save = arm(r#"*'"method":"save_file"'*"#, &save_body),
        delete = arm(r#"*'"method":"delete"'*"#, &delete),
        meta = missing_metadata(),
        shares = root_shares(&["data"]),
    ));
    let saved = save("stray.example", "pw");
    let provider = open(&saved, &script);
    accepted(&provider).await;
    let (_file_dir, file) = local_file(b"abc");
    assert_eq!(
        provider
            .create_file(
                &ItemId("/data".into()),
                "one.txt",
                &file,
                UNIX_EPOCH,
                &|_| {},
            )
            .await
            .unwrap_err(),
        VfsError::Conflict
    );
    let first = temporary_path(
        requests(&script)
            .iter()
            .find(|line| line["method"] == "save_file")
            .unwrap(),
    );
    assert_eq!(
        provider
            .create_file(
                &ItemId("/data".into()),
                "two.txt",
                &file,
                UNIX_EPOCH,
                &|_| {},
            )
            .await
            .unwrap_err(),
        VfsError::Conflict
    );
    let sent = requests(&script);
    let saves: Vec<usize> = sent
        .iter()
        .enumerate()
        .filter(|(_, line)| line["method"] == "save_file")
        .map(|(index, _)| index)
        .collect();
    assert_eq!(saves.len(), 2);
    // The first delete failed. The second upload sends the same path again, before its own `save_file`.
    let retried = sent
        .iter()
        .take(saves[1])
        .skip(saves[0] + 1)
        .filter(|line| line["method"] == "delete" && line["params"]["path"] == first.as_str())
        .count();
    assert_eq!(retried, 2, "requests:\n{}", logged(&script));
}
