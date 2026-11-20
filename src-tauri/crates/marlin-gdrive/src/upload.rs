use std::io::SeekFrom;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use futures_util::stream::{self, Stream};
use marlin_vfs::{ItemId, Progress, Revision, VfsError};
use reqwest::{header, Method, StatusCode};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::api::{self, DriveFile, Failure};
use crate::provider::{DriveProvider, ITEM_FIELDS};

/// Google takes any length on the request that ends a file, and only a multiple of this on every other request.
const PIECE: u64 = 256 * 1024;

/// The most one request sends, as in Google's own Go client. A failed request sends its bytes again, and the core counts only bytes past the highest count it heard, so this bounds what a resend must send before the count moves. On a link slower than about 2 Mbit/s, that can still take longer than the core's minute without progress.
const CHUNK: u64 = 16 * 1024 * 1024;

const UNREADABLE: &str = "Marlin could not read the file it uploads";

/// What an upload makes.
#[derive(Clone, Copy)]
pub(crate) enum Upload<'a> {
    /// A new file under the pre-made id `id`, named `name`, in the Drive folder `parent`.
    Create {
        id: &'a str,
        name: &'a str,
        parent: &'a str,
    },
    /// New bytes for the Drive file `file`, which the core holds as `id`, based on revision `base`. `mime_type` is the file's type now: without it, Drive guesses the type again from the new bytes, and a `.txt` file can become an image.
    Replace {
        file: &'a str,
        id: &'a ItemId,
        base: &'a Revision,
        mime_type: &'a str,
    },
}

/// What one session request carries.
enum Sends<'a> {
    /// The question: how much of a file of this many bytes Google holds.
    Question(u64),
    /// `length` bytes of the file at `from` from `at`, of a file of `size` bytes.
    Bytes {
        from: &'a Path,
        at: u64,
        length: u64,
        size: u64,
    },
    /// The whole of an empty file: no body and no range.
    Empty,
}

/// What Google answered one session request.
enum Answer {
    /// Google has the whole file.
    Done(DriveFile),
    /// A 308: Google holds this many bytes and waits for the rest.
    Holds(u64),
    /// A 401. The token is marked rejected, and the request goes again with a new one.
    Rejected,
    /// A failure that leaves the session as it was: a server error or a dropped connection. The next request asks what Google holds.
    Again(VfsError),
    /// A failure that ends the session: a lost session (a 404 or a 410) or a rate limit. Google's guide starts an upload again in a new session after a 4xx.
    Restart(VfsError),
    /// An answer that ends the call.
    Stop(VfsError),
}

