use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use marlin_vfs::{
    Attributes, Item, ItemId, Kind, Progress, Provider, Revision, Space, Thumbnail, VfsError,
};
use reqwest::{Method, StatusCode};

use crate::api::{self, About, DriveFile, DrivePage, FilePage, FileParents, SharedDrive};
use crate::upload::Upload;

/// The id of the top of a mount. A Drive id never holds `/`, so neither made-up id can be a real one.
const MOUNT_ROOT: &str = "/";

/// The id of the folder that lists the account's shared drives.
const SHARED_DRIVES: &str = "/shared-drives";

/// Google's own name for the My Drive folder. It works wherever a file id does.
const MY_DRIVE: &str = "root";

/// The MIME type of a Drive folder.
const FOLDER_TYPE: &str = "application/vnd.google-apps.folder";

/// The fields `item`, the change calls, and an upload's answer ask for: what an item is built from, plus `trashed` and `driveId` for their checks.
///
/// `headRevisionId` changes only when the bytes change. `version` also changes on a rename or a share, so it is not here. `canModifyContent` says whether the account may give a file new bytes. `canEdit` is wider, and other things can limit it.
pub(crate) const ITEM_FIELDS: &str =
    "id,name,mimeType,size,modifiedTime,headRevisionId,trashed,driveId,shortcutDetails(targetId),capabilities(canModifyContent)";

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

    /// The base address for uploads, with no slash at the end.
    pub upload: String,

    /// The wait before the first retry. Each later retry waits twice as long as the one before.
    pub retry_delay: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api: "https://www.googleapis.com/drive/v3".to_string(),
            upload: "https://www.googleapis.com/upload/drive/v3".to_string(),
            retry_delay: Duration::from_millis(500),
        }
    }
}

/// One Google account through the Drive API. It shows My Drive and the shared drives.
///
/// It can make a folder, rename or move an item, trash one, and write a file's bytes. `can_write` is true. A mount still takes writes only when it has a spool.
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

impl Place<'_> {
    /// The id that names this place: the reverse of [`place`].
    fn id(&self) -> ItemId {
        let real = |drive: &Option<&str>, file: &str| match drive {
            Some(drive) => format!("{drive}/{file}"),
            None => file.to_string(),
        };
        ItemId(match self {
            Place::Top => MOUNT_ROOT.to_string(),
            Place::SharedDrives => SHARED_DRIVES.to_string(),
            Place::File { drive, file } => real(drive, file),
            Place::Link { drive, file, opens } => format!("{}#{opens}", real(drive, file)),
        })
    }
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
            revision: None,
            read_only: false,
        },
    }
}

/// The ending a link file's shown name adds to the name Google stores. A folder and a plain file have none, so a rename of those sends the shown name unchanged.
///
/// A third-party shortcut's type is `drive-sdk.{app id}`, and the ending keeps only the word before the dot.
fn link_ending(mime_type: &str) -> Option<String> {
    let google_type = mime_type.strip_prefix("application/vnd.google-apps.")?;
    if google_type == "folder" {
        return None;
    }
    let google_type = google_type
        .split_once('.')
        .map_or(google_type, |(word, _)| word);
    Some(format!(".{google_type}.html"))
}

/// Where a mount shows the Drive file `id`, and the ending its name gets. `drive` is the shared drive it is in, if any.
///
/// A Google Doc, Sheet, shortcut, or other Google item that is not a folder has no bytes to read, so it is a link file: its id carries the id its page opens, and its name gets an ending.
fn shown<'a>(
    drive: Option<&'a str>,
    id: &'a str,
    file: &'a DriveFile,
) -> (Place<'a>, Option<String>) {
    let Some(ending) = link_ending(&file.mime_type) else {
        return (Place::File { drive, file: id }, None);
    };
    // Only a shortcut has a target. Every other item opens itself.
    let target = file
        .shortcut_details
        .as_ref()
        .and_then(|shortcut| shortcut.target_id.as_deref());
    let link = Place::Link {
        drive,
        file: id,
        opens: target.unwrap_or(id),
    };
    (link, Some(ending))
}

