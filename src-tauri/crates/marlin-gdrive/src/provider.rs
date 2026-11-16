use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use marlin_vfs::{Attributes, Item, ItemId, Kind, Provider, Space, VfsError};
use reqwest::StatusCode;

use crate::api::{self, About, DriveFile, DrivePage, FilePage, SharedDrive};

/// The id of the top of a mount. A Drive id never holds `/`, so neither made-up id can be a real one.
const MOUNT_ROOT: &str = "/";

/// The id of the folder that lists the account's shared drives.
const SHARED_DRIVES: &str = "/shared-drives";

/// Google's own name for the My Drive folder. It works wherever a file id does.
const MY_DRIVE: &str = "root";

/// Gives the provider an access token for one Google account.
#[async_trait::async_trait]
pub trait AccessTokens: Send + Sync + 'static {
    /// A token Google should accept now. An account that must sign in again is [`VfsError::SignInNeeded`].
    async fn access_token(&self) -> Result<String, VfsError>;

    /// Google answered 401 to `token`. `access_token` must never return that token again. A token that is no longer the current one changes nothing, so many calls can report one stale token and cause one refresh.
    fn rejected(&self, token: &str);
}

/// Where the Drive API is and how long to wait before a retry.
#[derive(Debug, Clone)]
pub struct Config {
    /// The API's base address, with no slash at the end.
    pub api: String,

    /// The wait before the first retry. Each later retry waits twice as long as the one before.
    pub retry_delay: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api: "https://www.googleapis.com/drive/v3".to_string(),
            retry_delay: Duration::from_millis(500),
        }
    }
}

/// One Google account, read through the Drive API. It shows My Drive and the shared drives, and it never changes anything.
pub struct DriveProvider {
    pub(crate) tokens: Arc<dyn AccessTokens>,
    pub(crate) config: Config,
    pub(crate) client: reqwest::Client,
}

impl DriveProvider {
    /// Reads the account that `tokens` signs in to.
    pub fn new(tokens: Arc<dyn AccessTokens>, config: Config) -> Self {
        // No total timeout: the mount core limits each call and cancels it by dropping the future.
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("the HTTP client starts with the default TLS");
        Self {
            tokens,
            config,
            client,
        }
    }
}

/// What an item id names.
enum Place<'a> {
    Top,
    SharedDrives,
    /// A Drive file or folder, and the shared drive it is in, if any.
    ///
    /// An item in a shared drive has the id `{drive id}/{file id}`. Google lists a shared drive's folder in full only when the request names that drive, and the provider keeps nothing between calls, so the id has to carry it.
    File {
        drive: Option<&'a str>,
        file: &'a str,
    },
    /// A link file: a small web page that opens a Drive item in the browser. A mount shows one for each Drive item that has no bytes to read.
    ///
    /// Its id is the id the item would have as a `File`, then `#`, then the id the page opens. `read` gets an id and nothing else, so the id has to carry it.
    Link {
        drive: Option<&'a str>,
        file: &'a str,
        opens: &'a str,
    },
}

fn place(id: &ItemId) -> Place<'_> {
    match id.0.as_str() {
        MOUNT_ROOT => Place::Top,
        SHARED_DRIVES => Place::SharedDrives,
        real => {
            let (real, opens) = match real.split_once('#') {
                Some((real, opens)) => (real, Some(opens)),
                None => (real, None),
            };
            let (drive, file) = match real.split_once('/') {
                Some((drive, file)) => (Some(drive), file),
                None => (None, real),
            };
            match opens {
                Some(opens) => Place::Link { drive, file, opens },
                None => Place::File { drive, file },
            }
        }
    }
}

/// One of the two folders that exist only in a mount. Google knows neither, so neither has a modified time.
fn made_up_folder(id: &ItemId, name: &str) -> Item {
    Item {
        id: id.clone(),
        name: name.to_string(),
        attributes: Attributes {
            kind: Kind::Folder,
            size: 0,
            modified: UNIX_EPOCH,
        },
    }
}

