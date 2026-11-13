use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use marlin_gdrive::{AccessTokens, Config, DriveProvider};
use marlin_vfs::{ItemId, Kind, Options, Provider, Space, Vfs, VfsError, POSIX};
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const MODIFIED: &str = "2020-01-02T03:04:05.678Z";

fn modified() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_577_934_245) + Duration::from_millis(678)
}

struct TestTokens {
    current: Mutex<String>,
    issued: Mutex<Vec<String>>,
    rejected: Mutex<Vec<String>>,
    fail: Mutex<Option<VfsError>>,
    /// Holds each caller until this many have read the current token, so they all send the same one.
    together: Option<tokio::sync::Barrier>,
}

impl TestTokens {
    fn new() -> Self {
        Self {
            current: Mutex::new("token-1".to_string()),
            issued: Mutex::new(Vec::new()),
            rejected: Mutex::new(Vec::new()),
            fail: Mutex::new(None),
            together: None,
        }
    }
}

#[async_trait::async_trait]
impl AccessTokens for TestTokens {
    async fn access_token(&self) -> Result<String, VfsError> {
        if let Some(error) = self.fail.lock().unwrap().clone() {
            return Err(error);
        }
        let token = self.current.lock().unwrap().clone();
        self.issued.lock().unwrap().push(token.clone());
        if let Some(together) = &self.together {
            together.wait().await;
        }
        Ok(token)
    }

    fn rejected(&self, token: &str) {
        self.rejected.lock().unwrap().push(token.to_string());
        let mut current = self.current.lock().unwrap();
        if *current != token {
            return;
        }
        let number: u64 = current.trim_start_matches("token-").parse().unwrap();
        *current = format!("token-{}", number + 1);
    }
}

fn config(server: &MockServer) -> Config {
    Config {
        api: server.uri(),
        retry_delay: Duration::from_millis(1),
    }
}

fn drive(server: &MockServer) -> DriveProvider {
    DriveProvider::new(Arc::new(TestTokens::new()), config(server))
}

fn drive_with(server: &MockServer, tokens: Arc<TestTokens>) -> DriveProvider {
    DriveProvider::new(tokens, config(server))
}

fn file_json(id: &str, name: &str, mime: &str, size: &str) -> Value {
    json!({
        "id": id,
        "name": name,
        "mimeType": mime,
        "size": size,
        "modifiedTime": MODIFIED,
        "trashed": false,
    })
}

fn folder_json(id: &str, name: &str) -> Value {
    json!({
        "id": id,
        "name": name,
        "mimeType": "application/vnd.google-apps.folder",
        "modifiedTime": MODIFIED,
        "trashed": false,
    })
}

fn file_page(files: Vec<Value>, token: Option<&str>) -> Value {
    let mut value = json!({ "files": files });
    if let Some(token) = token {
        value["nextPageToken"] = json!(token);
    }
    value
}

fn plain(id: &str, name: &str) -> Value {
    file_json(id, name, "text/plain", "4")
}

fn ok_json(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

struct Script {
    hits: AtomicUsize,
    responses: Vec<ResponseTemplate>,
}

impl Respond for Script {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let hit = self.hits.fetch_add(1, Ordering::SeqCst);
        self.responses
            .get(hit)
            .cloned()
            .unwrap_or_else(|| self.responses.last().unwrap().clone())
    }
}

async fn mount_script(server: &MockServer, responses: Vec<ResponseTemplate>) {
    Mock::given(method("GET"))
        .respond_with(Script {
            hits: AtomicUsize::new(0),
            responses,
        })
        .mount(server)
        .await;
}

async fn mount_path(server: &MockServer, path_str: &str, template: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(path_str))
        .respond_with(template)
        .mount(server)
        .await;
}

fn query_of(request: &Request) -> HashMap<String, String> {
    request
        .url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

fn sent_header<'a>(request: &'a Request, name: &str) -> &'a str {
    request
        .headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
}

#[tokio::test]
async fn the_top_lists_my_drive_and_shared_drives() {
    let server = MockServer::start().await;
    mount_path(
        &server,
        "/files/root",
        ok_json(folder_json("alias-root", "Mon Drive")),
    )
    .await;
    let drive = drive(&server);
    let items = drive.list(&drive.root()).await.unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "My Drive");
    assert_eq!(items[0].id, ItemId("root".to_string()));
    assert_eq!(items[0].attributes.kind, Kind::Folder);
    assert_eq!(items[1].name, "Shared drives");
    assert_eq!(items[1].id, ItemId("/shared-drives".to_string()));
    assert_eq!(items[1].attributes.kind, Kind::Folder);
    assert_eq!(items[1].attributes.size, 0);
    assert_eq!(items[1].attributes.modified, UNIX_EPOCH);
}

