//! Manual check of the two byte calls on one real Google account.
//!
//! It CHANGES the account named by `MARLIN_GDRIVE_LIVE_ACCOUNT`. It makes one folder
//! `Marlin live upload test {unix seconds}` at the top of My Drive, uploads a 100 KiB text file,
//! an empty file, and a 200 MiB file, replaces the text file twice and the 200 MiB file once,
//! and trashes them at the end. It also lists the text file's revisions, downloads the first,
//! and marks that revision `keepForever` (decision 0005, risk 4), and opens one more upload
//! session to ask it about an empty file. It never touches a shared drive.
//!
//! It prints the `{unix seconds}` first. If it fails midway, trash the folder whose name
//! holds that number. The trashed 200 MiB files count against the account until they leave
//! the trash: to get the room back at once, delete forever only the items of this run.
//!
//! It prints counts, sizes, statuses, times, and the text file's type only, and never an ID or a name,
//! also when an assertion fails.
//!
//! Run:
//! `MARLIN_GDRIVE_LIVE_ACCOUNT=<email> cargo test -p marlin-gdrive --test live_uploads -- --ignored --nocapture`

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use marlin_gdrive::{AccessTokens, Config, DriveProvider, OAuthClient, SavedSignIn};
use marlin_vfs::{ItemId, Provider, Revision, VfsError};

const PIECE: u64 = 256 * 1024;
const BIG: u64 = 200 * 1024 * 1024;

