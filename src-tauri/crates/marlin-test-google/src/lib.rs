//! A fake Google for Marlin's tests only: the Drive API, the token endpoint, the sign-in page, and the user info.
//! `app-tests/` runs it through the program in `src/main.rs`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const MODIFIED: &str = "2020-01-02T03:04:05.678Z";

/// The one account the sign-in page returns. No name and no picture, so the app fetches nothing else.
pub const USER_EMAIL: &str = "app-test@example.com";

pub struct FakeFile {
    pub id: &'static str,
    pub name: &'static str,
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

/// How the Drive API answers. The token endpoint answers the same in every case.
pub enum Drive {
    Up,
    /// Every call answers 503.
    Down,
    /// Every call answers 403 with a reason that is not a rate limit, as a Workspace policy that blocks the app does.
    Forbidden,
}

pub struct GoogleState {
    pub access_token: String,
    /// The one refresh token the fake Google accepts.
    pub refresh_token: String,
    pub rejected: bool,
    pub drive: Drive,
    pub files: Vec<FakeFile>,
}

impl GoogleState {
    fn new() -> Self {
        Self {
            access_token: "access-1".to_string(),
            refresh_token: "refresh-1".to_string(),
            rejected: false,
            drive: Drive::Up,
            files: vec![
                FakeFile {
                    id: "big",
                    name: "big.bin",
                    mime: "application/octet-stream",
                    bytes: pattern(300_000),
                },
                FakeFile {
                    id: "same-1",
                    name: "same.txt",
                    mime: "application/octet-stream",
                    bytes: b"one".to_vec(),
                },
                FakeFile {
                    id: "same-2",
                    name: "same.txt",
                    mime: "application/octet-stream",
                    bytes: b"two".to_vec(),
                },
                FakeFile {
                    id: "slash",
                    name: "a/b.txt",
                    mime: "application/octet-stream",
                    bytes: b"slash".to_vec(),
                },
                FakeFile {
                    id: "doc",
                    name: "Notes",
                    mime: "application/vnd.google-apps.document",
                    bytes: Vec::new(),
                },
            ],
        }
    }
}

struct FakeGoogle {
    state: Arc<Mutex<GoogleState>>,
}

pub fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index % 251) as u8).collect()
}

fn file_json(file: &FakeFile) -> Value {
    json!({
        "id": file.id,
        "name": file.name,
        "mimeType": file.mime,
        "size": file.bytes.len().to_string(),
        "modifiedTime": MODIFIED,
        "trashed": false,
    })
}