/// The item a mount shows for a Drive file. `drive` is the shared drive it is in, if any.
fn item_from(drive: Option<&str>, id: String, file: DriveFile) -> Item {
    let modified = modified(file.modified_time.as_deref());
    let (place, ending) = shown(drive, &id, &file);
    let item_id = place.id();
    // A folder has no bytes, and a link file's bytes are its page, which never changes, so neither has a revision. A link file has no bytes on Drive to replace, so it is always read-only.
    let (kind, size, revision, read_only) = match place {
        _ if file.mime_type == FOLDER_TYPE => (Kind::Folder, 0, None, false),
        Place::Link { opens, .. } => (Kind::File, link_page(opens).len() as u64, None, true),
        _ => {
            let size = file.size.as_deref().and_then(|size| size.parse().ok());
            let read_only = file.read_only();
            (
                Kind::File,
                size.unwrap_or(0),
                file.head_revision_id.map(Revision),
                read_only,
            )
        }
    };
    let name = match ending {
        Some(ending) => format!("{}{ending}", file.name),
        None => file.name,
    };
    Item {
        id: item_id,
        name,
        attributes: Attributes {
            kind,
            size,
            modified,
            revision,
            read_only,
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
        id: Place::File {
            drive: Some(&drive.id),
            file: &drive.id,
        }
        .id(),
        name: drive.name,
        attributes: Attributes {
            kind: Kind::Folder,
            size: 0,
            modified: modified(drive.created_time.as_deref()),
            revision: None,
            read_only: false,
        },
    }
}

/// The drive and the Drive folder of a folder that takes a new item. `/` and Shared drives are not Drive folders, and a link file is not a folder.
fn destination(id: &ItemId) -> Result<(Option<&str>, &str), VfsError> {
    match place(id) {
        Place::Top | Place::SharedDrives => Err(VfsError::AccessDenied),
        Place::Link { .. } => Err(VfsError::NotAFolder),
        Place::File { drive, file } => Ok((drive, file)),
    }
}

/// The Drive file of an item a caller removes or renames, and the id a link file opens. `/`, Shared drives, My Drive, and a shared drive's top folder are not Drive files that can be removed or renamed.
fn changeable(id: &ItemId) -> Result<(&str, Option<&str>), VfsError> {
    match place(id) {
        Place::Top | Place::SharedDrives => Err(VfsError::AccessDenied),
        Place::File {
            drive: None,
            file: MY_DRIVE,
        } => Err(VfsError::AccessDenied),
        Place::File {
            drive: Some(drive),
            file,
        } if drive == file => Err(VfsError::AccessDenied),
        Place::File { file, .. } => Ok((file, None)),
        Place::Link { file, opens, .. } => Ok((file, Some(opens))),
    }
}

/// The query for the children of a folder that are not trashed. `children` is the `q` that names the folder. Google lists a shared drive's folder in full only when the request names that drive.
fn children_query<'a>(drive: Option<&'a str>, children: &'a str) -> Vec<(&'a str, &'a str)> {
    let mut query = vec![
        ("q", children),
        ("supportsAllDrives", "true"),
        ("includeItemsFromAllDrives", "true"),
    ];
    if let Some(drive) = drive {
        query.push(("corpora", "drive"));
        query.push(("driveId", drive));
    }
    query
}

/// A time as Google writes it. One that is missing or unreadable is the start of 1970.
fn modified(time: Option<&str>) -> SystemTime {
    time.and_then(|time| chrono::DateTime::parse_from_rfc3339(time).ok())
        .map_or(UNIX_EPOCH, SystemTime::from)
}

impl DriveProvider {
    /// The Drive file the core holds as `id` in `folder`, as Google has it now.
    ///
    /// `NotFound` when Google has trashed it, moved it out of `folder`, or given it a drive or a type that would build a different id. My Drive's top is `root` in an id, while `parents` holds My Drive's real id, so that id is read at the same time and nothing is kept.
    async fn held(&self, id: &ItemId, folder: &ItemId) -> Result<DriveFile, VfsError> {
        let drive = match place(id) {
            Place::File { drive, .. } | Place::Link { drive, .. } => drive,
            Place::Top | Place::SharedDrives => return Err(VfsError::NotFound),
        };
        let folder_file = match place(folder) {
            Place::File { file, .. } => file,
            _ => return Err(VfsError::NotFound),
        };
        let fields = format!("{ITEM_FIELDS},parents");
        let asked = self.file_of(id, &fields);
        let (found, parent_id) = if folder_file == MY_DRIVE {
            let root_query = [("fields", "id")];
            let root = self.request(Method::GET, "/files/root", &root_query, None, None);
            let (found, root) = tokio::try_join!(asked, root)?;
            (found, api::json::<FileParents>(root).await?.id)
        } else {
            (asked.await?, folder_file.to_string())
        };
        if found.drive_id.as_deref() != drive || !found.parents.contains(&parent_id) {
            return Err(VfsError::NotFound);
        }
        Ok(found)
    }

