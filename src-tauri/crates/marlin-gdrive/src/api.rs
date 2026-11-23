use marlin_vfs::VfsError;
use reqwest::StatusCode;
use serde::Deserialize;

use crate::DriveProvider;

/// How many times one call is sent again after a rate limit or a server error.
pub(crate) const RETRIES: u32 = 3;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DriveFile {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) mime_type: String,
    pub(crate) size: Option<String>,
    pub(crate) modified_time: Option<String>,
    /// Set when the request asked for it. Drive changes it only when the bytes change.
    pub(crate) head_revision_id: Option<String>,
    #[serde(default)]
    pub(crate) trashed: bool,
    pub(crate) shortcut_details: Option<ShortcutDetails>,
    /// My Drive's real id is in here, never `root`. Empty when the request did not ask for parents.
    #[serde(default)]
    pub(crate) parents: Vec<String>,
    /// Set for a file in a shared drive. My Drive leaves it out.
    pub(crate) drive_id: Option<String>,
    /// Set when the request asked for it.
    pub(crate) capabilities: Option<Capabilities>,
}

impl DriveFile {
    /// Whether Google says the account may not give this file new bytes. No answer is not a no.
    pub(crate) fn read_only(&self) -> bool {
        self.capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.can_modify_content)
            == Some(false)
    }
}

/// What the account may do with a file.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Capabilities {
    /// Whether the account may give the file new bytes.
    pub(crate) can_modify_content: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShortcutDetails {
    pub(crate) target_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FilePage {
    pub(crate) next_page_token: Option<String>,
    #[serde(default)]
    pub(crate) files: Vec<DriveFile>,
}

/// A file's id and parents, for the checks that need nothing else. `parents` is empty at the top of My Drive and of a shared drive, and when the request did not ask for it.
#[derive(Deserialize)]
pub(crate) struct FileParents {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) parents: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct GeneratedIds {
    /// The request asks for one. Any other count is an answer Marlin could not read.
    pub(crate) ids: [String; 1],
}

/// A page of children when only their presence matters. Google can send an empty page that still has a next page.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChildPage {
    pub(crate) next_page_token: Option<String>,
    #[serde(default)]
    pub(crate) files: Vec<serde::de::IgnoredAny>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SharedDrive {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) created_time: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DrivePage {
    pub(crate) next_page_token: Option<String>,
    #[serde(default)]
    pub(crate) drives: Vec<SharedDrive>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct About {
    #[serde(default)]
    pub(crate) storage_quota: StorageQuota,
}

#[derive(Deserialize, Default)]
pub(crate) struct StorageQuota {
    pub(crate) limit: Option<String>,
    pub(crate) usage: Option<String>,
}

impl DriveProvider {
    /// How long to wait before try `retries`. Each later wait is twice the one before, plus up to half again at random, so calls that failed together do not come back together.
    pub(crate) fn retry_wait(&self, retries: u32) -> std::time::Duration {
        let wait = self.config.retry_delay * 2u32.pow(retries);
        wait + wait.mul_f64(fastrand::f64() / 2.0)
    }

    /// [`Self::request_url`] for a path under `config.api`.
    pub(crate) async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&serde_json::Value>,
        range: Option<&str>,
    ) -> Result<reqwest::Response, VfsError> {
        let url = format!("{}{path}", self.config.api);
        self.request_url(method, &url, query, body, range).await
    }

    /// Sends one request until Google answers it, the token is rejected twice, or the retries run out. Every request takes this path but an upload's `PUT`s, which have their own rules in `upload.rs`.
    ///
    /// A 416 is an answer only when a range was sent: only a ranged read can get one. A 409 is not sent again: Google sends it only when a create's pre-made id is already in use.
    ///
    /// A request that gets no answer is `Offline`, and so is a body that stops arriving. The only time limit here is on connecting, so a timeout also means Google could not be reached.
    pub(crate) async fn request_url(
        &self,
        method: reqwest::Method,
        url: &str,
        query: &[(&str, &str)],
        body: Option<&serde_json::Value>,
        range: Option<&str>,
    ) -> Result<reqwest::Response, VfsError> {
        let mut rejected = false;
        let mut retries = 0;
        loop {
            let token = self.tokens.access_token().await?;
            let mut request = self
                .client
                .request(method.clone(), url)
                .query(query)
                .bearer_auth(&token);
            if let Some(body) = body {
                request = request.json(body);
            }
            if let Some(range) = range {
                request = request.header(reqwest::header::RANGE, range);
            }
            let response = request.send().await.map_err(|_| VfsError::Offline)?;
            let status = response.status();
            let gave_up = match status {
                _ if status.is_success() => return Ok(response),
                StatusCode::RANGE_NOT_SATISFIABLE if range.is_some() => return Ok(response),
                StatusCode::UNAUTHORIZED => {
                    self.tokens.rejected(&token);
                    if rejected {
                        return Err(VfsError::SignInNeeded);
                    }
                    rejected = true;
                    continue;
                }
                StatusCode::NOT_FOUND => return Err(VfsError::NotFound),
                StatusCode::CONFLICT => return Err(VfsError::Conflict),
                _ => match failure(response).await {
                    Failure::Limited => limiting(),
                    Failure::Server(status) => answered(status),
                    Failure::Unread => return Err(VfsError::Offline),
                    Failure::Final(error) => return Err(error),
                },
            };
            if retries == RETRIES {
                return Err(gave_up);
            }
            tokio::time::sleep(self.retry_wait(retries)).await;
            retries += 1;
        }
    }
}

/// What a status that is neither an answer nor a 401 means. Each caller decides how it tries again.
pub(crate) enum Failure {
    /// A 429, or a 403 for a rate limit.
    Limited,
    /// A 500 to 599.
    Server(StatusCode),
    /// A 403 whose body did not arrive, so its reason is not known.
    Unread,
    /// An answer that ends the call: a full drive, a refusal, or a status the table does not name.
    Final(VfsError),
}

pub(crate) async fn failure(response: reqwest::Response) -> Failure {
    let status = response.status();
    match status {
        StatusCode::TOO_MANY_REQUESTS => Failure::Limited,
        // Google sends 403 for a rate limit, a full drive, and a file the account may not change. Only the reason tells them apart.
        StatusCode::FORBIDDEN => {
            let Ok(body) = response.bytes().await else {
                return Failure::Unread;
            };
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            match body
                .pointer("/error/errors/0/reason")
                .and_then(|reason| reason.as_str())
            {
                Some("rateLimitExceeded" | "userRateLimitExceeded") => Failure::Limited,
                Some("storageQuotaExceeded") => Failure::Final(VfsError::NoSpace),
                _ => Failure::Final(VfsError::AccessDenied),
            }
        }
        _ if status.is_server_error() => Failure::Server(status),
        _ => Failure::Final(answered(status)),
    }
}

/// The error for a rate limit that outlasts the retries.
pub(crate) fn limiting() -> VfsError {
    VfsError::Other("Google Drive is limiting requests".into())
}

/// The error for a status that has no error of its own. It names only the status, so the helper can log it.
pub(crate) fn answered(status: StatusCode) -> VfsError {
    VfsError::Other(format!("Google Drive answered {status}"))
}

/// The JSON body of an answer.
pub(crate) async fn json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, VfsError> {
    // Read first, then parse: a connection that drops is the network's failure, and a body that is not the expected JSON is Google's.
    let body = response.bytes().await.map_err(|_| VfsError::Offline)?;
    serde_json::from_slice(&body)
        .map_err(|_| VfsError::Other("Google Drive sent an answer Marlin could not read".into()))
}