#[tokio::test]
async fn a_folder_listing_follows_three_pages_in_order() {
    let server = MockServer::start().await;
    mount_script(
        &server,
        vec![
            ok_json(file_page(vec![plain("a", "a.txt")], Some("page-2"))),
            ok_json(file_page(vec![plain("b", "b.txt")], Some("page-3"))),
            ok_json(file_page(vec![plain("c", "c.txt")], None)),
        ],
    )
    .await;
    let drive = drive(&server);
    let items = drive.list(&ItemId("folder".to_string())).await.unwrap();
    let names: Vec<_> = items.iter().map(|item| item.name.as_str()).collect();
    assert_eq!(names, ["a.txt", "b.txt", "c.txt"]);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(query_of(&requests[0]).get("pageToken"), None);
    assert_eq!(query_of(&requests[1]).get("pageToken").unwrap(), "page-2");
    assert_eq!(query_of(&requests[2]).get("pageToken").unwrap(), "page-3");
}

#[tokio::test]
async fn a_listing_asks_only_for_children_that_are_not_trashed() {
    let server = MockServer::start().await;
    mount_script(
        &server,
        vec![ok_json(file_page(vec![plain("a", "a.txt")], None))],
    )
    .await;
    let drive = drive(&server);
    drive.list(&ItemId("folder-1".to_string())).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        query_of(&requests[0]).get("q").unwrap(),
        "'folder-1' in parents and trashed = false"
    );
}

#[tokio::test]
async fn google_docs_sheets_slides_and_shortcuts_are_left_out_of_a_listing() {
    let server = MockServer::start().await;
    let files = vec![
        file_json("pdf", "notes.pdf", "application/pdf", "10"),
        file_json("doc", "Notes", "application/vnd.google-apps.document", "0"),
        file_json(
            "sheet",
            "Numbers",
            "application/vnd.google-apps.spreadsheet",
            "0",
        ),
        file_json(
            "slides",
            "Deck",
            "application/vnd.google-apps.presentation",
            "0",
        ),
        file_json("link", "Link", "application/vnd.google-apps.shortcut", "0"),
        folder_json("sub", "Sub"),
    ];
    mount_script(&server, vec![ok_json(file_page(files, None))]).await;
    let drive = drive(&server);
    let items = drive.list(&ItemId("folder".to_string())).await.unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "notes.pdf");
    assert_eq!(items[0].attributes.kind, Kind::File);
    assert_eq!(items[1].name, "Sub");
    assert_eq!(items[1].attributes.kind, Kind::Folder);
    assert_eq!(items[1].attributes.size, 0);
}

#[tokio::test]
async fn a_file_keeps_its_size_and_modified_time_and_a_folder_has_size_zero() {
    let server = MockServer::start().await;
    mount_path(
        &server,
        "/files/file-1",
        ok_json(file_json("other", "notes.txt", "text/plain", "42")),
    )
    .await;
    mount_path(
        &server,
        "/files/folder-1",
        ok_json({
            let mut folder = folder_json("other", "Docs");
            folder["size"] = json!("999");
            folder
        }),
    )
    .await;
    let drive = drive(&server);
    let file = drive.item(&ItemId("file-1".to_string())).await.unwrap();
    assert_eq!(file.id, ItemId("file-1".to_string()));
    assert_eq!(file.attributes.kind, Kind::File);
    assert_eq!(file.attributes.size, 42);
    assert_eq!(file.attributes.modified, modified());
    let folder = drive.item(&ItemId("folder-1".to_string())).await.unwrap();
    assert_eq!(folder.attributes.kind, Kind::Folder);
    assert_eq!(folder.attributes.size, 0);
    assert_eq!(folder.attributes.modified, modified());
}

