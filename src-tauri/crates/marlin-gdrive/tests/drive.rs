use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use marlin_gdrive::{AccessTokens, Config, DriveProvider};
use marlin_vfs::{
    Access, IfExists, Item, ItemId, Kind, Options, Provider, Space, Vfs, VfsError, POSIX,
};
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const MODIFIED: &str = "2020-01-02T03:04:05.678Z";

const ITEM_FIELDS: &str =
    "id,name,mimeType,size,modifiedTime,trashed,driveId,shortcutDetails(targetId)";

const FOLDER: &str = "application/vnd.google-apps.folder";
const DOC: &str = "application/vnd.google-apps.document";
const PLAIN: &str = "text/plain";

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
        "mimeType": FOLDER,
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

fn body_of(request: &Request) -> Value {
    serde_json::from_slice(&request.body).unwrap()
}

fn held_json(id: &str, name: &str, mime: &str, parents: &[&str], drive_id: Option<&str>) -> Value {
    let mut file = if mime == FOLDER {
        folder_json(id, name)
    } else {
        file_json(id, name, mime, "4")
    };
    file["parents"] = json!(parents);
    if let Some(drive_id) = drive_id {
        file["driveId"] = json!(drive_id);
    }
    file
}

async fn mount_method(
    server: &MockServer,
    http_method: &str,
    path_str: &str,
    template: ResponseTemplate,
) {
    Mock::given(method(http_method))
        .and(path(path_str))
        .respond_with(template)
        .mount(server)
        .await;
}

async fn mount_answers(
    server: &MockServer,
    http_method: &str,
    path_str: &str,
    responses: Vec<ResponseTemplate>,
) {
    Mock::given(method(http_method))
        .and(path(path_str))
        .respond_with(Script {
            hits: AtomicUsize::new(0),
            responses,
        })
        .mount(server)
        .await;
}

async fn mount_held(
    server: &MockServer,
    id: &str,
    name: &str,
    mime: &str,
    parents: &[&str],
    drive_id: Option<&str>,
) {
    mount_path(
        server,
        &format!("/files/{id}"),
        ok_json(held_json(id, name, mime, parents, drive_id)),
    )
    .await;
}

fn id(value: &str) -> ItemId {
    ItemId(value.to_string())
}

async fn sent(server: &MockServer, http_method: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.method.as_str() == http_method)
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
async fn google_docs_sheets_slides_and_shortcuts_are_shown_as_link_files() {
    let server = MockServer::start().await;
    let mut shortcut = file_json("link", "Link", "application/vnd.google-apps.shortcut", "0");
    shortcut["shortcutDetails"] = json!({"targetId": "target"});
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
        shortcut,
        folder_json("sub", "Sub"),
    ];
    mount_script(&server, vec![ok_json(file_page(files, None))]).await;
    let drive = drive(&server);
    let items = drive.list(&ItemId("folder".to_string())).await.unwrap();
    let names: Vec<_> = items.iter().map(|item| item.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "notes.pdf",
            "Notes.document.html",
            "Numbers.spreadsheet.html",
            "Deck.presentation.html",
            "Link.shortcut.html",
            "Sub",
        ]
    );
    let ids: Vec<_> = items.iter().map(|item| item.id.0.as_str()).collect();
    assert_eq!(
        ids,
        [
            "pdf",
            "doc#doc",
            "sheet#sheet",
            "slides#slides",
            "link#target",
            "sub",
        ]
    );
    for item in &items[1..5] {
        assert_eq!(item.attributes.kind, Kind::File);
        assert_eq!(item.attributes.modified, modified());
        let page = drive.read(&item.id, 0, 4096).await.unwrap();
        assert_eq!(item.attributes.size, page.len() as u64);
    }
    let requests = server.received_requests().await.unwrap();
    assert!(query_of(&requests[0])
        .get("fields")
        .unwrap()
        .contains("shortcutDetails(targetId)"));
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
async fn item_of_a_google_doc_is_a_link_file_with_the_size_of_its_page() {
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
    let id = ItemId("doc-1#doc-1".to_string());
    let item = drive.item(&id).await.unwrap();
    assert_eq!(item.id, id);
    assert_eq!(item.name, "Notes.document.html");
    assert_eq!(item.attributes.kind, Kind::File);
    let bytes = drive.read(&id, 0, 4096).await.unwrap();
    assert_eq!(item.attributes.size, bytes.len() as u64);
}

#[tokio::test]
async fn item_of_a_shortcut_asks_google_for_its_target() {
    let server = MockServer::start().await;
    let mut shortcut = file_json("link", "Link", "application/vnd.google-apps.shortcut", "0");
    shortcut["shortcutDetails"] = json!({"targetId": "target"});
    mount_path(&server, "/files/link", ok_json(shortcut)).await;
    let drive = drive(&server);
    let id = ItemId("link#target".to_string());
    let item = drive.item(&id).await.unwrap();
    assert_eq!(item.id, id);
    assert_eq!(item.name, "Link.shortcut.html");
    let requests = server.received_requests().await.unwrap();
    assert!(query_of(&requests[0])
        .get("fields")
        .unwrap()
        .contains("shortcutDetails(targetId)"));
}

#[tokio::test]
async fn item_of_an_id_googles_answer_does_not_build_is_not_found() {
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
    mount_path(
        &server,
        "/files/file-1",
        ok_json(file_json("file-1", "notes.txt", "text/plain", "4")),
    )
    .await;
    let mut shortcut = file_json("link", "Link", "application/vnd.google-apps.shortcut", "0");
    shortcut["shortcutDetails"] = json!({"targetId": "other"});
    mount_path(&server, "/files/link", ok_json(shortcut)).await;
    let drive = drive(&server);
    assert_eq!(
        drive.item(&ItemId("doc-1".to_string())).await,
        Err(VfsError::NotFound)
    );
    assert_eq!(
        drive.item(&ItemId("file-1#file-1".to_string())).await,
        Err(VfsError::NotFound)
    );
    assert_eq!(
        drive.item(&ItemId("link#target".to_string())).await,
        Err(VfsError::NotFound)
    );
}