/// The item a mount shows for a Drive file. `drive` is the shared drive it is in, if any.
///
/// A Google Doc, Sheet, shortcut, or other Google item that is not a folder has no bytes to read, so it is a link file.
fn item_from(drive: Option<&str>, id: String, file: DriveFile) -> Item {
    let modified = modified(file.modified_time.as_deref());
    let plain = match drive {
        Some(drive) => format!("{drive}/{id}"),
        None => id.clone(),
    };
    let (id, name, kind, size) = if file.mime_type == "application/vnd.google-apps.folder" {
        (plain, file.name, Kind::Folder, 0)
    } else if let Some(google_type) = file.mime_type.strip_prefix("application/vnd.google-apps.") {
        // Only a shortcut has a target. Every other item opens itself.
        let target = file
            .shortcut_details
            .and_then(|shortcut| shortcut.target_id);
        let opens = target.unwrap_or(id);
        // A third-party shortcut's type is `drive-sdk.{app id}`.
        let google_type = google_type
            .split_once('.')
            .map_or(google_type, |(word, _)| word);
        (
            format!("{plain}#{opens}"),
            format!("{}.{google_type}.html", file.name),
            Kind::File,
            link_page(&opens).len() as u64,
        )
    } else {
        let size = file.size.and_then(|size| size.parse().ok());
        (plain, file.name, Kind::File, size.unwrap_or(0))
    };
    Item {
        id: ItemId(id),
        name,
        attributes: Attributes {
            kind,
            size,
            modified,
        },
    }
}

/// The bytes of a link file. They depend on the id alone, so the size is known when a folder is listed and a read sends no request.
fn link_page(opens: &str) -> String {
    format!(
        "\
<!doctype html>
<meta charset=\"utf-8\">
<meta http-equiv=\"refresh\" content=\"0; url=https://drive.google.com/open?id={opens}\">
<title>Open in Google Drive</title>
<a href=\"https://drive.google.com/open?id={opens}\">Open in Google Drive</a>
"
    )
}

/// The item a mount shows for a shared drive: the drive's top folder under the drive's own name. Google gives a shared drive no modified time, so the time is when the drive was made.
fn shared_drive_item(drive: SharedDrive) -> Item {
    Item {
        // A shared drive's id is also the id of its top folder.
        id: ItemId(format!("{0}/{0}", drive.id)),
        name: drive.name,
        attributes: Attributes {
            kind: Kind::Folder,
            size: 0,
            modified: modified(drive.created_time.as_deref()),
        },
    }
}

/// A time as Google writes it. One that is missing or unreadable is the start of 1970.
fn modified(time: Option<&str>) -> SystemTime {
    time.and_then(|time| chrono::DateTime::parse_from_rfc3339(time).ok())
        .map_or(UNIX_EPOCH, SystemTime::from)
}

#[async_trait::async_trait]
impl Provider for DriveProvider {
    fn root(&self) -> ItemId {
        ItemId(MOUNT_ROOT.to_string())
    }

    async fn item(&self, id: &ItemId) -> Result<Item, VfsError> {
        let (drive, file) = match place(id) {
            Place::Top => return Ok(made_up_folder(id, "")),
            Place::SharedDrives => return Ok(made_up_folder(id, "Shared drives")),
            // The top folder of a shared drive. `files.get` gives that folder another name than the drive's, so the answer comes from the drive itself, as it does in the listing of shared drives.
            Place::File {
                drive: Some(drive),
                file,
            } if drive == file => {
                let query = [("fields", "id,name,createdTime")];
                let response = self.get(&format!("/drives/{drive}"), &query, None).await?;
                return Ok(shared_drive_item(api::json(response).await?));
            }
            Place::File { drive, file } | Place::Link { drive, file, .. } => (drive, file),
        };
        let query = [
            (
                "fields",
                "id,name,mimeType,size,modifiedTime,trashed,shortcutDetails(targetId)",
            ),
            ("supportsAllDrives", "true"),
        ];
        let response = self.get(&format!("/files/{file}"), &query, None).await?;
        let mut found: DriveFile = api::json(response).await?;
        if found.trashed {
            return Err(VfsError::NotFound);
        }
        // The two folders at the top of a mount have the same names for every account, whatever Google calls My Drive.
        if file == MY_DRIVE {
            found.name = "My Drive".to_string();
        }
        let item = item_from(drive, file.to_string(), found);
        // The id says whether the core holds a plain item or a link, and which id the link opens. An answer that builds another id is not the item the core holds, and its size would not be the size of the bytes `read` returns.
        if item.id != *id {
            return Err(VfsError::NotFound);
        }
        Ok(item)
    }