#[tokio::test]
async fn duplicate_names_and_a_slash_come_back_unchanged_and_the_core_makes_them_usable() {
    let server = MockServer::start().await;
    mount_path(
        &server,
        "/files/root",
        ok_json(folder_json("alias-root", "My Drive")),
    )
    .await;
    mount_path(
        &server,
        "/files",
        ok_json(file_page(
            vec![
                plain("file-1", "a.txt"),
                plain("file-2", "a.txt"),
                file_json("file-3", "a/b.txt", "text/plain", "1"),
            ],
            None,
        )),
    )
    .await;
    let drive = Arc::new(drive(&server));
    let listed = drive.list(&ItemId("root".to_string())).await.unwrap();
    let names: Vec<_> = listed.iter().map(|item| item.name.as_str()).collect();
    assert_eq!(names, ["a.txt", "a.txt", "a/b.txt"]);
    let vfs = Vfs::new(drive, POSIX, Options::default());
    let my_drive = vfs.lookup(Vfs::ROOT, "My Drive").await.unwrap();
    assert!(vfs.lookup(my_drive.node, "a.txt").await.is_ok());
    assert!(vfs.lookup(my_drive.node, "a (2).txt").await.is_ok());
    assert!(vfs.lookup(my_drive.node, "a%2Fb.txt").await.is_ok());
}

#[tokio::test]
async fn a_rename_seen_on_the_remote_keeps_the_node() {
    let server = MockServer::start().await;
    mount_path(
        &server,
        "/files/root",
        ok_json(folder_json("alias-root", "My Drive")),
    )
    .await;
    Mock::given(path("/files"))
        .respond_with(ok_json(file_page(vec![plain("file-1", "old.txt")], None)))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_path(
        &server,
        "/files",
        ok_json(file_page(vec![plain("file-1", "new.txt")], None)),
    )
    .await;
    let drive = Arc::new(drive(&server));
    let options = Options {
        listing_ttl: Duration::ZERO,
        ..Options::default()
    };
    let vfs = Vfs::new(drive, POSIX, options);
    let my_drive = vfs.lookup(Vfs::ROOT, "My Drive").await.unwrap();
    let old = vfs.lookup(my_drive.node, "old.txt").await.unwrap();
    let new = vfs.lookup(my_drive.node, "new.txt").await.unwrap();
    assert_eq!(old.node, new.node);
}

#[tokio::test]
async fn read_sends_a_byte_range_and_returns_that_body() {
    let server = MockServer::start().await;
    mount_path(
        &server,
        "/files/file-1",
        ResponseTemplate::new(206).set_body_bytes(b"abcdefghij".to_vec()),
    )
    .await;
    let drive = drive(&server);
    let bytes = drive
        .read(&ItemId("file-1".to_string()), 10, 10)
        .await
        .unwrap();
    assert_eq!(bytes, b"abcdefghij");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(sent_header(&requests[0], "range"), "bytes=10-19");
}

#[tokio::test]
async fn read_at_the_end_of_a_file_returns_no_bytes() {
    let server = MockServer::start().await;
    mount_path(&server, "/files/file-1", ResponseTemplate::new(416)).await;
    let drive = drive(&server);
    let bytes = drive
        .read(&ItemId("file-1".to_string()), 100, 10)
        .await
        .unwrap();
    assert!(bytes.is_empty());
}