impl DriveProvider {
    /// Uploads the file at `from` through a resumable session, with the modified time `modified`, and returns the file Google stored.
    ///
    /// A replace checks its revision before every request that ends the file, a resend included. Google applies the new bytes only when the last byte arrives, so another client's change can be overwritten only during that check and the last piece.
    pub(crate) async fn upload(
        &self,
        upload: Upload<'_>,
        from: &Path,
        modified: SystemTime,
        progress: &Progress,
    ) -> Result<DriveFile, VfsError> {
        // Drive's own form of a time: UTC, with milliseconds.
        let modified = chrono::DateTime::<chrono::Utc>::from(modified)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let (method, path, metadata) = match upload {
            Upload::Create { id, name, parent } => {
                let metadata = serde_json::json!({
                    "id": id,
                    "name": name,
                    "parents": [parent],
                    "modifiedTime": modified,
                });
                (Method::POST, "/files".to_string(), metadata)
            }
            Upload::Replace {
                file, mime_type, ..
            } => {
                let metadata = serde_json::json!({
                    "mimeType": mime_type,
                    "modifiedTime": modified,
                });
                (Method::PATCH, format!("/files/{file}"), metadata)
            }
        };
        // The length when the call starts. A file that changes during the call is sent again by the core.
        let size = tokio::fs::metadata(from)
            .await
            .map_err(|_| VfsError::Other(UNREADABLE.into()))?
            .len();
        let mut session = self.open_session(method.clone(), &path, &metadata).await?;
        let mut held = 0;
        // The most Google has held in this call, over every session. A failure count goes back to 0 only past it, so a call that keeps losing its session still ends.
        let mut most = 0;
        let mut failures = 0;
        let mut rejected = false;
        // After a failure, the next request asks what Google holds instead of sending bytes.
        let mut asking = false;
        loop {
            let sends = if asking {
                Sends::Question(size)
            } else {
                let rest = size - held;
                // A request that does not end the file carries whole pieces, whatever Google already holds. A new file goes in as few requests as it can. A replace keeps the last 1 to PIECE bytes for a request of their own, so its revision check runs just before Google applies the new bytes.
                let create = matches!(upload, Upload::Create { .. });
                let length = if rest <= PIECE || (create && rest <= CHUNK) {
                    rest
                } else {
                    ((rest - 1) / PIECE * PIECE).min(CHUNK)
                };
                if held + length == size {
                    if let Upload::Replace { id, base, .. } = upload {
                        self.replaceable(id, base).await?;
                    }
                }
                match size {
                    0 => Sends::Empty,
                    _ => Sends::Bytes {
                        from,
                        at: held,
                        length,
                        size,
                    },
                }
            };
            let answer = self.send(&session, sends, progress).await;
            // A 401 right after a 401 means the new token was refused too.
            let rejected_before =
                std::mem::replace(&mut rejected, matches!(answer, Answer::Rejected));
            let (failure, restart) = match answer {
                Answer::Done(file) => return Ok(file),
                Answer::Stop(error) => return Err(error),
                Answer::Rejected if rejected_before => return Err(VfsError::SignInNeeded),
                Answer::Rejected => continue,
                // The answer to the question is where to go on from. A 308 that names every byte should have been a 200.
                Answer::Holds(now) if now < size && (asking || now > held) => {
                    (held, asking) = (now, false);
                    if held > most {
                        (most, failures) = (held, 0);
                    }
                    continue;
                }
                // Bytes went out and Google kept none of them.
                Answer::Holds(_) => (api::answered(StatusCode::PERMANENT_REDIRECT), false),
                Answer::Again(error) => (error, false),
                Answer::Restart(error) => (error, true),
            };
            if failures == api::RETRIES {
                return Err(failure);
            }
            tokio::time::sleep(self.retry_wait(failures)).await;
            failures += 1;
            if restart {
                // A create keeps its pre-made id in the new session, so it still cannot land twice.
                session = self.open_session(method.clone(), &path, &metadata).await?;
                (held, asking) = (0, false);
            } else {
                // An empty file has nothing to ask about, and the question ends it with no revision check, so it is sent again.
                asking = size > 0;
            }
        }
    }

    /// Opens a resumable session and returns its URL. The request uploads nothing, so it goes through [`Self::request_url`] and its retries like any other.
    async fn open_session(
        &self,
        method: Method,
        path: &str,
        metadata: &serde_json::Value,
    ) -> Result<String, VfsError> {
        let url = format!("{}{path}", self.config.upload);
        let query = [
            ("uploadType", "resumable"),
            ("supportsAllDrives", "true"),
            ("fields", ITEM_FIELDS),
        ];
        let response = self
            .request_url(method, &url, &query, Some(metadata), None)
            .await?;
        response
            .headers()
            .get(header::LOCATION)
            .and_then(|location| location.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| {
                VfsError::Other("Google Drive sent an answer Marlin could not read".into())
            })
    }