    async fn list(&self, folder: &ItemId) -> Result<Vec<Item>, VfsError> {
        match place(folder) {
            // Asking Google for My Drive here makes a dead sign-in show at the top of the mount.
            Place::Top => Ok(vec![
                self.item(&ItemId(MY_DRIVE.to_string())).await?,
                self.item(&ItemId(SHARED_DRIVES.to_string())).await?,
            ]),
            Place::SharedDrives => {
                let mut items = Vec::new();
                let mut page_token: Option<String> = None;
                loop {
                    let mut query = vec![
                        ("pageSize", "100"),
                        ("fields", "nextPageToken,drives(id,name,createdTime)"),
                    ];
                    if let Some(token) = &page_token {
                        query.push(("pageToken", token.as_str()));
                    }
                    let response = self.get("/drives", &query, None).await?;
                    let page: DrivePage = api::json(response).await?;
                    items.extend(page.drives.into_iter().map(shared_drive_item));
                    let Some(next) = page.next_page_token else {
                        return Ok(items);
                    };
                    page_token = Some(next);
                }
            }
            Place::Link { .. } => Err(VfsError::NotAFolder),
            Place::File {
                drive,
                file: parent,
            } => {
                let children = format!("'{parent}' in parents and trashed = false");
                let mut items = Vec::new();
                let mut google_listed_any = false;
                let mut page_token: Option<String> = None;
                loop {
                    let mut query = vec![
                        ("q", children.as_str()),
                        (
                            "fields",
                            "nextPageToken,files(id,name,mimeType,size,modifiedTime,shortcutDetails(targetId))",
                        ),
                        ("pageSize", "1000"),
                        ("supportsAllDrives", "true"),
                        ("includeItemsFromAllDrives", "true"),
                    ];
                    if let Some(drive) = drive {
                        query.push(("corpora", "drive"));
                        query.push(("driveId", drive));
                    }
                    if let Some(token) = &page_token {
                        query.push(("pageToken", token.as_str()));
                    }
                    let response = self.get("/files", &query, None).await?;
                    let page: FilePage = api::json(response).await?;
                    google_listed_any |= !page.files.is_empty();
                    for file in page.files {
                        // A folder's children are in the folder's own drive.
                        items.push(item_from(drive, file.id.clone(), file));
                    }
                    let Some(next) = page.next_page_token else {
                        break;
                    };
                    page_token = Some(next);
                }
                // Google answers a listing of a file, and of a folder that is gone, with an empty list and no error.
                if !google_listed_any && self.item(folder).await?.attributes.kind != Kind::Folder {
                    return Err(VfsError::NotAFolder);
                }
                Ok(items)
            }
        }
    }

    async fn read(&self, id: &ItemId, offset: u64, length: u32) -> Result<Vec<u8>, VfsError> {
        let file = match place(id) {
            Place::File { file, .. } => file,
            Place::Link { opens, .. } => {
                // A link file's bytes come from its id alone, so Google is not asked.
                let page = link_page(opens);
                let start = offset.min(page.len() as u64) as usize;
                let end = start.saturating_add(length as usize).min(page.len());
                return Ok(page.as_bytes()[start..end].to_vec());
            }
            Place::Top | Place::SharedDrives => return Err(VfsError::IsAFolder),
        };
        if length == 0 {
            return Ok(Vec::new());
        }
        let last = offset.saturating_add(u64::from(length) - 1);
        let range = format!("bytes={offset}-{last}");
        let query = [("alt", "media"), ("supportsAllDrives", "true")];
        let mut response = self
            .get(&format!("/files/{file}"), &query, Some(&range))
            .await?;
        // The range starts at or past the end of the file.
        if response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
            return Ok(Vec::new());
        }
        // A 200 is the whole file: the server ignored the range. Skip to the offset and stop at the length, so the file is never held in memory.
        let mut skip = if response.status() == StatusCode::OK {
            offset
        } else {
            0
        };
        let length = length as usize;
        let mut bytes = Vec::new();
        while bytes.len() < length {
            let Some(chunk) = response.chunk().await.map_err(|_| VfsError::Offline)? else {
                break;
            };
            let skipped = skip.min(chunk.len() as u64) as usize;
            skip -= skipped as u64;
            let kept = (length - bytes.len()).min(chunk.len() - skipped);
            bytes.extend_from_slice(&chunk[skipped..skipped + kept]);
        }
        Ok(bytes)
    }

    async fn space(&self) -> Result<Space, VfsError> {
        let query = [("fields", "storageQuota(limit,usage)")];
        let response = self.get("/about", &query, None).await?;
        let quota = api::json::<About>(response).await?.storage_quota;
        let usage = quota.usage.and_then(|usage| usage.parse().ok());
        let usage: u64 = usage.unwrap_or(0);
        let Some(limit) = quota.limit.and_then(|limit| limit.parse::<u64>().ok()) else {
            // Google gives some Workspace accounts no limit. A mount must report some number, and 0 free would make file managers refuse to copy. This is 1 PiB.
            let free = 1 << 50;
            return Ok(Space {
                total: usage.saturating_add(free),
                free,
            });
        };
        Ok(Space {
            total: limit,
            free: limit.saturating_sub(usage),
        })
    }
}
