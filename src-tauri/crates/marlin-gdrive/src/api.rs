use marlin_vfs::VfsError;
use reqwest::StatusCode;
use serde::Deserialize;

use crate::DriveProvider;

/// How many times one call is sent again after a rate limit or a server error.
const RETRIES: u32 = 3;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DriveFile {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) mime_type: String,
    pub(crate) size: Option<String>,
    pub(crate) modified_time: Option<String>,
    #[serde(default)]
    pub(crate) trashed: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FilePage {
    pub(crate) next_page_token: Option<String>,
    #[serde(default)]
    pub(crate) files: Vec<DriveFile>,
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
    /// Sends one GET until Google answers it, the token is rejected twice, or the retries run out. A 416 is an answer: only a read can get one.
    ///
    /// A request that gets no answer is `Offline`, and so is a body that stops arriving. The only time limit here is on connecting, so a timeout also means Google could not be reached.
    pub(crate) async fn get(
        &self,
        path: &str,
        query: &[(&str, &str)],
        range: Option<&str>,
    ) -> Result<reqwest::Response, VfsError> {
        let mut rejected = false;
        let mut retries = 0;
        loop {
            let token = self.tokens.access_token().await?;
            let mut request = self
                .client
                .get(format!("{}{path}", self.config.api))
                .query(query)
                .bearer_auth(&token);
            if let Some(range) = range {
                request = request.header(reqwest::header::RANGE, range);
            }
            let response = request.send().await.map_err(|_| VfsError::Offline)?;
            let status = response.status();
            let gave_up = match status {
                _ if status.is_success() => return Ok(response),
                StatusCode::RANGE_NOT_SATISFIABLE => return Ok(response),
                StatusCode::UNAUTHORIZED => {
                    self.tokens.rejected(&token);
                    if rejected {
                        return Err(VfsError::SignInNeeded);
                    }
                    rejected = true;
                    continue;
                }
                StatusCode::NOT_FOUND => return Err(VfsError::NotFound),
                StatusCode::TOO_MANY_REQUESTS => {
                    VfsError::Other("Google Drive is limiting requests".into())
                }
                // Google sends 403 for a rate limit and for a file the account may not read. Only the reason tells them apart.
                StatusCode::FORBIDDEN => {
                    let body = response.bytes().await.map_err(|_| VfsError::Offline)?;
                    let body: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
                    let reason = body
                        .pointer("/error/errors/0/reason")
                        .and_then(|reason| reason.as_str());
                    if !matches!(reason, Some("rateLimitExceeded" | "userRateLimitExceeded")) {
                        return Err(VfsError::AccessDenied);
                    }
                    VfsError::Other("Google Drive is limiting requests".into())
                }
                _ if status.is_server_error() => {
                    VfsError::Other(format!("Google Drive answered {status}"))
                }
                _ => return Err(VfsError::Other(format!("Google Drive answered {status}"))),
            };
            if retries == RETRIES {
                return Err(gave_up);
            }
            // The random part keeps calls that failed together from coming back together.
            let wait = self.config.retry_delay * 2u32.pow(retries);
            tokio::time::sleep(wait + wait.mul_f64(fastrand::f64() / 2.0)).await;
            retries += 1;
        }
    }
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