#[tokio::test]
async fn read_of_a_folder_that_exists_only_in_the_mount_is_a_folder() {
    let server = MockServer::start().await;
    let drive = drive(&server);
    for id in ["/", "/shared-drives"] {
        assert_eq!(
            drive.read(&ItemId(id.to_string()), 0, 10).await,
            Err(VfsError::IsAFolder)
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn read_of_length_zero_sends_no_request() {
    let server = MockServer::start().await;
    let drive = drive(&server);
    let bytes = drive
        .read(&ItemId("file-1".to_string()), 10, 0)
        .await
        .unwrap();
    assert!(bytes.is_empty());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn read_of_a_whole_response_returns_only_the_asked_range() {
    let server = MockServer::start().await;
    mount_path(
        &server,
        "/files/file-1",
        ResponseTemplate::new(200).set_body_bytes(b"0123456789".to_vec()),
    )
    .await;
    let drive = drive(&server);
    let bytes = drive
        .read(&ItemId("file-1".to_string()), 2, 4)
        .await
        .unwrap();
    assert_eq!(bytes, b"2345");
}

#[tokio::test]
async fn item_of_a_trashed_file_is_not_found() {
    let server = MockServer::start().await;
    let mut body = plain("file-1", "gone.txt");
    body["trashed"] = json!(true);
    mount_path(&server, "/files/file-1", ok_json(body)).await;
    let drive = drive(&server);
    assert_eq!(
        drive.item(&ItemId("file-1".to_string())).await,
        Err(VfsError::NotFound)
    );
}

#[tokio::test]
async fn item_of_an_unknown_id_is_not_found() {
    let server = MockServer::start().await;
    mount_path(&server, "/files/missing", ResponseTemplate::new(404)).await;
    let drive = drive(&server);
    assert_eq!(
        drive.item(&ItemId("missing".to_string())).await,
        Err(VfsError::NotFound)
    );
}

#[tokio::test]
async fn item_of_a_google_doc_is_not_supported() {
    let server = MockServer::start().await;
    mount_path(
        &server,
        "/files/doc-1",
        ok_json(file_json(
            "doc-1",
            "Notes",
            "application/vnd.google-apps.document",
            "0",
        )),
    )
    .await;
    let drive = drive(&server);
    assert_eq!(
        drive.item(&ItemId("doc-1".to_string())).await,
        Err(VfsError::NotSupported)
    );
}

#[tokio::test]
async fn listing_an_empty_folder_returns_no_items() {
    let server = MockServer::start().await;
    mount_path(&server, "/files", ok_json(file_page(vec![], None))).await;
    mount_path(
        &server,
        "/files/folder-1",
        ok_json(folder_json("folder-1", "Empty")),
    )
    .await;
    let drive = drive(&server);
    let items = drive.list(&ItemId("folder-1".to_string())).await.unwrap();
    assert!(items.is_empty());
}

#[tokio::test]
async fn listing_a_file_is_not_a_folder() {
    let server = MockServer::start().await;
    mount_path(&server, "/files", ok_json(file_page(vec![], None))).await;
    mount_path(&server, "/files/file-1", ok_json(plain("file-1", "a.txt"))).await;
    let drive = drive(&server);
    assert_eq!(
        drive.list(&ItemId("file-1".to_string())).await,
        Err(VfsError::NotAFolder)
    );
}

#[tokio::test]
async fn listing_an_id_that_is_gone_is_not_found() {
    let server = MockServer::start().await;
    mount_path(&server, "/files", ok_json(file_page(vec![], None))).await;
    mount_path(&server, "/files/gone", ResponseTemplate::new(404)).await;
    let drive = drive(&server);
    assert_eq!(
        drive.list(&ItemId("gone".to_string())).await,
        Err(VfsError::NotFound)
    );
}

#[tokio::test]
async fn one_rejected_token_is_replaced_and_the_call_succeeds() {
    let server = MockServer::start().await;
    let tokens = Arc::new(TestTokens::new());
    mount_script(
        &server,
        vec![
            ResponseTemplate::new(401),
            ok_json(folder_json("root", "My Drive")),
        ],
    )
    .await;
    let drive = drive_with(&server, tokens.clone());
    assert!(drive.item(&ItemId("root".to_string())).await.is_ok());
    assert_eq!(tokens.rejected.lock().unwrap().as_slice(), ["token-1"]);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(sent_header(&requests[0], "authorization"), "Bearer token-1");
    assert_eq!(sent_header(&requests[1], "authorization"), "Bearer token-2");
}

#[tokio::test]
async fn two_rejections_mean_sign_in_is_needed() {
    let server = MockServer::start().await;
    mount_script(&server, vec![ResponseTemplate::new(401)]).await;
    let drive = drive(&server);
    assert_eq!(
        drive.item(&ItemId("root".to_string())).await,
        Err(VfsError::SignInNeeded)
    );
}

#[tokio::test]
async fn a_token_source_that_needs_sign_in_sends_nothing() {
    let server = MockServer::start().await;
    let tokens = Arc::new(TestTokens::new());
    *tokens.fail.lock().unwrap() = Some(VfsError::SignInNeeded);
    let drive = drive_with(&server, tokens);
    assert_eq!(
        drive.item(&ItemId("root".to_string())).await,
        Err(VfsError::SignInNeeded)
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_user_rate_limit_is_retried_until_the_call_succeeds() {
    let server = MockServer::start().await;
    let limited = ResponseTemplate::new(403).set_body_json(json!({
        "error": { "errors": [{ "reason": "userRateLimitExceeded" }] }
    }));
    mount_script(
        &server,
        vec![
            limited.clone(),
            limited,
            ok_json(folder_json("root", "My Drive")),
        ],
    )
    .await;
    let drive = drive(&server);
    assert!(drive.item(&ItemId("root".to_string())).await.is_ok());
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn a_forbidden_file_is_access_denied() {
    let server = MockServer::start().await;
    mount_script(
        &server,
        vec![ResponseTemplate::new(403).set_body_json(json!({
            "error": { "errors": [{ "reason": "insufficientFilePermissions" }] }
        }))],
    )
    .await;
    let drive = drive(&server);
    assert_eq!(
        drive.item(&ItemId("file-1".to_string())).await,
        Err(VfsError::AccessDenied)
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_forbidden_answer_with_no_reason_is_access_denied() {
    let server = MockServer::start().await;
    mount_script(&server, vec![ResponseTemplate::new(403)]).await;
    let drive = drive(&server);
    assert_eq!(
        drive.item(&ItemId("file-1".to_string())).await,
        Err(VfsError::AccessDenied)
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_status_the_table_does_not_name_fails_without_a_retry() {
    let server = MockServer::start().await;
    mount_script(&server, vec![ResponseTemplate::new(400)]).await;
    let drive = drive(&server);
    assert_eq!(
        drive.item(&ItemId("file-1".to_string())).await,
        Err(VfsError::Other(
            "Google Drive answered 400 Bad Request".into()
        ))
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_server_error_on_every_attempt_gives_up() {
    let server = MockServer::start().await;
    mount_script(&server, vec![ResponseTemplate::new(503)]).await;
    let drive = drive(&server);
    assert_eq!(
        drive.item(&ItemId("root".to_string())).await,
        Err(VfsError::Other(
            "Google Drive answered 503 Service Unavailable".into()
        ))
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 4);
}

#[tokio::test]
async fn too_many_requests_on_every_attempt_gives_up() {
    let server = MockServer::start().await;
    mount_script(&server, vec![ResponseTemplate::new(429)]).await;
    let drive = drive(&server);
    assert_eq!(
        drive.item(&ItemId("root".to_string())).await,
        Err(VfsError::Other("Google Drive is limiting requests".into()))
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 4);
}

#[tokio::test]
async fn an_answer_that_is_not_the_expected_json_is_a_failure_of_the_remote() {
    let server = MockServer::start().await;
    mount_path(
        &server,
        "/files/file-1",
        ResponseTemplate::new(200).set_body_string("<html>"),
    )
    .await;
    let drive = drive(&server);
    assert_eq!(
        drive.item(&ItemId("file-1".to_string())).await,
        Err(VfsError::Other(
            "Google Drive sent an answer Marlin could not read".into()
        ))
    );
}

#[tokio::test]
async fn a_server_that_is_not_listening_is_offline() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let drive = DriveProvider::new(
        Arc::new(TestTokens::new()),
        Config {
            api: format!("http://{address}"),
            retry_delay: Duration::from_millis(1),
        },
    );
    assert_eq!(
        drive.item(&ItemId("root".to_string())).await,
        Err(VfsError::Offline)
    );
}

#[tokio::test]
async fn space_with_a_limit_reports_what_is_left() {
    let server = MockServer::start().await;
    mount_path(
        &server,
        "/about",
        ok_json(json!({ "storageQuota": { "limit": "1000", "usage": "400" } })),
    )
    .await;
    let drive = drive(&server);
    assert_eq!(
        drive.space().await.unwrap(),
        Space {
            total: 1000,
            free: 600,
        }
    );
}

#[tokio::test]
async fn space_without_a_limit_still_reports_free_room() {
    let server = MockServer::start().await;
    mount_path(
        &server,
        "/about",
        ok_json(json!({ "storageQuota": { "usage": "100" } })),
    )
    .await;
    let drive = drive(&server);
    let free = 1u64 << 50;
    assert_eq!(
        drive.space().await.unwrap(),
        Space {
            total: 100 + free,
            free,
        }
    );
}

#[tokio::test]
async fn shared_drives_follow_two_pages_and_each_id_is_the_drive_twice() {
    let server = MockServer::start().await;
    mount_script(
        &server,
        vec![
            ok_json(json!({
                "nextPageToken": "page-2",
                "drives": [{ "id": "team-1", "name": "Team", "createdTime": MODIFIED }],
            })),
            ok_json(json!({
                "drives": [{ "id": "team-2", "name": "Other", "createdTime": MODIFIED }],
            })),
        ],
    )
    .await;
    let drive = drive(&server);
    let items = drive
        .list(&ItemId("/shared-drives".to_string()))
        .await
        .unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, ItemId("team-1/team-1".to_string()));
    assert_eq!(items[0].name, "Team");
    assert_eq!(items[0].attributes.kind, Kind::Folder);
    assert_eq!(items[0].attributes.size, 0);
    assert_eq!(items[0].attributes.modified, modified());
    assert_eq!(items[1].id, ItemId("team-2/team-2".to_string()));
    assert_eq!(items[1].name, "Other");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(query_of(&requests[0]).get("pageToken"), None);
    assert_eq!(query_of(&requests[1]).get("pageToken").unwrap(), "page-2");
}

#[tokio::test]
async fn a_shared_drive_listing_names_the_drive_and_a_my_drive_listing_does_not() {
    let server = MockServer::start().await;
    for (parent, child) in [
        ("drive-1", "child-1"),
        ("folder-2", "child-2"),
        ("folder-3", "child-3"),
    ] {
        Mock::given(query_param(
            "q",
            format!("'{parent}' in parents and trashed = false"),
        ))
        .respond_with(ok_json(file_page(vec![plain(child, "a.txt")], None)))
        .mount(&server)
        .await;
    }
    let drive = drive(&server);
    let root = drive
        .list(&ItemId("drive-1/drive-1".to_string()))
        .await
        .unwrap();
    let nested = drive
        .list(&ItemId("drive-1/folder-2".to_string()))
        .await
        .unwrap();
    let mine = drive.list(&ItemId("folder-3".to_string())).await.unwrap();
    assert_eq!(root[0].id, ItemId("drive-1/child-1".to_string()));
    assert_eq!(nested[0].id, ItemId("drive-1/child-2".to_string()));
    assert_eq!(mine[0].id, ItemId("child-3".to_string()));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    for request in &requests[..2] {
        let query = query_of(request);
        assert_eq!(query.get("corpora").unwrap(), "drive");
        assert_eq!(query.get("driveId").unwrap(), "drive-1");
    }
    let mine_query = query_of(&requests[2]);
    assert_eq!(mine_query.get("corpora"), None);
    assert_eq!(mine_query.get("driveId"), None);
}

#[tokio::test]
async fn eight_calls_share_one_token_refresh() {
    let server = MockServer::start().await;
    let tokens = Arc::new(TestTokens {
        together: Some(tokio::sync::Barrier::new(8)),
        ..TestTokens::new()
    });
    Mock::given(header("authorization", "Bearer token-1"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    mount_path(
        &server,
        "/files/root",
        ok_json(folder_json("root", "My Drive")),
    )
    .await;
    let drive = Arc::new(drive_with(&server, tokens.clone()));
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let drive = Arc::clone(&drive);
        tasks.push(tokio::spawn(async move {
            drive.item(&ItemId("root".to_string())).await
        }));
    }
    for task in tasks {
        assert!(task.await.unwrap().is_ok());
    }
    let issued = tokens.issued.lock().unwrap().clone();
    assert!(issued.iter().any(|token| token == "token-1"));
    assert!(issued.iter().any(|token| token == "token-2"));
    assert!(issued
        .iter()
        .all(|token| token == "token-1" || token == "token-2"));
    let requests = server.received_requests().await.unwrap();
    let mut first = 0;
    let mut second = 0;
    for request in &requests {
        match sent_header(request, "authorization") {
            "Bearer token-1" => first += 1,
            "Bearer token-2" => second += 1,
            other => panic!("unexpected token {other}"),
        }
    }
    assert_eq!((first, second), (8, 8));
}

#[tokio::test]
async fn every_request_is_a_get() {
    let server = MockServer::start().await;
    mount_path(
        &server,
        "/about",
        ok_json(json!({ "storageQuota": { "limit": "10", "usage": "1" } })),
    )
    .await;
    mount_path(
        &server,
        "/files",
        ok_json(file_page(vec![plain("child", "a.txt")], None)),
    )
    .await;
    Mock::given(path("/files/file-1"))
        .and(query_param("alt", "media"))
        .respond_with(ResponseTemplate::new(206).set_body_bytes(b"hello".to_vec()))
        .mount(&server)
        .await;
    mount_path(&server, "/files/file-1", ok_json(plain("file-1", "a.txt"))).await;
    let drive = drive(&server);
    drive.list(&ItemId("folder".to_string())).await.unwrap();
    drive.item(&ItemId("file-1".to_string())).await.unwrap();
    drive
        .read(&ItemId("file-1".to_string()), 0, 5)
        .await
        .unwrap();
    drive.space().await.unwrap();
    let requests = server.received_requests().await.unwrap();
    assert!(requests.len() >= 4);
    assert!(requests
        .iter()
        .all(|request| request.method.as_str() == "GET"));
}
