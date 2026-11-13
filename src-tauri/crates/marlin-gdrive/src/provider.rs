use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use marlin_vfs::{Attributes, Item, ItemId, Kind, Provider, Space, VfsError};
use reqwest::StatusCode;

use crate::api::{self, About, DriveFile, DrivePage, FilePage};

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
}

fn place(id: &ItemId) -> Place<'_> {
    match id.0.as_str() {
        MOUNT_ROOT => Place::Top,
        SHARED_DRIVES => Place::SharedDrives,
        real => match real.split_once('/') {
            Some((drive, file)) => Place::File {
                drive: Some(drive),
                file,
            },
            None => Place::File {
                drive: None,
                file: real,
            },
        },
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

/// The item a mount shows for a Drive file, or `None` for one it leaves out.
fn item_from(id: ItemId, file: DriveFile) -> Option<Item> {
    let (kind, size) = if file.mime_type == "application/vnd.google-apps.folder" {
        (Kind::Folder, 0)
    } else if file.mime_type.starts_with("application/vnd.google-apps.") {
        // Docs, Sheets, Slides, shortcuts, and the rest have no bytes to read. Issue #527 decides how a mount shows them.
        return None;
    } else {
        let size = file.size.and_then(|size| size.parse().ok());
        (Kind::File, size.unwrap_or(0))
    };
    Some(Item {
        id,
        name: file.name,
        attributes: Attributes {
            kind,
            size,
            modified: modified(file.modified_time.as_deref()),
        },
    })
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
        let file = match place(id) {
            Place::Top => return Ok(made_up_folder(id, "")),
            Place::SharedDrives => return Ok(made_up_folder(id, "Shared drives")),
            Place::File { file, .. } => file,
        };
        let query = [
            ("fields", "id,name,mimeType,size,modifiedTime,trashed"),
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
        // The id stays the one the core asked for, so My Drive stays `root`.
        item_from(id.clone(), found).ok_or(VfsError::NotSupported)
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
                    for drive in page.drives {
                        items.push(Item {
                            // A shared drive's id is also the id of its top folder.
                            id: ItemId(format!("{0}/{0}", drive.id)),
                            name: drive.name,
                            attributes: Attributes {
                                kind: Kind::Folder,
                                size: 0,
                                modified: modified(drive.created_time.as_deref()),
                            },
                        });
                    }
                    let Some(next) = page.next_page_token else {
                        return Ok(items);
                    };
                    page_token = Some(next);
                }
            }
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
                            "nextPageToken,files(id,name,mimeType,size,modifiedTime)",
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
                        let id = match drive {
                            Some(drive) => format!("{drive}/{}", file.id),
                            None => file.id.clone(),
                        };
                        items.extend(item_from(ItemId(id), file));
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
        let Place::File { file, .. } = place(id) else {
            return Err(VfsError::IsAFolder);
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