impl Respond for FakeGoogle {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let path = request.url.path();
        let method = request.method.as_str();
        let state = self.state.lock().unwrap();
        if method == "GET" && path == "/auth" {
            let redirect_uri = request
                .url
                .query_pairs()
                .find(|(key, _)| key == "redirect_uri")
                .map(|(_, value)| value.into_owned());
            let Some(redirect_uri) = redirect_uri else {
                return ResponseTemplate::new(400);
            };
            // The fake user picks the account at once.
            return ResponseTemplate::new(302)
                .insert_header("Location", format!("{redirect_uri}?code=code-1"));
        }
        if method == "POST" && path == "/token" {
            // A form body. The values the tests send need no decoding.
            let fields: HashMap<&str, &str> = std::str::from_utf8(&request.body)
                .unwrap_or("")
                .split('&')
                .filter_map(|pair| pair.split_once('='))
                .collect();
            let refused =
                ResponseTemplate::new(400).set_body_json(json!({"error": "invalid_grant"}));
            if state.rejected {
                return refused;
            }
            // A token only for the code the sign-in page gave, or for the one refresh token the fake knows.
            return match fields.get("grant_type") {
                Some(&"authorization_code") if fields.get("code") == Some(&"code-1") => {
                    ResponseTemplate::new(200).set_body_json(json!({
                        "access_token": state.access_token,
                        "refresh_token": state.refresh_token,
                        "expires_in": 3600,
                    }))
                }
                Some(&"refresh_token")
                    if fields.get("refresh_token") == Some(&state.refresh_token.as_str()) =>
                {
                    ResponseTemplate::new(200).set_body_json(json!({
                        "access_token": state.access_token,
                        "expires_in": 3600,
                    }))
                }
                _ => refused,
            };
        }
        let authorization = request
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if authorization != format!("Bearer {}", state.access_token) {
            return ResponseTemplate::new(401);
        }
        if method == "GET" && path == "/userinfo" {
            return ResponseTemplate::new(200).set_body_json(json!({"email": USER_EMAIL}));
        }
        match state.drive {
            Drive::Up => {}
            Drive::Down => return ResponseTemplate::new(503),
            Drive::Forbidden => {
                return ResponseTemplate::new(403)
                    .set_body_json(json!({"error": {"errors": [{"reason": "domainPolicy"}]}}))
            }
        }
        if method != "GET" {
            return ResponseTemplate::new(404);
        }
        if path == "/drives" {
            return ResponseTemplate::new(200).set_body_json(json!({"drives": []}));
        }
        if path == "/about" {
            return ResponseTemplate::new(200).set_body_json(json!({
                "storageQuota": {"limit": "1000", "usage": "400"}
            }));
        }
        let query: HashMap<String, String> = request
            .url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        if path == "/files" {
            let q = query.get("q").map(String::as_str).unwrap_or("");
            if !q.contains("'root' in parents") {
                return ResponseTemplate::new(200).set_body_json(json!({"files": []}));
            }
            let (shown, next) = if query.get("pageToken").map(String::as_str) == Some("page-2") {
                (&state.files[2..], None)
            } else {
                (&state.files[..2], Some("page-2"))
            };
            let mut body = json!({
                "files": shown.iter().map(file_json).collect::<Vec<_>>(),
            });
            if let Some(next) = next {
                body["nextPageToken"] = json!(next);
            }
            return ResponseTemplate::new(200).set_body_json(body);
        }
        let Some(id) = path.strip_prefix("/files/") else {
            return ResponseTemplate::new(404);
        };
        if id == "root" {
            return ResponseTemplate::new(200).set_body_json(json!({
                "id": "root",
                "name": "My Drive",
                "mimeType": "application/vnd.google-apps.folder",
                "modifiedTime": MODIFIED,
                "trashed": false,
            }));
        }
        let Some(file) = state.files.iter().find(|file| file.id == id) else {
            return ResponseTemplate::new(404);
        };
        if query.get("alt").map(String::as_str) != Some("media") {
            return ResponseTemplate::new(200).set_body_json(file_json(file));
        }
        if file.mime.starts_with("application/vnd.google-apps.") {
            return ResponseTemplate::new(403)
                .set_body_json(json!({"error": {"errors": [{"reason": "fileNotDownloadable"}]}}));
        }
        let range = request
            .headers
            .get("range")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        let Some(rest) = range.strip_prefix("bytes=") else {
            return ResponseTemplate::new(404);
        };
        let Some((start, end)) = rest.split_once('-') else {
            return ResponseTemplate::new(404);
        };
        let (Ok(start), Ok(end)) = (start.parse::<u64>(), end.parse::<u64>()) else {
            return ResponseTemplate::new(404);
        };
        let len = file.bytes.len() as u64;
        if start >= len {
            return ResponseTemplate::new(416);
        }
        let end = end.min(len - 1);
        ResponseTemplate::new(206)
            .set_body_bytes(file.bytes[start as usize..=end as usize].to_vec())
    }
}

pub async fn start() -> (MockServer, Arc<Mutex<GoogleState>>) {
    let server = MockServer::start().await;
    let state = Arc::new(Mutex::new(GoogleState::new()));
    Mock::given(any())
        .respond_with(FakeGoogle {
            state: Arc::clone(&state),
        })
        .mount(&server)
        .await;
    (server, state)
}