#[tokio::test]
#[ignore = "changes a real Google account and reads the real keychain: see the header"]
async fn a_real_account_uploads_a_file_and_replaces_its_bytes() {
    let email = std::env::var("MARLIN_GDRIVE_LIVE_ACCOUNT")
        .expect("Set MARLIN_GDRIVE_LIVE_ACCOUNT to the account's email");
    // The loader's own text can hold a file path, so it stays out of the output.
    let client = OAuthClient::load().unwrap_or_else(|_| panic!("no Google OAuth client"));
    let sign_in = Arc::new(SavedSignIn::new(&email, client));
    let tokens: Arc<dyn AccessTokens> = sign_in.clone();
    let provider = DriveProvider::new(tokens, Config::default());
    let Config { api, upload, .. } = Config::default();
    let http = reqwest::Client::new();
    let root = ItemId("root".to_string());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let quiet = |_: u64| {};

    println!("test folder holds {stamp}");

    let scratch = provider
        .create_folder(&root, &format!("Marlin live upload test {stamp}"))
        .await
        .unwrap_or_else(|_| panic!("the test folder was not made"));

    let modified =
        SystemTime::from(chrono::DateTime::parse_from_rfc3339("2021-02-03T04:05:06.789Z").unwrap());
    let first = pattern(100 * 1024, 1);
    let small = temp_file(&first);
    let made = provider
        .create_file(&scratch.id, "notes.txt", small.path(), modified, &quiet)
        .await
        .unwrap_or_else(|_| panic!("the 100 KiB file was not made"));
    let token = sign_in
        .access_token()
        .await
        .unwrap_or_else(|_| panic!("no access token"));
    // A file in My Drive: its item id is its Drive id.
    let file_id = made.id.0.as_str();
    let made_as = mime_type(&http, &api, &token, file_id).await;
    println!("text file stored as {made_as}");
    assert_eq!(
        made.attributes.size,
        first.len() as u64,
        "the size is wrong"
    );
    assert!(
        made.attributes.revision.is_some(),
        "the new file has no revision"
    );
    assert_eq!(
        made.attributes.modified, modified,
        "the modified time is not the one sent"
    );
    let listed = provider
        .list(&scratch.id)
        .await
        .unwrap_or_else(|_| panic!("the test folder did not list"));
    assert!(
        listed
            .iter()
            .any(|item| item.attributes.revision == made.attributes.revision),
        "the listing does not hold the new file's revision"
    );
    println!(
        "100 KiB file: size {}, has a revision",
        made.attributes.size
    );

    let empty_file = temp_file(&[]);
    let empty = provider
        .create_file(
            &scratch.id,
            "empty.txt",
            empty_file.path(),
            SystemTime::now(),
            &quiet,
        )
        .await
        .unwrap_or_else(|_| panic!("the empty file was not made"));
    assert_eq!(empty.attributes.size, 0, "the empty file has a size");
    assert!(
        empty.attributes.revision.is_some(),
        "the empty file has no revision"
    );
    println!("empty file: size 0, has a revision");

    // Bytes that start like a PNG image, so a replace that let Drive guess the type again would show.
    let mut second = b"\x89PNG\r\n\x1a\n".to_vec();
    second.extend(pattern(150 * 1024 - 8, 2));
    let replaced_at =
        SystemTime::from(chrono::DateTime::parse_from_rfc3339("2022-03-04T05:06:07.890Z").unwrap());
    let replaced_file = temp_file(&second);
    let base = made.attributes.revision.clone().unwrap();
    let started = Instant::now();
    let replaced = provider
        .replace_bytes(&made.id, replaced_file.path(), &base, replaced_at, &quiet)
        .await
        .unwrap_or_else(|_| panic!("the 100 KiB file was not replaced"));
    println!("replace 150 KiB: {} ms", started.elapsed().as_millis());
    assert!(replaced.id == made.id, "the id changed");
    assert!(
        replaced.attributes.revision.is_some(),
        "the replaced file has no revision"
    );
    assert!(
        replaced.attributes.revision != made.attributes.revision,
        "the revision did not change"
    );
    let got = provider
        .read(&made.id, 0, second.len() as u32)
        .await
        .unwrap_or_else(|_| panic!("the replaced file could not be read"));
    assert!(got == second, "the file is not the new bytes");
    let read_back = provider
        .item(&made.id)
        .await
        .unwrap_or_else(|_| panic!("the replaced file could not be asked for"));
    assert_eq!(
        read_back.attributes.modified, replaced_at,
        "the modified time after a replace is not the one sent"
    );
    let replaced_as = mime_type(&http, &api, &token, file_id).await;
    println!("after a replace, stored as {replaced_as}");

    let first_id = base.0;
    let revisions: serde_json::Value = http
        .get(format!("{api}/files/{file_id}/revisions"))
        .bearer_auth(&token)
        .query(&[("fields", "revisions(id,keepForever)")])
        .send()
        .await
        .unwrap_or_else(|_| panic!("the revision list failed"))
        .json()
        .await
        .unwrap_or_else(|_| panic!("the revision list could not be read"));
    let still = revisions["revisions"]
        .as_array()
        .map(|rows| rows.iter().any(|row| row["id"].as_str() == Some(&first_id)))
        .unwrap_or(false);
    println!("first revision still listed: {still}");

    // The provider marks no revision `keepForever`, so the overwritten bytes must download without it.
    let (before_status, before_matches) =
        revision_bytes(&http, &api, &token, file_id, &first_id, &first).await;
    println!(
        "revision download: {} {}",
        before_status,
        if before_matches { "matches" } else { "differs" }
    );

    let kept = http
        .patch(format!("{api}/files/{file_id}/revisions/{first_id}"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "keepForever": true }))
        .send()
        .await
        .unwrap_or_else(|_| panic!("the revision could not be kept"))
        .status();
    println!("keep revision: {}", kept.as_u16());
    let (status, kept_matches) =
        revision_bytes(&http, &api, &token, file_id, &first_id, &first).await;
    println!(
        "kept revision download: {} {}",
        status,
        if kept_matches { "matches" } else { "differs" }
    );
    // Asserted after the cleanup, so a failed check still trashes the files.
    let overwritten_bytes_kept = still
        && before_status == 200
        && before_matches
        && kept.is_success()
        && status == 200
        && kept_matches;

    // The first bytes, so a write that landed and still answered `Conflict` would show.
    let stale = provider
        .replace_bytes(
            &made.id,
            small.path(),
            &Revision(first_id),
            SystemTime::now(),
            &quiet,
        )
        .await;
    assert!(
        matches!(stale, Err(VfsError::Conflict)),
        "a stale revision was not a conflict"
    );
    let still_new = provider
        .read(&made.id, 0, second.len() as u32)
        .await
        .unwrap_or_else(|_| panic!("the file could not be read after the conflict"));
    assert!(still_new == second, "the conflict changed the file");
    println!("stale revision: conflict, bytes unchanged");

    // A save that cuts a file to nothing.
    let base = replaced.attributes.revision.clone().unwrap();
    let emptied = provider
        .replace_bytes(
            &made.id,
            empty_file.path(),
            &base,
            SystemTime::now(),
            &quiet,
        )
        .await
        .unwrap_or_else(|_| panic!("the text file was not emptied"));
    assert_eq!(emptied.attributes.size, 0, "the emptied file has a size");
    assert!(
        emptied.attributes.revision.is_some()
            && emptied.attributes.revision != replaced.attributes.revision,
        "emptying the file made no new revision"
    );
    println!("replace with an empty file: size 0, a new revision");

    // How a session answers the question for an empty file. The provider never asks it, because a 200 here ends the file with no revision check: this checks that Google answers so.
    let opened = http
        .post(format!("{upload}/files"))
        .bearer_auth(&token)
        .query(&[("uploadType", "resumable")])
        .json(&serde_json::json!({ "name": "probe.txt", "parents": [scratch.id.0] }))
        .send()
        .await
        .unwrap_or_else(|_| panic!("the probe session did not open"));
    let session = opened
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|location| location.to_str().ok())
        .unwrap_or_else(|| panic!("the probe session has no address"))
        .to_string();
    let asked = http
        .put(session)
        .bearer_auth(&token)
        .header(reqwest::header::CONTENT_LENGTH, 0)
        .header(reqwest::header::CONTENT_RANGE, "bytes */0")
        .send()
        .await
        .unwrap_or_else(|_| panic!("the probe question failed"));
    let probe_status = asked.status();
    println!(
        "question for an empty file: {}, Range {}",
        probe_status.as_u16(),
        asked.headers().contains_key(reqwest::header::RANGE)
    );
    // A 200 or 201 means the question ended the empty file, and the probe made one.
    let mut probe_left = false;
    if probe_status.is_success() {
        let landed: serde_json::Value = asked
            .json()
            .await
            .unwrap_or_else(|_| panic!("the probe answer could not be read"));
        let probe = ItemId(landed["id"].as_str().unwrap_or("").to_string());
        probe_left = provider.remove(&probe, &scratch.id).await.is_err();
    }

    let dir = tempfile::tempdir().unwrap_or_else(|_| panic!("no temp dir"));
    let big = write_big(dir.path(), "big.bin", 3);
    let (big_item, growth) = with_rss(async {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let progress = {
            let seen = Arc::clone(&seen);
            move |n: u64| seen.lock().unwrap().push((Instant::now(), n))
        };
        let started = Instant::now();
        let item = provider
            .create_file(&scratch.id, "big.bin", &big, SystemTime::now(), &progress)
            .await
            .unwrap_or_else(|_| panic!("the 200 MiB file was not made"));
        let done = Instant::now();
        report("new 200 MiB file", started, done, &seen.lock().unwrap());
        item
    })
    .await;
    println!("new 200 MiB file peak rss growth: {}", kib(growth));
    assert_eq!(
        big_item.attributes.size, BIG,
        "the returned size is not 200 MiB"
    );

    let other = write_big(dir.path(), "other.bin", 9);
    let base = big_item.attributes.revision.clone().unwrap();
    let (replaced_big, growth) = with_rss(async {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let progress = {
            let seen = Arc::clone(&seen);
            move |n: u64| seen.lock().unwrap().push((Instant::now(), n))
        };
        let started = Instant::now();
        let item = provider
            .replace_bytes(&big_item.id, &other, &base, SystemTime::now(), &progress)
            .await
            .unwrap_or_else(|_| panic!("the 200 MiB file was not replaced"));
        let done = Instant::now();
        report("replace 200 MiB file", started, done, &seen.lock().unwrap());
        item
    })
    .await;
    println!("replace 200 MiB file peak rss growth: {}", kib(growth));
    assert_eq!(
        replaced_big.attributes.size, BIG,
        "the returned size is not 200 MiB"
    );
    assert!(replaced_big.id == big_item.id, "the id changed");
    // Three samples of the new bytes: the start, across the first 16 MiB boundary, and the end.
    let mut same = true;
    for at in [0, 16 * 1024 * 1024 - 512 * 1024, BIG - 1024 * 1024] {
        let got = provider
            .read(&big_item.id, at, 1024 * 1024)
            .await
            .unwrap_or_else(|_| panic!("the replaced 200 MiB file could not be read"));
        same &= got == big_bytes(at, 1024 * 1024, 9);
    }
    println!("replaced 200 MiB file: bytes match in 3 samples: {same}");
    assert!(same, "the replaced 200 MiB file holds other bytes");

    let mut failed = usize::from(probe_left);
    for id in [&made.id, &empty.id, &big_item.id] {
        if provider.remove(id, &scratch.id).await.is_err() {
            failed += 1;
        }
    }
    if provider.remove(&scratch.id, &root).await.is_err() {
        failed += 1;
    }
    println!("trash failures: {failed}");
    assert_eq!(failed, 0, "something was left behind");
    assert_eq!(
        probe_status,
        reqwest::StatusCode::OK,
        "the question did not end the empty file"
    );
    assert_eq!(
        made_as, "text/plain",
        "Drive did not take the .txt file for text"
    );
    assert_eq!(made_as, replaced_as, "a replace changed the file's type");
    assert!(
        overwritten_bytes_kept,
        "Drive did not keep the overwritten bytes as a revision that can be read back"
    );
}