#[tokio::test]
async fn reading_a_link_file_returns_the_page_and_sends_google_nothing() {
    let server = MockServer::start().await;
    let drive = drive(&server);
    let bytes = drive
        .read(&ItemId("sc#tgt".to_string()), 0, 4096)
        .await
        .unwrap();
    let expected = "\
<!doctype html>
<meta charset=\"utf-8\">
<meta http-equiv=\"refresh\" content=\"0; url=https://drive.google.com/open?id=tgt\">
<title>Open in Google Drive</title>
<a href=\"https://drive.google.com/open?id=tgt\">Open in Google Drive</a>
";
    assert_eq!(bytes, expected.as_bytes());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_read_of_part_of_a_link_file_returns_that_part() {
    let server = MockServer::start().await;
    let drive = drive(&server);
    let id = ItemId("doc#doc".to_string());
    let whole = drive.read(&id, 0, 4096).await.unwrap();
    assert_eq!(drive.read(&id, 10, 5).await.unwrap(), whole[10..15]);
    let short = drive.read(&id, whole.len() as u64 - 3, 10).await.unwrap();
    assert_eq!(short, whole[whole.len() - 3..]);
    assert!(drive
        .read(&id, whole.len() as u64, 10)
        .await
        .unwrap()
        .is_empty());
    assert!(drive.read(&id, u64::MAX, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_link_file_in_a_shared_drive_carries_the_drive_in_its_id() {
    let server = MockServer::start().await;
    let doc = file_json("doc", "Notes", "application/vnd.google-apps.document", "0");
    mount_path(
        &server,
        "/files",
        ok_json(file_page(vec![doc.clone()], None)),
    )
    .await;
    mount_path(&server, "/files/doc", ok_json(doc)).await;
    let drive = drive(&server);
    let items = drive
        .list(&ItemId("drive-1/folder-1".to_string()))
        .await
        .unwrap();
    assert_eq!(items[0].id, ItemId("drive-1/doc#doc".to_string()));
    let item = drive
        .item(&ItemId("drive-1/doc#doc".to_string()))
        .await
        .unwrap();
    assert_eq!(item.id, ItemId("drive-1/doc#doc".to_string()));
    assert_eq!(item.name, "Notes.document.html");
    let requests = server.received_requests().await.unwrap();
    assert!(requests
        .iter()
        .any(|request| request.url.path() == "/files/doc"));
}

#[tokio::test]
async fn a_shortcut_whose_target_is_gone_still_lists() {
    let server = MockServer::start().await;
    let mut shortcut = file_json("link", "Link", "application/vnd.google-apps.shortcut", "0");
    shortcut["shortcutDetails"] = json!({"targetId": "gone"});
    mount_script(
        &server,
        vec![ok_json(file_page(
            vec![shortcut, plain("file-1", "a.txt")],
            None,
        ))],
    )
    .await;
    let drive = drive(&server);
    let items = drive.list(&ItemId("folder".to_string())).await.unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, ItemId("link#gone".to_string()));
    assert_eq!(items[1].id, ItemId("file-1".to_string()));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_shortcut_with_no_target_in_the_answer_opens_itself() {
    let server = MockServer::start().await;
    let bare = file_json("bare", "Bare", "application/vnd.google-apps.shortcut", "0");
    let mut empty = file_json(
        "empty",
        "Empty",
        "application/vnd.google-apps.shortcut",
        "0",
    );
    empty["shortcutDetails"] = json!({});
    mount_script(&server, vec![ok_json(file_page(vec![bare, empty], None))]).await;
    let drive = drive(&server);
    let items = drive.list(&ItemId("folder".to_string())).await.unwrap();
    assert_eq!(items[0].id, ItemId("bare#bare".to_string()));
    assert_eq!(items[1].id, ItemId("empty#empty".to_string()));
}

#[tokio::test]
async fn a_third_party_shortcut_is_named_drive_sdk() {
    let server = MockServer::start().await;
    let file = file_json(
        "app",
        "App file",
        "application/vnd.google-apps.drive-sdk.12345",
        "0",
    );
    mount_script(&server, vec![ok_json(file_page(vec![file], None))]).await;
    let drive = drive(&server);
    let items = drive.list(&ItemId("folder".to_string())).await.unwrap();
    assert_eq!(items[0].name, "App file.drive-sdk.html");
}

#[tokio::test]
async fn listing_a_link_file_is_not_a_folder_and_sends_no_request() {
    let server = MockServer::start().await;
    let drive = drive(&server);
    assert_eq!(
        drive.list(&ItemId("doc#doc".to_string())).await,
        Err(VfsError::NotAFolder)
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_google_doc_a_sheet_and_a_file_with_the_docs_name_get_distinct_names_and_the_doc_reads_as_its_page(
) {
    let server = MockServer::start().await;
    mount_path(
        &server,
        "/files/root",
        ok_json(folder_json("alias-root", "My Drive")),
    )
    .await;
    let doc = file_json("doc", "Notes", "application/vnd.google-apps.document", "0");
    mount_path(&server, "/files/doc", ok_json(doc.clone())).await;
    mount_path(
        &server,
        "/files",
        ok_json(file_page(
            vec![
                doc,
                file_json(
                    "sheet",
                    "Notes",
                    "application/vnd.google-apps.spreadsheet",
                    "0",
                ),
                file_json("file", "Notes.document.html", "text/plain", "4"),
            ],
            None,
        )),
    )
    .await;
    let drive = Arc::new(drive(&server));
    let vfs = Vfs::new(drive, POSIX, Options::default());
    let my_drive = vfs.lookup(Vfs::ROOT, "My Drive").await.unwrap();
    let folder = vfs.open_folder(my_drive.node).unwrap();
    vfs.rewind_folder(folder).await.unwrap();
    let names: Vec<_> = vfs
        .read_folder(folder, 0, 10)
        .unwrap()
        .iter()
        .map(|entry| entry.name.clone())
        .collect();
    assert_eq!(
        names,
        [
            "Notes.document.html".to_string(),
            "Notes.spreadsheet.html".to_string(),
            "Notes.document (2).html".to_string(),
        ]
    );
    let doc = vfs
        .lookup(my_drive.node, "Notes.document.html")
        .await
        .unwrap();
    let opened = vfs.open(doc.node, Access::Read).await.unwrap();
    let bytes = vfs.read(opened, 0, 4096).await.unwrap();
    let page = "\
<!doctype html>
<meta charset=\"utf-8\">
<meta http-equiv=\"refresh\" content=\"0; url=https://drive.google.com/open?id=doc\">
<title>Open in Google Drive</title>
<a href=\"https://drive.google.com/open?id=doc\">Open in Google Drive</a>
";
    assert_eq!(bytes, page.as_bytes());
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
async fn a_shared_drives_top_folder_is_asked_from_the_drive_and_matches_its_listing() {
    let server = MockServer::start().await;
    // On a real account, `files.get` gives the top folder of a shared drive another name than the drive's. The provider must not ask it.
    Mock::given(path("/files/team-1"))
        .respond_with(ok_json(folder_json("team-1", "Drive")))
        .mount(&server)
        .await;
    Mock::given(path("/drives/team-1"))
        .respond_with(ok_json(
            json!({ "id": "team-1", "name": "Team", "createdTime": MODIFIED }),
        ))
        .mount(&server)
        .await;
    Mock::given(path("/drives"))
        .respond_with(ok_json(json!({
            "drives": [{ "id": "team-1", "name": "Team", "createdTime": MODIFIED }],
        })))
        .mount(&server)
        .await;
    let drive = drive(&server);
    let listed = drive
        .list(&ItemId("/shared-drives".to_string()))
        .await
        .unwrap();
    let found = drive.item(&listed[0].id).await.unwrap();
    assert_eq!(found, listed[0]);
    assert_eq!(found.name, "Team");
}

#[tokio::test]
async fn an_empty_shared_drive_lists_nothing_and_one_that_is_gone_is_not_found() {
    let server = MockServer::start().await;
    Mock::given(path("/files"))
        .respond_with(ok_json(file_page(vec![], None)))
        .mount(&server)
        .await;
    Mock::given(path("/drives/drive-1"))
        .respond_with(ok_json(
            json!({ "id": "drive-1", "name": "Team", "createdTime": MODIFIED }),
        ))
        .mount(&server)
        .await;
    Mock::given(path("/drives/gone-1"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let drive = drive(&server);
    let empty = drive.list(&ItemId("drive-1/drive-1".to_string())).await;
    assert_eq!(empty, Ok(vec![]));
    let gone = drive.list(&ItemId("gone-1/gone-1".to_string())).await;
    assert_eq!(gone, Err(VfsError::NotFound));
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
async fn the_five_read_calls_send_only_get() {
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
        ok_json(file_page(
            vec![
                plain("child", "a.txt"),
                file_json("doc", "Notes", "application/vnd.google-apps.document", "0"),
            ],
            None,
        )),
    )
    .await;
    Mock::given(path("/files/file-1"))
        .and(query_param("alt", "media"))
        .respond_with(ResponseTemplate::new(206).set_body_bytes(b"hello".to_vec()))
        .mount(&server)
        .await;
    mount_path(&server, "/files/file-1", ok_json(plain("file-1", "a.txt"))).await;
    let drive = drive(&server);
    let items = drive.list(&ItemId("folder".to_string())).await.unwrap();
    let link = items.iter().find(|item| item.id.0 == "doc#doc").unwrap();
    drive.read(&link.id, 0, 4096).await.unwrap();
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

#[tokio::test]
async fn create_folder_in_my_drive_sends_the_generated_id() {
    let server = MockServer::start().await;
    let body = ok_json(json!({ "ids": ["new-1"] }));
    mount_path(&server, "/files/generateIds", body).await;
    let made = ok_json(folder_json("new-1", "Plans"));
    mount_method(&server, "POST", "/files", made).await;
    let provider = drive(&server);
    let made = provider.create_folder(&id("root"), "Plans").await.unwrap();
    assert_eq!(made.id, id("new-1"));
    assert_eq!(made.name, "Plans");
    assert_eq!(made.attributes.kind, Kind::Folder);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method.as_str(), "GET");
    assert_eq!(requests[0].url.path(), "/files/generateIds");
    let query = query_of(&requests[0]);
    assert_eq!(query.get("count").unwrap(), "1");
    assert_eq!(query.get("space").unwrap(), "drive");
    assert_eq!(query.get("type").unwrap(), "files");
    assert_eq!(requests[1].method.as_str(), "POST");
    assert_eq!(requests[1].url.path(), "/files");
    let query = query_of(&requests[1]);
    assert_eq!(query.get("supportsAllDrives").unwrap(), "true");
    assert_eq!(query.get("fields").unwrap(), ITEM_FIELDS);
    assert_eq!(
        body_of(&requests[1]),
        json!({ "id": "new-1", "name": "Plans", "mimeType": FOLDER, "parents": ["root"] })
    );
}

#[tokio::test]
async fn create_folder_in_a_shared_drive_uses_that_drives_id_as_the_parent() {
    let server = MockServer::start().await;
    let answers = vec![
        ok_json(json!({ "ids": ["new-a"] })),
        ok_json(json!({ "ids": ["new-b"] })),
    ];
    mount_answers(&server, "GET", "/files/generateIds", answers).await;
    let mut top = folder_json("new-a", "Top");
    top["driveId"] = json!("d");
    let mut nested = folder_json("new-b", "Nested");
    nested["driveId"] = json!("d");
    let answers = vec![ok_json(top), ok_json(nested)];
    mount_answers(&server, "POST", "/files", answers).await;
    let provider = drive(&server);
    let at_top = provider.create_folder(&id("d/d"), "Top").await.unwrap();
    let inside = provider.create_folder(&id("d/f"), "Nested").await.unwrap();
    assert_eq!(at_top.id, id("d/new-a"));
    assert_eq!(inside.id, id("d/new-b"));
    assert_eq!(at_top.attributes.kind, Kind::Folder);
    let posts = sent(&server, "POST").await;
    assert_eq!(body_of(&posts[0])["parents"], json!(["d"]));
    assert_eq!(body_of(&posts[1])["parents"], json!(["f"]));
}

#[tokio::test]
async fn a_create_answered_500_then_409_returns_the_folder_the_first_try_made() {
    let server = MockServer::start().await;
    let body = ok_json(json!({ "ids": ["made-1"] }));
    mount_path(&server, "/files/generateIds", body).await;
    let answers = vec![ResponseTemplate::new(500), ResponseTemplate::new(409)];
    mount_answers(&server, "POST", "/files", answers).await;
    let body = ok_json(folder_json("made-1", "Plans"));
    mount_path(&server, "/files/made-1", body).await;
    let provider = drive(&server);
    let made = provider.create_folder(&id("root"), "Plans").await.unwrap();
    assert_eq!(made.id, id("made-1"));
    assert_eq!(made.attributes.kind, Kind::Folder);
    let posts = sent(&server, "POST").await;
    assert_eq!(posts.len(), 2);
    assert_eq!(body_of(&posts[0])["id"], "made-1");
    assert_eq!(body_of(&posts[1])["id"], "made-1");
    assert!(sent(&server, "GET")
        .await
        .iter()
        .any(|request| request.url.path() == "/files/made-1"));
}

#[tokio::test]
async fn a_rate_limit_on_a_change_is_sent_again() {
    let server = MockServer::start().await;
    mount_held(&server, "file-1", "a.txt", PLAIN, &["folder"], None).await;
    let answers = vec![ResponseTemplate::new(429), ResponseTemplate::new(200)];
    mount_answers(&server, "PATCH", "/files/file-1", answers).await;
    let provider = drive(&server);
    provider.remove(&id("file-1"), &id("folder")).await.unwrap();
    let patches = sent(&server, "PATCH").await;
    assert_eq!(patches.len(), 2);
    assert_eq!(body_of(&patches[0]), json!({ "trashed": true }));
    assert_eq!(body_of(&patches[1]), json!({ "trashed": true }));
}

#[tokio::test]
async fn remove_trashes_a_file_and_never_deletes_it() {
    let server = MockServer::start().await;
    mount_held(&server, "file-1", "a.txt", PLAIN, &["folder"], None).await;
    let status = ResponseTemplate::new(200);
    mount_method(&server, "PATCH", "/files/file-1", status).await;
    let provider = drive(&server);
    provider.remove(&id("file-1"), &id("folder")).await.unwrap();
    assert!(sent(&server, "DELETE").await.is_empty());
    let patches = sent(&server, "PATCH").await;
    let patch = &patches[0];
    assert_eq!(patch.url.path(), "/files/file-1");
    assert_eq!(body_of(patch), json!({ "trashed": true }));
    let query = query_of(patch);
    assert_eq!(query.get("supportsAllDrives").unwrap(), "true");
    assert_eq!(query.get("fields").unwrap(), "id");
    let gets = sent(&server, "GET").await;
    let fields = query_of(&gets[0]).get("fields").unwrap().clone();
    assert!(fields.contains("parents"));
    assert!(fields.contains("driveId"));
}

#[tokio::test]
async fn remove_of_a_folder_refuses_one_that_has_a_child_and_trashes_an_empty_one() {
    let with_child = MockServer::start().await;
    mount_held(&with_child, "folder-1", "Docs", FOLDER, &["parent"], None).await;
    let body = ok_json(file_page(vec![plain("child", "a.txt")], None));
    mount_path(&with_child, "/files", body).await;
    let provider = drive(&with_child);
    assert_eq!(
        provider.remove(&id("folder-1"), &id("parent")).await,
        Err(VfsError::NotEmpty)
    );
    assert!(sent(&with_child, "PATCH").await.is_empty());

    let paged = MockServer::start().await;
    mount_held(&paged, "folder-1", "Docs", FOLDER, &["parent"], None).await;
    let answers = vec![
        ok_json(json!({ "files": [], "nextPageToken": "next" })),
        ok_json(json!({ "files": [{ "id": "child" }] })),
    ];
    mount_answers(&paged, "GET", "/files", answers).await;
    let provider = drive(&paged);
    assert_eq!(
        provider.remove(&id("folder-1"), &id("parent")).await,
        Err(VfsError::NotEmpty)
    );
    assert!(sent(&paged, "PATCH").await.is_empty());
    let requests = paged.received_requests().await.unwrap();
    let lists: Vec<_> = requests
        .iter()
        .filter(|request| request.url.path() == "/files")
        .collect();
    assert_eq!(lists.len(), 2);
    let first = query_of(lists[0]);
    assert_eq!(first.get("pageSize").unwrap(), "1");
    assert_eq!(
        first.get("q").unwrap(),
        "'folder-1' in parents and trashed = false"
    );
    assert_eq!(first.get("pageToken"), None);
    assert_eq!(query_of(lists[1]).get("pageToken").unwrap(), "next");
    assert_eq!(first.get("corpora"), None);

    let empty = MockServer::start().await;
    mount_held(&empty, "folder-1", "Docs", FOLDER, &["parent"], None).await;
    mount_path(&empty, "/files", ok_json(file_page(vec![], None))).await;
    let status = ResponseTemplate::new(200);
    mount_method(&empty, "PATCH", "/files/folder-1", status).await;
    let provider = drive(&empty);
    provider
        .remove(&id("folder-1"), &id("parent"))
        .await
        .unwrap();
    let patches = sent(&empty, "PATCH").await;
    assert_eq!(patches.len(), 1);
    assert_eq!(body_of(&patches[0]), json!({ "trashed": true }));
}

#[tokio::test]
async fn remove_in_a_shared_drive_names_the_drive_when_it_checks_for_children() {
    let empty = MockServer::start().await;
    mount_held(&empty, "folder-1", "Docs", FOLDER, &["d"], Some("d")).await;
    mount_path(&empty, "/files", ok_json(file_page(vec![], None))).await;
    mount_method(
        &empty,
        "PATCH",
        "/files/folder-1",
        ResponseTemplate::new(200),
    )
    .await;
    let provider = drive(&empty);
    provider
        .remove(&id("d/folder-1"), &id("d/d"))
        .await
        .unwrap();
    let lists: Vec<_> = sent(&empty, "GET")
        .await
        .into_iter()
        .filter(|request| request.url.path() == "/files")
        .collect();
    let query = query_of(&lists[0]);
    assert_eq!(query.get("corpora").unwrap(), "drive");
    assert_eq!(query.get("driveId").unwrap(), "d");
    assert_eq!(query.get("includeItemsFromAllDrives").unwrap(), "true");
    assert_eq!(
        query.get("q").unwrap(),
        "'folder-1' in parents and trashed = false"
    );
    let patches = sent(&empty, "PATCH").await;
    assert_eq!(patches.len(), 1);
    assert_eq!(body_of(&patches[0]), json!({ "trashed": true }));
    assert_eq!(
        query_of(&patches[0]).get("supportsAllDrives").unwrap(),
        "true"
    );

    let with_child = MockServer::start().await;
    mount_held(&with_child, "folder-1", "Docs", FOLDER, &["d"], Some("d")).await;
    let body = ok_json(file_page(vec![plain("child", "a.txt")], None));
    mount_path(&with_child, "/files", body).await;
    let provider = drive(&with_child);
    assert_eq!(
        provider.remove(&id("d/folder-1"), &id("d/d")).await,
        Err(VfsError::NotEmpty)
    );
    assert!(sent(&with_child, "PATCH").await.is_empty());
}

#[tokio::test]
async fn remove_of_an_item_that_is_not_in_the_folder_is_not_found() {
    let elsewhere = MockServer::start().await;
    mount_held(&elsewhere, "file-1", "a.txt", PLAIN, &["other"], None).await;
    let provider = drive(&elsewhere);
    assert_eq!(
        provider.remove(&id("file-1"), &id("folder")).await,
        Err(VfsError::NotFound)
    );
    assert!(sent(&elsewhere, "PATCH").await.is_empty());

    let trashed = MockServer::start().await;
    let mut file = held_json("file-1", "a.txt", PLAIN, &["folder"], None);
    file["trashed"] = json!(true);
    mount_path(&trashed, "/files/file-1", ok_json(file)).await;
    let provider = drive(&trashed);
    assert_eq!(
        provider.remove(&id("file-1"), &id("folder")).await,
        Err(VfsError::NotFound)
    );
    assert!(trashed.received_requests().await.unwrap().len() == 1);

    for drive_id in [Some("other"), None] {
        let server = MockServer::start().await;
        mount_held(&server, "f", "a.txt", PLAIN, &["folder"], drive_id).await;
        let provider = drive(&server);
        assert_eq!(
            provider.remove(&id("d/f"), &id("d/folder")).await,
            Err(VfsError::NotFound)
        );
        assert!(sent(&server, "PATCH").await.is_empty());
    }
}

#[tokio::test]
async fn remove_at_the_top_of_my_drive_checks_the_real_root_id() {
    let server = MockServer::start().await;
    let root = ok_json(folder_json("real-root", "Mine"));
    mount_path(&server, "/files/root", root).await;
    mount_held(&server, "file-1", "a.txt", PLAIN, &["real-root"], None).await;
    let status = ResponseTemplate::new(200);
    mount_method(&server, "PATCH", "/files/file-1", status).await;
    let provider = drive(&server);
    provider.remove(&id("file-1"), &id("root")).await.unwrap();
    assert!(sent(&server, "GET")
        .await
        .iter()
        .any(|request| request.url.path() == "/files/root"));
    assert_eq!(sent(&server, "PATCH").await.len(), 1);

    let missing = MockServer::start().await;
    let body = ok_json(folder_json("real-root", "Mine"));
    mount_path(&missing, "/files/root", body).await;
    mount_held(&missing, "file-1", "a.txt", PLAIN, &["other"], None).await;
    let provider = drive(&missing);
    assert_eq!(
        provider.remove(&id("file-1"), &id("root")).await,
        Err(VfsError::NotFound)
    );
    let requests = missing.received_requests().await.unwrap();
    assert!(requests
        .iter()
        .any(|request| request.url.path() == "/files/root"));
    assert!(sent(&missing, "PATCH").await.is_empty());
}

#[tokio::test]
async fn remove_of_a_link_file_trashes_its_drive_file() {
    let server = MockServer::start().await;
    mount_held(&server, "abc", "Notes", DOC, &["folder"], None).await;
    mount_method(&server, "PATCH", "/files/abc", ResponseTemplate::new(200)).await;
    let provider = drive(&server);
    provider
        .remove(&id("abc#abc"), &id("folder"))
        .await
        .unwrap();
    let patches = sent(&server, "PATCH").await;
    let patch = &patches[0];
    assert_eq!(patch.url.path(), "/files/abc");
    assert_eq!(body_of(patch), json!({ "trashed": true }));
}

#[tokio::test]
async fn a_rename_in_one_folder_sends_the_name_and_keeps_the_id() {
    let server = MockServer::start().await;
    mount_held(&server, "file-1", "old.txt", PLAIN, &["folder"], None).await;
    let body = ok_json(held_json("file-1", "new.txt", PLAIN, &["folder"], None));
    mount_method(&server, "PATCH", "/files/file-1", body).await;
    let provider = drive(&server);
    let renamed = provider
        .rename(&id("file-1"), &id("folder"), &id("folder"), "new.txt", None)
        .await
        .unwrap();
    assert_eq!(renamed.id, id("file-1"));
    assert_eq!(renamed.name, "new.txt");
    let patches = sent(&server, "PATCH").await;
    let patch = &patches[0];
    assert_eq!(body_of(patch), json!({ "name": "new.txt" }));
    let query = query_of(patch);
    assert_eq!(query.get("addParents"), None);
    assert_eq!(query.get("removeParents"), None);
    assert_eq!(query.get("fields").unwrap(), ITEM_FIELDS);
}

#[tokio::test]
async fn a_move_sends_the_drive_ids_of_the_two_folders() {
    let server = MockServer::start().await;
    mount_held(&server, "m", "a.txt", PLAIN, &["a"], None).await;
    let body = ok_json(held_json("m", "a.txt", PLAIN, &["b"], None));
    mount_method(&server, "PATCH", "/files/m", body).await;
    mount_held(&server, "s", "a.txt", PLAIN, &["d"], Some("d")).await;
    let body = ok_json(held_json("s", "a.txt", PLAIN, &["f"], Some("d")));
    mount_method(&server, "PATCH", "/files/s", body).await;
    mount_held(&server, "t", "a.txt", PLAIN, &["f"], Some("d")).await;
    let body = ok_json(held_json("t", "a.txt", PLAIN, &["d"], Some("d")));
    mount_method(&server, "PATCH", "/files/t", body).await;
    let provider = drive(&server);
    let mine = provider
        .rename(&id("m"), &id("a"), &id("b"), "a.txt", None)
        .await
        .unwrap();
    assert_eq!(mine.id, id("m"));
    let into_folder = provider
        .rename(&id("d/s"), &id("d/d"), &id("d/f"), "a.txt", None)
        .await
        .unwrap();
    assert_eq!(into_folder.id, id("d/s"));
    let into_top = provider
        .rename(&id("d/t"), &id("d/f"), &id("d/d"), "a.txt", None)
        .await
        .unwrap();
    assert_eq!(into_top.id, id("d/t"));
    let patches = sent(&server, "PATCH").await;
    let query = |path_str: &str| {
        query_of(
            patches
                .iter()
                .find(|request| request.url.path() == path_str)
                .unwrap(),
        )
    };
    let mine = query("/files/m");
    assert_eq!(mine.get("addParents").unwrap(), "b");
    assert_eq!(mine.get("removeParents").unwrap(), "a");
    let into_folder = query("/files/s");
    assert_eq!(into_folder.get("addParents").unwrap(), "f");
    assert_eq!(into_folder.get("removeParents").unwrap(), "d");
    let into_top = query("/files/t");
    assert_eq!(into_top.get("addParents").unwrap(), "d");
    assert_eq!(into_top.get("removeParents").unwrap(), "f");
}

#[tokio::test]
async fn a_file_moved_between_drives_gets_the_drives_id() {
    let server = MockServer::start().await;
    mount_held(&server, "file-1", "a.txt", PLAIN, &["src"], None).await;
    let body = ok_json(held_json("file-1", "a.txt", PLAIN, &["dest"], Some("d")));
    mount_method(&server, "PATCH", "/files/file-1", body).await;
    mount_held(&server, "file-2", "b.txt", PLAIN, &["drive-src"], Some("d")).await;
    let body = ok_json(held_json("file-2", "b.txt", PLAIN, &["my-dest"], None));
    mount_method(&server, "PATCH", "/files/file-2", body).await;
    let provider = drive(&server);
    let into_shared = provider
        .rename(&id("file-1"), &id("src"), &id("d/dest"), "a.txt", None)
        .await
        .unwrap();
    assert_eq!(into_shared.id, id("d/file-1"));
    let into_mine = provider
        .rename(
            &id("d/file-2"),
            &id("d/drive-src"),
            &id("my-dest"),
            "b.txt",
            None,
        )
        .await
        .unwrap();
    assert_eq!(into_mine.id, id("file-2"));
    let patches = sent(&server, "PATCH").await;
    let shared = query_of(
        patches
            .iter()
            .find(|request| request.url.path() == "/files/file-1")
            .unwrap(),
    );
    assert_eq!(shared.get("addParents").unwrap(), "dest");
    assert_eq!(shared.get("removeParents").unwrap(), "src");
    let mine = query_of(
        patches
            .iter()
            .find(|request| request.url.path() == "/files/file-2")
            .unwrap(),
    );
    assert_eq!(mine.get("addParents").unwrap(), "my-dest");
    assert_eq!(mine.get("removeParents").unwrap(), "drive-src");
}

#[tokio::test]
async fn a_folder_moved_between_drives_is_not_supported() {
    let server = MockServer::start().await;
    mount_held(&server, "folder-1", "Docs", FOLDER, &["src"], None).await;
    let provider = drive(&server);
    let moved = provider
        .rename(&id("folder-1"), &id("src"), &id("d/dest"), "Docs", None)
        .await;
    assert_eq!(moved, Err(VfsError::NotSupported));
    assert!(sent(&server, "PATCH").await.is_empty());
}

#[tokio::test]
async fn a_folder_moved_into_itself_or_into_its_own_subtree_is_not_supported() {
    let itself = MockServer::start().await;
    mount_held(&itself, "folder", "Docs", FOLDER, &["p"], None).await;
    let provider = drive(&itself);
    let moved = provider
        .rename(&id("folder"), &id("p"), &id("folder"), "Docs", None)
        .await;
    assert_eq!(moved, Err(VfsError::NotSupported));
    let requests = itself.received_requests().await.unwrap();
    assert!(sent(&itself, "PATCH").await.is_empty());
    assert_eq!(requests.len(), 1);

    let inside = MockServer::start().await;
    mount_held(&inside, "folder", "Docs", FOLDER, &["p"], None).await;
    let body = ok_json(json!({ "id": "grand", "parents": ["child"] }));
    mount_path(&inside, "/files/grand", body).await;
    let body = ok_json(json!({ "id": "child", "parents": ["folder"] }));
    mount_path(&inside, "/files/child", body).await;
    let provider = drive(&inside);
    let moved = provider
        .rename(&id("folder"), &id("p"), &id("grand"), "Docs", None)
        .await;
    assert_eq!(moved, Err(VfsError::NotSupported));
    assert!(sent(&inside, "PATCH").await.is_empty());
    let requests = inside.received_requests().await.unwrap();
    let paths: Vec<_> = requests.iter().map(|request| request.url.path()).collect();
    assert_eq!(paths, ["/files/folder", "/files/grand", "/files/child"]);

    // A move to the top of My Drive walks up to a folder with no parents and goes on.
    let up = MockServer::start().await;
    mount_held(&up, "folder", "Docs", FOLDER, &["p"], None).await;
    mount_path(&up, "/files/root", ok_json(json!({ "id": "real-root" }))).await;
    let body = ok_json(held_json("folder", "Docs", FOLDER, &["real-root"], None));
    mount_method(&up, "PATCH", "/files/folder", body).await;
    let provider = drive(&up);
    provider
        .rename(&id("folder"), &id("p"), &id("root"), "Docs", None)
        .await
        .unwrap();
    let patches = sent(&up, "PATCH").await;
    let query = query_of(&patches[0]);
    assert_eq!(query.get("addParents").unwrap(), "root");
    assert_eq!(query.get("removeParents").unwrap(), "p");

    let sibling = MockServer::start().await;
    mount_held(&sibling, "folder", "Docs", FOLDER, &["p"], None).await;
    let body = ok_json(json!({ "id": "sib", "parents": ["p"] }));
    mount_path(&sibling, "/files/sib", body).await;
    let body = ok_json(held_json("folder", "Docs", FOLDER, &["sib"], None));
    mount_method(&sibling, "PATCH", "/files/folder", body).await;
    let provider = drive(&sibling);
    let moved = provider
        .rename(&id("folder"), &id("p"), &id("sib"), "Docs", None)
        .await
        .unwrap();
    assert_eq!(moved.id, id("folder"));
    let patches = sent(&sibling, "PATCH").await;
    let query = query_of(&patches[0]);
    assert_eq!(query.get("addParents").unwrap(), "sib");
    assert_eq!(query.get("removeParents").unwrap(), "p");
}

#[tokio::test]
async fn a_link_file_keeps_its_ending_when_it_is_renamed() {
    let server = MockServer::start().await;
    mount_held(&server, "doc", "Notes", DOC, &["folder"], None).await;
    let body = ok_json(held_json("doc", "Plan", DOC, &["folder"], None));
    mount_method(&server, "PATCH", "/files/doc", body).await;
    let provider = drive(&server);
    let renamed = provider
        .rename(
            &id("doc#doc"),
            &id("folder"),
            &id("folder"),
            "Plan.document.html",
            None,
        )
        .await
        .unwrap();
    assert_eq!(renamed.id, id("doc#doc"));
    assert_eq!(renamed.name, "Plan.document.html");
    let renamed = provider
        .rename(
            &id("doc#doc"),
            &id("folder"),
            &id("folder"),
            "Notes.txt",
            None,
        )
        .await;
    assert_eq!(renamed, Err(VfsError::NotSupported));
    let patches = sent(&server, "PATCH").await;
    assert_eq!(patches.len(), 1);
    assert_eq!(body_of(&patches[0]), json!({ "name": "Plan" }));
    assert_eq!(query_of(&patches[0]).get("addParents"), None);
}

#[tokio::test]
async fn a_move_whose_answer_was_lost_is_read_back() {
    let applied = MockServer::start().await;
    let answers = vec![
        ok_json(held_json("file-1", "old.txt", PLAIN, &["from-1"], None)),
        ok_json(held_json("file-1", "new.txt", PLAIN, &["to-1"], None)),
    ];
    mount_answers(&applied, "GET", "/files/file-1", answers).await;
    mount_held(&applied, "old-1", "new.txt", PLAIN, &["to-1"], None).await;
    let failed = ResponseTemplate::new(500);
    mount_method(&applied, "PATCH", "/files/file-1", failed).await;
    let status = ResponseTemplate::new(200);
    mount_method(&applied, "PATCH", "/files/old-1", status).await;
    let provider = drive(&applied);
    let moved = provider
        .rename(
            &id("file-1"),
            &id("from-1"),
            &id("to-1"),
            "new.txt",
            Some(&id("old-1")),
        )
        .await
        .unwrap();
    assert_eq!(moved.id, id("file-1"));
    assert_eq!(moved.name, "new.txt");
    let patches = sent(&applied, "PATCH").await;
    assert!(patches.len() > 1);
    assert_eq!(patches[0].url.path(), "/files/file-1");
    assert_eq!(body_of(&patches[0])["name"], "new.txt");
    assert_eq!(query_of(&patches[0]).get("addParents").unwrap(), "to-1");
    assert_eq!(
        query_of(&patches[0]).get("removeParents").unwrap(),
        "from-1"
    );
    // The move is sent again after each 500, and the trash follows the last of those.
    let trash = patches
        .iter()
        .position(|request| request.url.path() == "/files/old-1")
        .unwrap();
    assert!(trash > 0);
    assert!(patches[..trash]
        .iter()
        .all(|request| request.url.path() == "/files/file-1"));
    assert_eq!(body_of(&patches[trash]), json!({ "trashed": true }));

    // A Google Doc in a shared drive: the read-back asks for the id with the drive and the id the page opens.
    let link = MockServer::start().await;
    let answers = vec![
        ok_json(held_json("doc", "Notes", DOC, &["from-1"], Some("d"))),
        ok_json(held_json("doc", "Notes", DOC, &["f"], Some("d"))),
    ];
    mount_answers(&link, "GET", "/files/doc", answers).await;
    mount_method(&link, "PATCH", "/files/doc", ResponseTemplate::new(500)).await;
    let provider = drive(&link);
    let moved = provider
        .rename(
            &id("d/doc#doc"),
            &id("d/from-1"),
            &id("d/f"),
            "Notes.document.html",
            None,
        )
        .await
        .unwrap();
    assert_eq!(moved.id, id("d/doc#doc"));
    assert_eq!(moved.name, "Notes.document.html");

    // An answer whose body cannot be read is read back too: the move may have happened.
    let unread = MockServer::start().await;
    let answers = vec![
        ok_json(held_json("file-1", "old.txt", PLAIN, &["from-1"], None)),
        ok_json(held_json("file-1", "new.txt", PLAIN, &["to-1"], None)),
    ];
    mount_answers(&unread, "GET", "/files/file-1", answers).await;
    let cut = ResponseTemplate::new(200).set_body_raw("{", "application/json");
    mount_method(&unread, "PATCH", "/files/file-1", cut).await;
    let provider = drive(&unread);
    let moved = provider
        .rename(&id("file-1"), &id("from-1"), &id("to-1"), "new.txt", None)
        .await
        .unwrap();
    assert_eq!(moved.name, "new.txt");
    assert_eq!(sent(&unread, "PATCH").await.len(), 1);

    // A plain rename whose answer was lost is read back too.
    let renamed = MockServer::start().await;
    let answers = vec![
        ok_json(held_json("file-1", "old.txt", PLAIN, &["folder"], None)),
        ok_json(held_json("file-1", "new.txt", PLAIN, &["folder"], None)),
    ];
    mount_answers(&renamed, "GET", "/files/file-1", answers).await;
    mount_method(
        &renamed,
        "PATCH",
        "/files/file-1",
        ResponseTemplate::new(500),
    )
    .await;
    let provider = drive(&renamed);
    let moved = provider
        .rename(&id("file-1"), &id("folder"), &id("folder"), "new.txt", None)
        .await
        .unwrap();
    assert_eq!(moved.name, "new.txt");
    assert_eq!(
        query_of(&sent(&renamed, "PATCH").await[0]).get("addParents"),
        None
    );

    let still = MockServer::start().await;
    mount_held(&still, "file-1", "old.txt", PLAIN, &["from-1"], None).await;
    mount_method(&still, "PATCH", "/files/file-1", ResponseTemplate::new(500)).await;
    let provider = drive(&still);
    let moved = provider
        .rename(&id("file-1"), &id("from-1"), &id("to-1"), "new.txt", None)
        .await;
    assert_eq!(
        moved,
        Err(VfsError::Other(
            "Google Drive answered 500 Internal Server Error".into()
        ))
    );
    assert!(sent(&still, "PATCH")
        .await
        .iter()
        .all(|request| request.url.path() == "/files/file-1"));
}

#[tokio::test]
async fn replace_moves_first_and_trashes_second() {
    let server = MockServer::start().await;
    mount_held(&server, "file-1", "old.txt", PLAIN, &["from-1"], None).await;
    mount_held(&server, "old-1", "new.txt", PLAIN, &["to-1"], None).await;
    let body = ok_json(held_json("file-1", "new.txt", PLAIN, &["to-1"], None));
    mount_method(&server, "PATCH", "/files/file-1", body).await;
    mount_method(&server, "PATCH", "/files/old-1", ResponseTemplate::new(500)).await;
    let provider = drive(&server);
    let moved = provider
        .rename(
            &id("file-1"),
            &id("from-1"),
            &id("to-1"),
            "new.txt",
            Some(&id("old-1")),
        )
        .await;
    assert_eq!(
        moved,
        Err(VfsError::Other(
            "Google Drive answered 500 Internal Server Error".into()
        ))
    );
    let patches = sent(&server, "PATCH").await;
    assert_eq!(patches[0].url.path(), "/files/file-1");
    assert_eq!(body_of(&patches[0])["name"], "new.txt");
    assert!(patches.len() > 1);
    assert!(
        patches[1..]
            .iter()
            .all(|request| request.url.path() == "/files/old-1"
                && body_of(request)["trashed"] == true)
    );

    let full = MockServer::start().await;
    mount_held(&full, "file-1", "old.txt", PLAIN, &["folder"], None).await;
    mount_held(&full, "full", "Docs", FOLDER, &["folder"], None).await;
    let body = ok_json(file_page(vec![plain("child", "a.txt")], None));
    mount_path(&full, "/files", body).await;
    let provider = drive(&full);
    let moved = provider
        .rename(
            &id("file-1"),
            &id("folder"),
            &id("folder"),
            "Docs",
            Some(&id("full")),
        )
        .await;
    assert_eq!(moved, Err(VfsError::NotEmpty));
    assert!(sent(&full, "PATCH").await.is_empty());

    let missing = MockServer::start().await;
    mount_held(&missing, "file-1", "old.txt", PLAIN, &["folder"], None).await;
    mount_held(&missing, "old-1", "new.txt", PLAIN, &["elsewhere"], None).await;
    let provider = drive(&missing);
    let moved = provider
        .rename(
            &id("file-1"),
            &id("folder"),
            &id("folder"),
            "new.txt",
            Some(&id("old-1")),
        )
        .await;
    assert_eq!(moved, Err(VfsError::NotFound));
    assert!(sent(&missing, "PATCH").await.is_empty());
}

#[tokio::test]
async fn the_folders_a_mount_makes_up_cannot_be_changed() {
    let server = MockServer::start().await;
    let provider = drive(&server);
    let top = id("/");
    let shared = id("/shared-drives");
    let root = id("root");
    let drive_top = id("d/d");
    let file = id("file-1");
    let link = id("doc#doc");
    let folder = id("folder");
    let denied = Err::<Item, _>(VfsError::AccessDenied);
    let removed = Err::<(), _>(VfsError::AccessDenied);
    let not_folder = Err::<Item, _>(VfsError::NotAFolder);
    let missing = Err::<(), _>(VfsError::NotFound);
    let not_found = Err::<Item, _>(VfsError::NotFound);
    assert_eq!(provider.create_folder(&top, "a").await, denied);
    assert_eq!(provider.create_folder(&shared, "a").await, denied);
    assert_eq!(provider.create_folder(&link, "a").await, not_folder);
    assert_eq!(provider.remove(&top, &folder).await, removed);
    assert_eq!(provider.remove(&shared, &folder).await, removed);
    assert_eq!(provider.remove(&root, &folder).await, removed);
    assert_eq!(provider.remove(&drive_top, &folder).await, removed);
    assert_eq!(provider.remove(&file, &link).await, missing);
    assert_eq!(
        provider.rename(&top, &folder, &folder, "a", None).await,
        denied
    );
    assert_eq!(
        provider.rename(&shared, &folder, &folder, "a", None).await,
        denied
    );
    assert_eq!(
        provider.rename(&root, &folder, &folder, "a", None).await,
        denied
    );
    assert_eq!(
        provider
            .rename(&drive_top, &folder, &folder, "a", None)
            .await,
        denied
    );
    assert_eq!(
        provider.rename(&file, &folder, &top, "a", None).await,
        denied
    );
    assert_eq!(
        provider.rename(&file, &folder, &shared, "a", None).await,
        denied
    );
    assert_eq!(
        provider.rename(&file, &folder, &link, "a", None).await,
        not_folder
    );
    assert_eq!(
        provider.rename(&file, &link, &folder, "a", None).await,
        not_found
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn moving_a_duplicate_link_file_under_its_shown_name_is_not_supported() {
    let server = MockServer::start().await;
    let body = ok_json(folder_json("alias-root", "My Drive"));
    mount_path(&server, "/files/root", body).await;
    Mock::given(method("GET"))
        .and(query_param("q", "'root' in parents and trashed = false"))
        .respond_with(ok_json(file_page(
            vec![folder_json("src", "Source"), folder_json("dst", "Dest")],
            None,
        )))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(query_param("q", "'src' in parents and trashed = false"))
        .respond_with(ok_json(file_page(
            vec![
                file_json("doc-1", "Notes", DOC, "0"),
                file_json("doc-2", "Notes", DOC, "0"),
            ],
            None,
        )))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(query_param("q", "'dst' in parents and trashed = false"))
        .respond_with(ok_json(file_page(vec![], None)))
        .mount(&server)
        .await;
    // An empty listing is checked with `item`, which asks for the folder itself.
    let dest = ok_json(folder_json("dst", "Dest"));
    mount_path(&server, "/files/dst", dest).await;
    mount_held(&server, "doc-2", "Notes", DOC, &["src"], None).await;
    let drive = Arc::new(drive(&server));
    let vfs = Vfs::new(drive, POSIX, Options::default());
    let my_drive = vfs.lookup(Vfs::ROOT, "My Drive").await.unwrap();
    let source = vfs.lookup(my_drive.node, "Source").await.unwrap();
    let dest = vfs.lookup(my_drive.node, "Dest").await.unwrap();
    assert_eq!(
        vfs.rename(
            source.node,
            "Notes.document (2).html",
            dest.node,
            "Notes.document (2).html",
            IfExists::Refuse,
        )
        .await,
        Err(VfsError::NotSupported)
    );
    assert!(sent(&server, "PATCH").await.is_empty());
}

#[tokio::test]
async fn a_file_moved_into_a_shared_drive_keeps_its_node() {
    let server = MockServer::start().await;
    let body = ok_json(folder_json("alias-root", "My Drive"));
    mount_path(&server, "/files/root", body).await;
    let drives = json!({
        "drives": [{ "id": "team", "name": "Team", "createdTime": MODIFIED }]
    });
    mount_path(&server, "/drives", ok_json(drives)).await;
    Mock::given(method("GET"))
        .and(query_param("q", "'root' in parents and trashed = false"))
        .respond_with(ok_json(file_page(vec![plain("file-1", "notes.txt")], None)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(query_param("q", "'team' in parents and trashed = false"))
        .respond_with(ok_json(file_page(vec![], None)))
        .mount(&server)
        .await;
    // An empty listing of a shared drive's top folder is checked with `drives.get`.
    let body = ok_json(json!({ "id": "team", "name": "Team", "createdTime": MODIFIED }));
    mount_path(&server, "/drives/team", body).await;
    mount_held(&server, "file-1", "notes.txt", PLAIN, &["alias-root"], None).await;
    let file = held_json("file-1", "moved.txt", PLAIN, &["team"], Some("team"));
    let body = ok_json(file);
    mount_method(&server, "PATCH", "/files/file-1", body).await;
    let drive = Arc::new(drive(&server));
    let vfs = Vfs::new(drive, POSIX, Options::default());
    let my_drive = vfs.lookup(Vfs::ROOT, "My Drive").await.unwrap();
    let file = vfs.lookup(my_drive.node, "notes.txt").await.unwrap();
    let shared = vfs.lookup(Vfs::ROOT, "Shared drives").await.unwrap();
    let team = vfs.lookup(shared.node, "Team").await.unwrap();
    vfs.rename(
        my_drive.node,
        "notes.txt",
        team.node,
        "moved.txt",
        IfExists::Refuse,
    )
    .await
    .unwrap();
    let moved = vfs.lookup(team.node, "moved.txt").await.unwrap();
    assert_eq!(moved.node, file.node);
    let patches = sent(&server, "PATCH").await;
    let query = query_of(&patches[0]);
    assert_eq!(query.get("addParents").unwrap(), "team");
    assert_eq!(query.get("removeParents").unwrap(), "root");
    assert_eq!(body_of(&patches[0])["name"], "moved.txt");
}