    /// The Drive file to trash for `id` in `folder`, once it passes the checks a trash needs. A folder that holds anything is `NotEmpty`: Drive would trash the folder and everything in it.
    async fn trashable<'a>(&self, id: &'a ItemId, folder: &ItemId) -> Result<&'a str, VfsError> {
        let (file, _) = changeable(id)?;
        let found = self.held(id, folder).await?;
        if found.mime_type != FOLDER_TYPE {
            return Ok(file);
        }
        let children = format!("'{file}' in parents and trashed = false");
        let mut page_token: Option<String> = None;
        // Google can send an empty page that still has a next page, so an empty first page is not an empty folder.
        loop {
            // `held` checked that this is the drive in `id`.
            let mut query = children_query(found.drive_id.as_deref(), &children);
            query.push(("fields", "nextPageToken,files(id)"));
            query.push(("pageSize", "1"));
            if let Some(token) = &page_token {
                query.push(("pageToken", token.as_str()));
            }
            let response = self
                .request(Method::GET, "/files", &query, None, None)
                .await?;
            let page: api::ChildPage = api::json(response).await?;
            if !page.files.is_empty() {
                return Err(VfsError::NotEmpty);
            }
            let Some(next) = page.next_page_token else {
                return Ok(file);
            };
            page_token = Some(next);
        }
    }

    /// Moves one Drive file to the trash. A removed item can be restored, and a shortcut's target is a different file.
    async fn trash(&self, file: &str) -> Result<(), VfsError> {
        let body = serde_json::json!({ "trashed": true });
        let query = [("supportsAllDrives", "true"), ("fields", "id")];
        self.request(
            Method::PATCH,
            &format!("/files/{file}"),
            &query,
            Some(&body),
            None,
        )
        .await?;
        Ok(())
    }

    /// One id from `files.generateIds`. The same id goes out on every try of the create, so a retry cannot make a second item.
    async fn new_file_id(&self) -> Result<String, VfsError> {
        let query = [("count", "1"), ("space", "drive"), ("type", "files")];
        let response = self
            .request(Method::GET, "/files/generateIds", &query, None, None)
            .await?;
        let [id] = api::json::<api::GeneratedIds>(response).await?.ids;
        Ok(id)
    }

    /// The Drive file the core holds as `id`, as Google has it now, with `fields`. `NotFound` when Google has trashed it, or when its answer builds another id. Such an answer is not the item the core holds: a plain id whose file is now a Google Doc, or a link whose shortcut has another target. Its size would not be the size of the bytes `read` returns.
    async fn file_of(&self, id: &ItemId, fields: &str) -> Result<DriveFile, VfsError> {
        let (drive, file) = match place(id) {
            Place::File { drive, file } | Place::Link { drive, file, .. } => (drive, file),
            Place::Top | Place::SharedDrives => return Err(VfsError::NotFound),
        };
        let query = [("fields", fields), ("supportsAllDrives", "true")];
        let response = self
            .request(Method::GET, &format!("/files/{file}"), &query, None, None)
            .await?;
        let found: DriveFile = api::json(response).await?;
        if found.trashed || shown(drive, file, &found).0.id() != *id {
            return Err(VfsError::NotFound);
        }
        Ok(found)
    }

    /// The Drive file the core holds as `id`, once it is checked to still hold the bytes of revision `base`. Bytes based on another revision would overwrite a change the core never saw, so that is `Conflict`. A file with no revision is `NotSupported`, a folder `IsAFolder`, a file the account may only view is `AccessDenied`, and a trashed or gone file `NotFound`.
    pub(crate) async fn replaceable(
        &self,
        id: &ItemId,
        base: &Revision,
    ) -> Result<DriveFile, VfsError> {
        let found = self.file_of(id, ITEM_FIELDS).await?;
        if found.mime_type == FOLDER_TYPE {
            return Err(VfsError::IsAFolder);
        }
        // No upload session opens for a file the account may only view.
        if found.read_only() {
            return Err(VfsError::AccessDenied);
        }
        match &found.head_revision_id {
            None => Err(VfsError::NotSupported),
            Some(revision) if *revision == base.0 => Ok(found),
            Some(_) => Err(VfsError::Conflict),
        }
    }
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
                let response = self
                    .request(Method::GET, &format!("/drives/{drive}"), &query, None, None)
                    .await?;
                return Ok(shared_drive_item(api::json(response).await?));
            }
            Place::File { drive, file } | Place::Link { drive, file, .. } => (drive, file),
        };
        let mut found = self.file_of(id, ITEM_FIELDS).await?;
        // The two folders at the top of a mount have the same names for every account, whatever Google calls My Drive.
        if file == MY_DRIVE {
            found.name = "My Drive".to_string();
        }
        Ok(item_from(drive, file.to_string(), found))
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
                    let response = self
                        .request(Method::GET, "/drives", &query, None, None)
                        .await?;
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
                    let mut query = children_query(drive, &children);
                    // The fields `item` asks for, so a listed item and an asked one are built from the same facts.
                    let fields = format!("nextPageToken,files({ITEM_FIELDS})");
                    query.push(("fields", &fields));
                    query.push(("pageSize", "1000"));
                    if let Some(token) = &page_token {
                        query.push(("pageToken", token.as_str()));
                    }
                    let response = self
                        .request(Method::GET, "/files", &query, None, None)
                        .await?;
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
            .request(
                Method::GET,
                &format!("/files/{file}"),
                &query,
                None,
                Some(&range),
            )
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
        let response = self
            .request(Method::GET, "/about", &query, None, None)
            .await?;
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

    async fn locate(&self, reference: &str) -> Result<Option<Vec<ItemId>>, VfsError> {
        // The reference goes into the request's path, so only the characters of a Drive ID pass.
        if reference.is_empty()
            || !reference
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(VfsError::NotFound);
        }
        let fields = format!("{ITEM_FIELDS},parents");
        let query = [("fields", fields.as_str()), ("supportsAllDrives", "true")];
        let path = format!("/files/{reference}");
        let asked = self.request(Method::GET, &path, &query, None, None);
        // `parents` names the top of My Drive by its real ID, never `root`. It is asked at the same time as the item, as `held` does, so an item in My Drive waits for one answer less.
        let root_query = [("fields", "id")];
        let root = self.request(Method::GET, "/files/root", &root_query, None, None);
        let (found, root) = tokio::join!(asked, root);
        let found: DriveFile = api::json(found?).await?;
        // A mount lists no trashed file.
        if found.trashed {
            return Ok(None);
        }
        let drive = found.drive_id.as_deref();
        // The walk up ends at the top folder of the item's shared drive, or at the top of My Drive. Only the second needs `root`'s answer.
        let top = match drive {
            Some(drive) => drive.to_string(),
            None => api::json::<FileParents>(root?).await?.id,
        };
        let id_of = |file: &str| match drive {
            None if file == top => ItemId(MY_DRIVE.to_string()),
            drive => Place::File { drive, file }.id(),
        };
        let mut chain = vec![if found.id == top {
            id_of(&found.id)
        } else {
            shown(drive, &found.id, &found).0.id()
        }];
        let mut file = found.id.clone();
        let mut parents = found.parents.clone();
        while file != top {
            // An item outside My Drive and the account's shared drives, such as a file shared with the account, has no parent the account can see.
            let Some(parent) = parents.first().cloned() else {
                return Ok(None);
            };
            if parent != top {
                let query = [("fields", "id,parents"), ("supportsAllDrives", "true")];
                match self
                    .request(Method::GET, &format!("/files/{parent}"), &query, None, None)
                    .await
                {
                    Ok(response) => parents = api::json::<FileParents>(response).await?.parents,
                    Err(VfsError::NotFound | VfsError::AccessDenied) => return Ok(None),
                    Err(error) => return Err(error),
                }
            }
            chain.push(id_of(&parent));
            file = parent;
        }
        if drive.is_some() {
            chain.push(ItemId(SHARED_DRIVES.to_string()));
        }
        chain.reverse();
        Ok(Some(chain))
    }

    fn reference(&self, id: &ItemId) -> Option<String> {
        match place(id) {
            // The top and the two folders under it are the mount's own: Google gives My Drive's top another real ID, and decision 0009 stores these by name.
            Place::Top
            | Place::SharedDrives
            | Place::File {
                drive: None,
                file: MY_DRIVE,
            } => None,
            // A shared drive's top folder is `{drive}/{drive}`, so this gives the drive's ID. A link file gives the Drive file it is, not the one its page opens.
            Place::File { file, .. } | Place::Link { file, .. } => Some(file.to_string()),
        }
    }

    async fn thumbnail(&self, id: &ItemId, size: u32) -> Result<Option<Thumbnail>, VfsError> {
        // A link file's bytes are its small web page, so Drive's picture of the item it opens is not a picture of the file's bytes. The top and Shared drives are not Drive files.
        let Place::File { .. } = place(id) else {
            return Ok(None);
        };
        let fields = format!(
            "{ITEM_FIELDS},thumbnailLink,imageMediaMetadata(width,height),videoMediaMetadata(width,height)"
        );
        let found = self.file_of(id, &fields).await?;
        let Some(link) = found.thumbnail_link else {
            return Ok(None);
        };
        // Drive's link ends with `=s220`, the longer side in pixels. Ask for the size the caller wants.
        let url = match link.rsplit_once("=s") {
            Some((rest, digits))
                if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                format!("{rest}=s{size}")
            }
            _ => link,
        };
        let response = match self.request_url(Method::GET, &url, &[], None, None).await {
            Ok(response) => response,
            Err(VfsError::NotFound | VfsError::AccessDenied) => return Ok(None),
            Err(error) => return Err(error),
        };
        let image = response
            .bytes()
            .await
            .map_err(|_| VfsError::Offline)?
            .to_vec();
        let media = found.image_media_metadata.or(found.video_media_metadata);
        let dimensions = media.and_then(|media| Some((media.width?, media.height?)));
        Ok(Some(Thumbnail { image, dimensions }))
    }

    fn can_write(&self) -> bool {
        true
    }

    fn check_new_item(&self, folder: &ItemId, _name: &str) -> Result<(), VfsError> {
        destination(folder).map(|_| ())
    }

    async fn create_folder(&self, parent: &ItemId, name: &str) -> Result<Item, VfsError> {
        let (drive, parent_file) = destination(parent)?;
        let id = self.new_file_id().await?;
        let body = serde_json::json!({
            "id": &id,
            "name": name,
            "mimeType": FOLDER_TYPE,
            "parents": [parent_file],
        });
        let query = [("supportsAllDrives", "true"), ("fields", ITEM_FIELDS)];
        // The same id goes out on every try, so a 409 means the folder was made and the answer was lost. Nobody else knows that id.
        match self
            .request(Method::POST, "/files", &query, Some(&body), None)
            .await
        {
            // A folder's children are in the folder's own drive, as in a listing.
            Ok(response) => Ok(item_from(drive, id, api::json(response).await?)),
            Err(VfsError::Conflict) => self.item(&Place::File { drive, file: &id }.id()).await,
            Err(error) => Err(error),
        }
    }

    async fn remove(&self, id: &ItemId, parent: &ItemId) -> Result<(), VfsError> {
        let file = self.trashable(id, parent).await?;
        self.trash(file).await
    }

    async fn rename(
        &self,
        id: &ItemId,
        from: &ItemId,
        to: &ItemId,
        name: &str,
        replace: Option<&ItemId>,
    ) -> Result<Item, VfsError> {
        let (file, opens) = changeable(id)?;
        let (to_drive, to_file) = destination(to)?;
        let Place::File {
            drive: from_drive,
            file: from_file,
        } = place(from)
        else {
            return Err(VfsError::NotFound);
        };
        let found = self.held(id, from).await?;
        let drive_name = match link_ending(&found.mime_type) {
            // Drive stores a link file's name without the ending the mount adds. A name without that ending would not show back as the name asked for, and a duplicate's `Notes.document (2).html` is such a name.
            Some(ending) => name.strip_suffix(&ending).ok_or(VfsError::NotSupported)?,
            None => name,
        };
        if found.mime_type == FOLDER_TYPE && from != to {
            if from_drive != to_drive {
                // Google refuses to move a folder into a shared drive, and the items inside it would keep ids that still name the old drive.
                return Err(VfsError::NotSupported);
            }
            // Google answers a move into the folder's own subtree with a 400 that looks like any other 400, so the folders above `to` are walked up to `from` or to the top.
            let mut above = to_file.to_string();
            while above != from_file {
                if above == file {
                    return Err(VfsError::NotSupported);
                }
                let query = [("fields", "id,parents"), ("supportsAllDrives", "true")];
                let response = self
                    .request(Method::GET, &format!("/files/{above}"), &query, None, None)
                    .await?;
                let Some(parent) = api::json::<FileParents>(response)
                    .await?
                    .parents
                    .into_iter()
                    .next()
                else {
                    break;
                };
                above = parent;
            }
        }
        let replaced = match replace {
            Some(replace) => Some(self.trashable(replace, to).await?),
            None => None,
        };
        let mut query = vec![("supportsAllDrives", "true"), ("fields", ITEM_FIELDS)];
        if from != to {
            query.push(("addParents", to_file));
            query.push(("removeParents", from_file));
        }
        let body = serde_json::json!({ "name": drive_name });
        let sent: Result<DriveFile, VfsError> = async {
            let path = format!("/files/{file}");
            let response = self
                .request(Method::PATCH, &path, &query, Some(&body), None)
                .await?;
            api::json(response).await
        }
        .await;
        // A change Google applied can still come back as a failure: a server error, a dropped connection, or a resent move Google refuses because the item already has the new parent. The change counts when the item is already in `to` under the name that was sent.
        let answer = match sent {
            Ok(answer) => answer,
            Err(error @ (VfsError::Other(_) | VfsError::Offline)) => {
                let moved = match opens {
                    Some(opens) => Place::Link {
                        drive: to_drive,
                        file,
                        opens,
                    },
                    None => Place::File {
                        drive: to_drive,
                        file,
                    },
                };
                match self.held(&moved.id(), to).await {
                    Ok(found) if found.name == drive_name => found,
                    _ => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
        if let Some(replace_file) = replaced {
            // The move is already done. A failed trash leaves both items, so the name is never left with none.
            self.trash(replace_file).await?;
        }
        // A folder's children are in the folder's own drive, as in a listing.
        Ok(item_from(to_drive, file.to_string(), answer))
    }

    async fn create_file(
        &self,
        parent: &ItemId,
        name: &str,
        from: &Path,
        modified: SystemTime,
        progress: &Progress,
    ) -> Result<Item, VfsError> {
        let (drive, parent_file) = destination(parent)?;
        let id = self.new_file_id().await?;
        let upload = Upload::Create {
            id: &id,
            name,
            parent: parent_file,
        };
        let uploaded = self.upload(upload, from, modified, progress).await;
        // Drive allows two files with one name, so a taken name is not a conflict. A 409 means this call's pre-made id already landed.
        match uploaded {
            Ok(file) => Ok(item_from(drive, id, file)),
            Err(VfsError::Conflict) => self.item(&Place::File { drive, file: &id }.id()).await,
            Err(error) => Err(error),
        }
    }

    async fn replace_bytes(
        &self,
        id: &ItemId,
        from: &Path,
        base: &Revision,
        modified: SystemTime,
        progress: &Progress,
    ) -> Result<Item, VfsError> {
        let (drive, file) = match place(id) {
            // A link file has no bytes on Drive to replace.
            Place::Link { .. } => return Err(VfsError::NotSupported),
            Place::Top | Place::SharedDrives => return Err(VfsError::IsAFolder),
            Place::File { drive, file } => (drive, file),
        };
        // A file that already changed costs no upload.
        let found = self.replaceable(id, base).await?;
        let upload = Upload::Replace {
            file,
            id,
            base,
            mime_type: &found.mime_type,
        };
        let answer = self.upload(upload, from, modified, progress).await?;
        Ok(item_from(drive, file.to_string(), answer))
    }
}