/// Letters, so Drive takes the `.txt` file for text.
fn pattern(len: usize, salt: u8) -> Vec<u8> {
    (0..len)
        .map(|i| b'a' + ((i + usize::from(salt)) % 26) as u8)
        .collect()
}

async fn mime_type(http: &reqwest::Client, api: &str, token: &str, file_id: &str) -> String {
    let file: serde_json::Value = http
        .get(format!("{api}/files/{file_id}"))
        .bearer_auth(token)
        .query(&[("fields", "mimeType")])
        .send()
        .await
        .unwrap_or_else(|_| panic!("the file type could not be read"))
        .error_for_status()
        .unwrap_or_else(|_| panic!("Google refused to give the file type"))
        .json()
        .await
        .unwrap_or_else(|_| panic!("the file type answer could not be read"));
    file["mimeType"]
        .as_str()
        .filter(|mime| !mime.is_empty())
        .unwrap_or_else(|| panic!("the answer has no file type"))
        .to_string()
}

fn kib(growth: Option<u64>) -> String {
    growth.map_or("not measured".to_string(), |kib| format!("{kib} KiB"))
}

fn temp_file(bytes: &[u8]) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().unwrap_or_else(|_| panic!("no temp file"));
    file.write_all(bytes)
        .unwrap_or_else(|_| panic!("the temp file could not be written"));
    file
}