    /// Sends one session request.
    async fn send(&self, session: &str, sends: Sends<'_>, progress: &Progress) -> Answer {
        let token = match self.tokens.access_token().await {
            Ok(token) => token,
            Err(error) => return Answer::Stop(error),
        };
        let request = self.client.put(session).bearer_auth(&token);
        // The body must own what it uses, so it counts the bytes it read into `read` and marks a file it could not read in `unreadable`.
        let read = Arc::new(AtomicU64::new(0));
        let unreadable = Arc::new(AtomicBool::new(false));
        let mut start = 0;
        let request = match sends {
            Sends::Question(size) => request
                .header(header::CONTENT_LENGTH, 0)
                .header(header::CONTENT_RANGE, format!("bytes */{size}")),
            Sends::Empty => request.header(header::CONTENT_LENGTH, 0),
            Sends::Bytes {
                from,
                at,
                length,
                size,
            } => {
                let Ok(mut file) = tokio::fs::File::open(from).await else {
                    return Answer::Stop(VfsError::Other(UNREADABLE.into()));
                };
                if file.seek(SeekFrom::Start(at)).await.is_err() {
                    return Answer::Stop(VfsError::Other(UNREADABLE.into()));
                }
                start = at;
                let body = pieces(file, length, Arc::clone(&read), Arc::clone(&unreadable));
                let last = at + length - 1;
                request
                    .header(header::CONTENT_LENGTH, length)
                    .header(header::CONTENT_RANGE, format!("bytes {at}-{last}/{size}"))
                    .body(reqwest::Body::wrap_stream(body))
            }
        };
        let sending = request.send();
        tokio::pin!(sending);
        let mut ticks = tokio::time::interval(Duration::from_millis(250));
        let sent = loop {
            tokio::select! {
                sent = &mut sending => break sent,
                _ = ticks.tick() => progress(start + read.load(Ordering::Relaxed)),
            }
        };
        progress(start + read.load(Ordering::Relaxed));
        if unreadable.load(Ordering::Relaxed) {
            return Answer::Stop(VfsError::Other(UNREADABLE.into()));
        }
        let Ok(response) = sent else {
            return Answer::Again(VfsError::Offline);
        };
        let status = response.status();
        match status {
            StatusCode::OK | StatusCode::CREATED => match api::json(response).await {
                Ok(file) => Answer::Done(file),
                // The headers came and the body did not. Drive may have the file, and the question after the wait gets it again.
                Err(VfsError::Offline) => Answer::Again(VfsError::Offline),
                Err(error) => Answer::Stop(error),
            },
            StatusCode::PERMANENT_REDIRECT => {
                // `Range: bytes=0-{n}` means Google holds n + 1 bytes. No `Range` means it holds none.
                let held = response
                    .headers()
                    .get(header::RANGE)
                    .and_then(|range| range.to_str().ok())
                    .and_then(|range| range.strip_prefix("bytes=0-"))
                    .and_then(|last| last.parse::<u64>().ok())
                    .map_or(0, |last| last + 1);
                Answer::Holds(held)
            }
            StatusCode::UNAUTHORIZED => {
                self.tokens.rejected(&token);
                Answer::Rejected
            }
            // Google sends a 409 when a create's pre-made id is already in use: this call's file already landed, and `create_file` reads it back. A replace passes it on as `Conflict`.
            StatusCode::CONFLICT => Answer::Stop(VfsError::Conflict),
            StatusCode::NOT_FOUND | StatusCode::GONE => {
                Answer::Restart(VfsError::Other("Google Drive ended the upload".into()))
            }
            _ => match api::failure(response).await {
                Failure::Limited => Answer::Restart(api::limiting()),
                Failure::Server(status) => Answer::Again(api::answered(status)),
                Failure::Unread => Answer::Again(VfsError::Offline),
                Failure::Final(error) => Answer::Stop(error),
            },
        }
    }
}

/// `left` bytes of `file` from where it stands, one [`PIECE`] at a time, so only one piece is in memory. `read` counts the bytes read. A file that cannot be read, or that ends early because it was cut during the upload, sets `unreadable`.
fn pieces(
    file: tokio::fs::File,
    left: u64,
    read: Arc<AtomicU64>,
    unreadable: Arc<AtomicBool>,
) -> impl Stream<Item = std::io::Result<Vec<u8>>> + Send + 'static {
    stream::try_unfold((file, left), move |(mut file, left)| {
        let (read, unreadable) = (Arc::clone(&read), Arc::clone(&unreadable));
        async move {
            if left == 0 {
                return Ok(None);
            }
            let mut piece = vec![0; left.min(PIECE) as usize];
            if let Err(error) = file.read_exact(&mut piece).await {
                unreadable.store(true, Ordering::Relaxed);
                return Err(error);
            }
            read.fetch_add(piece.len() as u64, Ordering::Relaxed);
            let left = left - piece.len() as u64;
            Ok(Some((piece, (file, left))))
        }
    })
}