/// Writes `BIG` bytes in 1 MiB pieces. The whole file is never in memory.
fn write_big(dir: &std::path::Path, name: &str, salt: u8) -> PathBuf {
    let path = dir.join(name);
    let mut file =
        std::fs::File::create(&path).unwrap_or_else(|_| panic!("the big file was not created"));
    let piece = 1024 * 1024;
    for at in (0..BIG).step_by(piece) {
        file.write_all(&big_bytes(at, piece, salt))
            .unwrap_or_else(|_| panic!("the big file could not be written"));
    }
    path
}

/// `len` bytes of a big file from `at`. They repeat every 251 bytes, so no two 256 KiB pieces are alike, and a piece sent at the wrong offset shows.
fn big_bytes(at: u64, len: usize, salt: u8) -> Vec<u8> {
    (at..at + len as u64)
        .map(|n| ((n % 251) as u8).wrapping_add(salt))
        .collect()
}

async fn revision_bytes(
    http: &reqwest::Client,
    api: &str,
    token: &str,
    file_id: &str,
    revision: &str,
    first: &[u8],
) -> (u16, bool) {
    let response = http
        .get(format!("{api}/files/{file_id}/revisions/{revision}"))
        .bearer_auth(token)
        .query(&[("alt", "media")])
        .send()
        .await
        .unwrap_or_else(|_| panic!("the revision download failed"));
    let status = response.status().as_u16();
    let bytes = response
        .bytes()
        .await
        .unwrap_or_else(|_| panic!("the revision body could not be read"));
    (status, bytes.as_ref() == first)
}

/// Prints the time, the speed, and the time from the first progress report at or past the start of the last 256 KiB to the answer. That time holds the wait for the answer to the request before, and for a replace the revision check and the last piece: it bounds the window of decision 0005's risk 4 from above.
fn report(label: &str, started: Instant, done: Instant, seen: &[(Instant, u64)]) {
    let took = done.saturating_duration_since(started);
    let secs = took.as_secs_f64().max(0.001);
    let mib_s = (BIG as f64) / (1024.0 * 1024.0) / secs;
    let mark = (BIG - 1) / PIECE * PIECE;
    let window = seen
        .iter()
        .find(|(_, n)| *n >= mark)
        .map(|(at, _)| done.saturating_duration_since(*at).as_millis());
    match window {
        Some(ms) => println!(
            "{label}: {} ms, {mib_s:.1} MiB/s, from the count reaching the last 256 KiB to the answer {ms} ms",
            took.as_millis()
        ),
        None => println!(
            "{label}: {} ms, {mib_s:.1} MiB/s, the count never reached the last 256 KiB",
            took.as_millis()
        ),
    }
    assert!(window.is_some(), "progress never reached the last 256 KiB");
}

/// Samples this process's RSS every 200 ms while `work` runs. The number is the peak growth, in KiB, or `None` when `ps` gave no baseline or no sample.
async fn with_rss<T>(work: impl std::future::Future<Output = T>) -> (T, Option<u64>) {
    let before = rss_kib();
    let stop = Arc::new(AtomicBool::new(false));
    let peak = Arc::new(AtomicU64::new(0));
    let handle = {
        let stop = Arc::clone(&stop);
        let peak = Arc::clone(&peak);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Some(now) = rss_kib() {
                    peak.fetch_max(now, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        })
    };
    let value = work.await;
    stop.store(true, Ordering::Relaxed);
    handle.join().ok();
    let peak = peak.load(Ordering::Relaxed);
    let growth = before
        .filter(|_| peak > 0)
        .map(|before| peak.saturating_sub(before));
    (value, growth)
}

fn rss_kib() -> Option<u64> {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    String::from_utf8(out.stdout).ok()?.trim().parse().ok()
}
