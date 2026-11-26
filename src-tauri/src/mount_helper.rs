//! The mount commands, and the request that stops the mount helper when the user quits.

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::path::{Component, Path};

use marlin_mount_client::protocol::{
    methods, AccountParams, DeadMount, Identified, IdentifyParams, LocateParams, MountParams,
    MountState, MountStatus, UnsentFiles, UnsentParams,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tauri::AppHandle;

/// What the app knows about the helper's mounts: the latest status of each, from the helper's answers and its `mountChanged` notifications.
static MOUNTS: std::sync::Mutex<MountTable> = std::sync::Mutex::new(MountTable::new());

/// The latest status of each mount, one per provider and account.
///
/// The helper writes an answer and a notification from separate tasks, and a command reads its answer while the client's reader goes on with notifications. So an answer can land after a notification that is newer than it. Notifications arrive in the order the changes happened, so an answer changes a mount only when no notification about that mount arrived after the request was sent. A mount the helper no longer has stays as `unmounted`, so an older answer cannot bring it back.
struct MountTable {
    /// How many notifications have arrived. A request takes this number before it is sent.
    notices: u64,
    mounts: BTreeMap<MountKey, Known>,
}

/// What names one mount: its provider and its account.
type MountKey = (String, String);

/// One mount's latest status, and the number of the last notification about it, 0 when none came.
struct Known {
    status: MountStatus,
    notified: u64,
}

impl MountTable {
    const fn new() -> Self {
        Self {
            notices: 0,
            mounts: BTreeMap::new(),
        }
    }

    /// The number a request takes before it is sent, for [`Self::answered`].
    fn asked(&self) -> u64 {
        self.notices
    }

    /// A `mountChanged`. It is always the newest word on its mount.
    #[cfg(unix)]
    fn changed(&mut self, status: &MountStatus) {
        self.notices += 1;
        let key = (status.provider.clone(), status.account.clone());
        let known = Known {
            status: status.clone(),
            notified: self.notices,
        };
        self.mounts.insert(key, known);
    }

    /// An answer about one mount, to a request that took `asked`: its status, or `None` when the helper has no such mount, which marks a known mount `unmounted`. Ignored when a notification about the mount came after `asked`.
    fn answered(&mut self, asked: u64, key: MountKey, status: Option<&MountStatus>) {
        match (self.mounts.entry(key), status) {
            (Entry::Occupied(known), _) if known.get().notified > asked => {}
            (Entry::Occupied(mut known), Some(status)) => known.get_mut().status = status.clone(),
            (Entry::Occupied(mut known), None) => {
                known.get_mut().status.state = MountState::Unmounted;
            }
            (Entry::Vacant(slot), Some(status)) => {
                slot.insert(Known {
                    status: status.clone(),
                    notified: 0,
                });
            }
            (Entry::Vacant(_), None) => {}
        }
    }

    /// A `list` answer, to a request that took `asked`: every mount the helper has. Each listed mount is an answer about it, and each known mount the list leaves out is an answer of `None`.
    fn listed(&mut self, asked: u64, statuses: &[MountStatus]) {
        let listed: BTreeMap<MountKey, &MountStatus> = statuses
            .iter()
            .map(|status| ((status.provider.clone(), status.account.clone()), status))
            .collect();
        let left_out: Vec<MountKey> = self
            .mounts
            .keys()
            .filter(|key| !listed.contains_key(*key))
            .cloned()
            .collect();
        for key in left_out {
            self.answered(asked, key, None);
        }
        for (key, status) in listed {
            self.answered(asked, key, Some(status));
        }
    }

    /// The connection to the helper ended, so the app knows nothing about the mounts.
    #[cfg(unix)]
    fn clear(&mut self) {
        self.mounts.clear();
    }

    /// The mount that holds `path`, and the path inside it with `/` between names. Only a mount that is up holds a path. The path is compared as given, with no disk access: a path with a `..` in it is in no mount, because it can lead out of one. A link in the path is not followed.
    fn holding(&self, path: &Path) -> Option<MountedPath> {
        if path.components().any(|name| name == Component::ParentDir) {
            return None;
        }
        // Mount points are folders side by side in the mount folder, so at most one holds a path.
        self.mounts
            .values()
            .filter(|known| known.status.state.is_up())
            .find_map(|known| {
                // Whole names are compared, so `gdrive-ana` never holds `gdrive-ana2/x`.
                let inside = path.strip_prefix(&known.status.mount_point).ok()?;
                // Names in a mount come from the remote, as UTF-8.
                let names: Option<Vec<&str>> = inside
                    .components()
                    .map(|name| name.as_os_str().to_str())
                    .collect();
                Some(MountedPath {
                    mount: known.status.clone(),
                    path: names?.join("/"),
                })
            })
    }
}

/// A path in a mount: the mount, and the path inside it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MountedPath {
    pub(crate) mount: MountStatus,
    path: String,
}

/// Why a mount command failed. `code` is the helper's own error code, and is missing when the failure was not an answer from the helper.
#[derive(Debug, Serialize)]
pub struct MountCommandError {
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<i32>,
    message: String,
}

impl MountCommandError {
    fn message(message: impl ToString) -> Self {
        Self {
            code: None,
            message: message.to_string(),
        }
    }
}

/// The app's one connection to the helper.
#[cfg(unix)]
static CLIENT: tokio::sync::Mutex<Option<std::sync::Arc<marlin_mount_client::Client>>> =
    tokio::sync::Mutex::const_new(None);

/// Sends one request to the helper. Connects first when there is no open connection, which starts the helper when none is running.
#[cfg(unix)]
async fn call<T: DeserializeOwned>(
    app: &AppHandle,
    method: &str,
    params: impl Serialize,
) -> Result<T, MountCommandError> {
    use std::sync::Arc;

    use marlin_mount_client::{Client, ClientError, Event, Start};
    use tauri::{Emitter, Manager};

    let client = {
        // Held across the connect, so two commands at once start one helper. Released before the request, which can take long.
        let mut slot = CLIENT.lock().await;
        match slot.as_ref().filter(|client| !client.is_closed()) {
            Some(client) => Arc::clone(client),
            None => {
                let start = Start {
                    // The one place a helper is looked for: beside the app binary. That covers a dev build and a bundle. `PATH` is never searched.
                    binary: std::env::current_exe()
                        .map_err(MountCommandError::message)?
                        .with_file_name("marlin-mount"),
                    socket: marlin_mount_client::default_socket().ok_or_else(|| {
                        MountCommandError::message(
                            "Could not find a folder for the mount helper's socket.",
                        )
                    })?,
                    root: None,
                    log: app
                        .path()
                        .app_log_dir()
                        .map_err(MountCommandError::message)?
                        .join("marlin-mount.log"),
                };
                let app = app.clone();
                let client = Client::connect(&start, move |event| match event {
                    Event::Changed(status) => {
                        MOUNTS.lock().unwrap().changed(&status);
                        let _ = app.emit("mount-changed", status);
                    }
                    Event::Unsent(files) => {
                        let _ = app.emit("unsent-changed", files);
                    }
                    Event::Closed => {
                        MOUNTS.lock().unwrap().clear();
                        let _ = app.emit("mount-helper-closed", ());
                    }
                })
                .await
                .map_err(MountCommandError::message)?;
                // So the table holds every mount from the moment the app connects, whatever the frontend asks first.
                let asked = MOUNTS.lock().unwrap().asked();
                let statuses: Vec<MountStatus> = client
                    .call(methods::LIST, ())
                    .await
                    .map_err(MountCommandError::message)?;
                MOUNTS.lock().unwrap().listed(asked, &statuses);
                let client = Arc::new(client);
                *slot = Some(Arc::clone(&client));
                client
            }
        }
    };
    client
        .call(method, params)
        .await
        .map_err(|error| match error {
            ClientError::Rpc(error) => MountCommandError {
                code: Some(error.code),
                message: error.message,
            },
            error => MountCommandError::message(error),
        })
}

#[cfg(not(unix))]
async fn call<T: DeserializeOwned>(
    _app: &AppHandle,
    _method: &str,
    _params: impl Serialize,
) -> Result<T, MountCommandError> {
    Err(MountCommandError::message(
        "Marlin cannot mount on this OS yet.",
    ))
}

#[tauri::command]
pub async fn list_mounts(app: AppHandle) -> Result<Vec<MountStatus>, MountCommandError> {
    let asked = MOUNTS.lock().unwrap().asked();
    let statuses: Vec<MountStatus> = call(&app, methods::LIST, ()).await?;
    MOUNTS.lock().unwrap().listed(asked, &statuses);
    Ok(statuses)
}

#[tauri::command]
pub async fn get_mount_status(
    app: AppHandle,
    provider: String,
    account: String,
) -> Result<Option<MountStatus>, MountCommandError> {
    let asked = MOUNTS.lock().unwrap().asked();
    let params = AccountParams {
        provider: provider.clone(),
        account: account.clone(),
    };
    let status: Option<MountStatus> = call(&app, methods::STATUS, params).await?;
    MOUNTS
        .lock()
        .unwrap()
        .answered(asked, (provider, account), status.as_ref());
    Ok(status)
}

#[tauri::command]
pub async fn mount_account(
    app: AppHandle,
    provider: String,
    account: String,
    name: String,
) -> Result<MountStatus, MountCommandError> {
    let asked = MOUNTS.lock().unwrap().asked();
    let params = MountParams {
        provider: provider.clone(),
        account: account.clone(),
        name,
    };
    let status = call(&app, methods::MOUNT, params).await?;
    MOUNTS
        .lock()
        .unwrap()
        .answered(asked, (provider, account), Some(&status));
    Ok(status)
}

#[tauri::command]
pub async fn unmount_account(
    app: AppHandle,
    provider: String,
    account: String,
) -> Result<(), MountCommandError> {
    call(&app, methods::UNMOUNT, AccountParams { provider, account }).await
}

#[tauri::command]
pub async fn list_dead_mounts(app: AppHandle) -> Result<Vec<DeadMount>, MountCommandError> {
    call(&app, methods::DEAD_MOUNTS, ()).await
}

#[tauri::command]
pub async fn list_unsent_files(
    app: AppHandle,
    provider: String,
    account: String,
) -> Result<Option<UnsentFiles>, MountCommandError> {
    call(&app, methods::UNSENT, AccountParams { provider, account }).await
}

#[tauri::command]
pub async fn retry_unsent_file(
    app: AppHandle,
    provider: String,
    account: String,
    mount_id: String,
    node: u64,
) -> Result<(), MountCommandError> {
    let params = UnsentParams {
        provider,
        account,
        mount_id,
        node,
    };
    call(&app, methods::RETRY_UNSENT, params).await
}

#[tauri::command]
pub async fn discard_unsent_file(
    app: AppHandle,
    provider: String,
    account: String,
    mount_id: String,
    node: u64,
) -> Result<(), MountCommandError> {
    let params = UnsentParams {
        provider,
        account,
        mount_id,
        node,
    };
    call(&app, methods::DISCARD_UNSENT, params).await
}

#[tauri::command]
pub async fn locate_in_mount(
    app: AppHandle,
    provider: String,
    account: String,
    reference: String,
) -> Result<Option<String>, MountCommandError> {
    let params = LocateParams {
        provider,
        account,
        reference,
    };
    call(&app, methods::LOCATE, params).await
}

#[tauri::command]
pub async fn identify_in_mount(
    app: AppHandle,
    provider: String,
    account: String,
    path: String,
) -> Result<Identified, MountCommandError> {
    let params = IdentifyParams {
        provider,
        account,
        path,
    };
    call(&app, methods::IDENTIFY, params).await
}

#[tauri::command]
pub async fn refresh_mount(
    app: AppHandle,
    provider: String,
    account: String,
) -> Result<(), MountCommandError> {
    call(&app, methods::REFRESH, AccountParams { provider, account }).await
}

/// A picture of a file in a mount that the mount's remote made. It lives in a private temp folder of the app's own, which goes when this is dropped.
#[cfg(unix)]
pub(crate) struct RemotePicture {
    /// Removes the folder and the picture when dropped.
    _folder: tempfile::TempDir,
    pub(crate) file: std::path::PathBuf,
    /// The file's own width and height, when the remote knows them.
    pub(crate) dimensions: Option<marlin_mount_client::protocol::Dimensions>,
}

/// Google Drive's own picture of the file at `path`, when `path` is in a Google Drive mount that is up and Drive has the file's current bytes. `None` sends the caller to the file itself. It reads the mount table and uses the open connection only, so it never starts a helper and never waits for one to connect.
#[cfg(unix)]
pub(crate) async fn remote_thumbnail(path: &Path, size: u32) -> Option<RemotePicture> {
    use std::os::unix::fs::PermissionsExt;

    use marlin_mount_client::protocol::{ThumbnailParams, Thumbnailed};

    let mounted = MOUNTS.lock().unwrap().holding(path)?;
    // Only the Drive provider makes pictures. Asking another mount would walk its folders for nothing.
    if mounted.mount.provider != "gdrive" {
        return None;
    }
    // The table holds a mount only while a connection is open, or at the end of a connect, which keeps this lock until it has listed the mounts. So this never waits for a helper to start.
    let client = {
        let slot = CLIENT.lock().await;
        std::sync::Arc::clone(slot.as_ref().filter(|client| !client.is_closed())?)
    };
    // The folder is the app's own, and only this user may list it. A cancelled thumbnail drops it before the helper answers, and a helper that writes later finds no folder and leaves nothing behind.
    let folder = tempfile::Builder::new()
        .prefix("marlin-thumbnail-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .ok()?;
    let file = folder.path().join("picture");
    let params = ThumbnailParams {
        provider: mounted.mount.provider,
        account: mounted.mount.account,
        path: mounted.path,
        size,
        file: file.to_str()?.to_owned(),
    };
    match client.call(methods::THUMBNAIL, params).await {
        Ok(Thumbnailed::Written { dimensions }) => Some(RemotePicture {
            _folder: folder,
            file,
            dimensions,
        }),
        Ok(answer) => {
            log::debug!("The mount helper gave no picture of the file: {answer:?}");
            None
        }
        Err(error) => {
            log::debug!("The mount helper gave no picture of the file: {error}");
            None
        }
    }
}

/// Reads the app's mount table only, so it never starts a helper. The table is empty until the first command that sends a request, so call `list_mounts` first at launch.
#[tauri::command]
pub fn find_mount(path: String) -> Option<MountedPath> {
    MOUNTS.lock().unwrap().holding(Path::new(&path))
}

/// Asks the helper to unmount everything and exit. Never starts one.
///
/// It returns once the request is sent. The helper then finishes its uploads and unmounts even when the app has gone, so a quit never waits for an upload.
pub fn stop_at_quit() {
    #[cfg(unix)]
    if let Some(socket) = marlin_mount_client::default_socket() {
        stop(&socket);
    }
}

/// `stop_at_quit` for the helper on `socket`. Apart so that a test can name a socket of its own.
#[cfg(unix)]
fn stop(socket: &std::path::Path) {
    // The timer is made inside `block_on`: it needs the runtime, and this runs on the main thread, outside it.
    let sent = tauri::async_runtime::block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            marlin_mount_client::ask_to_stop(socket),
        )
        .await
    });
    match sent {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => log::warn!("The mount helper could not be stopped: {error}"),
        Err(_) => log::warn!("The mount helper did not take the request to stop in time."),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::path::Path;

    use marlin_mount_client::protocol::{MountState, MountStatus};

    use super::MountTable;

    #[test]
    fn stopping_at_quit_works_outside_the_runtime_and_with_no_helper() {
        let dir = tempfile::tempdir().unwrap();
        super::stop(&dir.path().join("mount.sock"));
    }

    fn status(account: &str, mount_point: &str, state: MountState) -> MountStatus {
        MountStatus {
            provider: "gdrive".into(),
            account: account.into(),
            name: Path::new(mount_point)
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string(),
            mount_point: mount_point.into(),
            state,
            error: None,
            read_only: None,
        }
    }

    #[test]
    fn holding_finds_a_path_inside_a_mount_that_is_up() {
        let mut table = MountTable::new();
        table.changed(&status("ana", "/m/gdrive-ana", MountState::Mounted));
        let deep = table
            .holding(Path::new("/m/gdrive-ana/My Drive/Projects"))
            .unwrap();
        assert_eq!(deep.path, "My Drive/Projects");
        assert_eq!(deep.mount.account, "ana");
        assert_eq!(table.holding(Path::new("/m/gdrive-ana")).unwrap().path, "");
        assert!(table.holding(Path::new("/m/gdrive-ana2/x")).is_none());
        assert!(table.holding(Path::new("/m/gdrive-ana/../local")).is_none());
        assert_eq!(
            table.holding(Path::new("/m/gdrive-ana/./a/")).unwrap().path,
            "a"
        );
    }

    #[test]
    fn holding_skips_a_mount_that_is_not_up() {
        let mut table = MountTable::new();
        table.changed(&status("start", "/m/gdrive-start", MountState::Starting));
        table.changed(&status("fail", "/m/gdrive-fail", MountState::Failed));
        table.changed(&status("gone", "/m/gdrive-gone", MountState::Unmounted));
        table.changed(&status("off", "/m/gdrive-off", MountState::Offline));
        assert!(table.holding(Path::new("/m/gdrive-start/a")).is_none());
        assert!(table.holding(Path::new("/m/gdrive-fail/a")).is_none());
        assert!(table.holding(Path::new("/m/gdrive-gone/a")).is_none());
        let held = table.holding(Path::new("/m/gdrive-off/a")).unwrap();
        assert_eq!(held.path, "a");
        assert_eq!(held.mount.state, MountState::Offline);
    }

    #[test]
    fn a_list_asked_before_an_unmount_does_not_bring_the_mount_back() {
        let mut table = MountTable::new();
        let up = status("ana", "/m/gdrive-ana", MountState::Mounted);
        table.changed(&up);
        let asked = table.asked();
        let mut gone = up.clone();
        gone.state = MountState::Unmounted;
        table.changed(&gone);
        table.listed(asked, &[up]);
        assert!(table.holding(Path::new("/m/gdrive-ana/a")).is_none());
    }

    #[test]
    fn a_status_asked_before_a_newer_change_does_not_replace_it() {
        let mut table = MountTable::new();
        table.changed(&status("ana", "/m/gdrive-ana", MountState::Starting));
        let asked = table.asked();
        table.changed(&status("ana", "/m/gdrive-ana", MountState::Mounted));
        table.answered(
            asked,
            ("gdrive".into(), "ana".into()),
            Some(&status("ana", "/m/gdrive-ana", MountState::Failed)),
        );
        assert_eq!(
            table
                .holding(Path::new("/m/gdrive-ana"))
                .unwrap()
                .mount
                .state,
            MountState::Mounted
        );
        let asked = table.asked();
        table.answered(
            asked,
            ("gdrive".into(), "ana".into()),
            Some(&status("ana", "/m/gdrive-ana", MountState::Offline)),
        );
        assert_eq!(
            table
                .holding(Path::new("/m/gdrive-ana"))
                .unwrap()
                .mount
                .state,
            MountState::Offline
        );
    }

    #[test]
    fn a_list_marks_a_missing_mount_gone_unless_it_started_later() {
        let mut table = MountTable::new();
        table.answered(
            0,
            ("gdrive".into(), "ana".into()),
            Some(&status("ana", "/m/gdrive-ana", MountState::Mounted)),
        );
        let asked = table.asked();
        table.changed(&status("ben", "/m/gdrive-ben", MountState::Mounted));
        table.listed(asked, &[]);
        assert!(table.holding(Path::new("/m/gdrive-ana/a")).is_none());
        assert_eq!(
            table.holding(Path::new("/m/gdrive-ben/a")).unwrap().path,
            "a"
        );
    }

    #[test]
    fn clear_empties_the_table() {
        let mut table = MountTable::new();
        table.changed(&status("ana", "/m/gdrive-ana", MountState::Mounted));
        table.clear();
        assert!(table.holding(Path::new("/m/gdrive-ana")).is_none());
        table.changed(&status("ana", "/m/gdrive-ana", MountState::Degraded));
        assert_eq!(
            table
                .holding(Path::new("/m/gdrive-ana"))
                .unwrap()
                .mount
                .state,
            MountState::Degraded
        );
    }

    fn write_png(path: &Path, width: u32, height: u32) {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
        image
            .save_with_format(path, image::ImageFormat::Png)
            .unwrap();
    }

    fn request(path: &Path) -> crate::thumbnails::ThumbnailRequest {
        use crate::thumbnails::{
            ThumbnailFormat, ThumbnailPriority, ThumbnailQuality, ThumbnailRequest,
        };
        ThumbnailRequest {
            id: "t".into(),
            path: path.to_str().unwrap().to_string(),
            size: 64,
            quality: ThumbnailQuality::Low,
            priority: ThumbnailPriority::High,
            format: ThumbnailFormat::PNG,
            accent: None,
        }
    }

    /// Clears the mount table and the helper connection this test is the only one to touch.
    struct Reset;

    impl Drop for Reset {
        fn drop(&mut self) {
            if let Ok(mut client) = super::CLIENT.try_lock() {
                *client = None;
            }
            super::MOUNTS.lock().unwrap().clear();
        }
    }

    #[tokio::test]
    async fn a_file_in_a_drive_mount_gets_drives_picture_or_its_own_thumbnail() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::{Arc, Mutex};

        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        use tokio::net::UnixListener;

        use crate::thumbnails::generators::{Source, ThumbnailGenerator};
        use crate::thumbnails::{ThumbnailFormat, ThumbnailRequest};

        let within = std::time::Duration::from_secs(10);

        let _reset = Reset;
        let dir = tempfile::tempdir().unwrap();
        let mnt = dir.path().join("mnt");
        let drive = mnt.join("My Drive");
        std::fs::create_dir_all(&drive).unwrap();
        // `a.png` has no file, so only Drive's picture can make its thumbnail.
        for name in ["b.png", "c.png", "d.png", "e.png"] {
            write_png(&drive.join(name), 20, 10);
        }
        let outside = dir.path().join("out.png");
        write_png(&outside, 20, 10);
        let sftp = dir.path().join("sftp");
        std::fs::create_dir_all(&sftp).unwrap();
        write_png(&sftp.join("x.png"), 20, 10);

        for (provider, mount_point) in [("gdrive", &mnt), ("sftp", &sftp)] {
            super::MOUNTS.lock().unwrap().changed(&MountStatus {
                provider: provider.into(),
                account: "ana@example.com".into(),
                name: format!("{provider}-ana"),
                mount_point: mount_point.to_str().unwrap().to_string(),
                state: MountState::Mounted,
                error: None,
                read_only: None,
            });
        }

        let socket = dir.path().join("h.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        // Every `thumbnail` request's params, for the checks at the end.
        let asked = Arc::new(Mutex::new(Vec::new()));
        // For `e.png` the fake hands the test its file, waits for the go, then tries to make the file.
        let (held_tx, held_rx) = tokio::sync::oneshot::channel::<std::path::PathBuf>();
        let (go_tx, go_rx) = tokio::sync::oneshot::channel::<()>();
        let (made_tx, made_rx) = tokio::sync::oneshot::channel::<bool>();
        let written = serde_json::json!({"thumbnail": "written", "dimensions": {"width": 4000, "height": 2000}});
        let fake_asked = Arc::clone(&asked);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut writer) = stream.into_split();
            let mut lines = tokio::io::BufReader::new(read).lines();
            let mut held = Some((held_tx, go_rx, made_tx));
            while let Ok(Some(line)) = lines.next_line().await {
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let params = &request["params"];
                let file = params["file"].as_str().unwrap_or_default().to_string();
                let result = if request["method"] == "hello" {
                    serde_json::json!({
                        "protocol": marlin_mount_client::protocol::PROTOCOL_VERSION,
                        "version": "fake",
                    })
                } else {
                    fake_asked.lock().unwrap().push(request.clone());
                    match params["path"].as_str().unwrap_or_default() {
                        "My Drive/a.png" | "My Drive/f.jpg" => {
                            write_png(Path::new(&file), 40, 20);
                            written.clone()
                        }
                        "My Drive/c.png" => serde_json::json!({"thumbnail": "not-eligible"}),
                        "My Drive/d.png" => {
                            std::fs::write(&file, b"not an image").unwrap();
                            written.clone()
                        }
                        "My Drive/e.png" => {
                            let (held_tx, go_rx, made_tx) = held.take().unwrap();
                            held_tx.send(file.clone().into()).unwrap();
                            go_rx.await.unwrap();
                            made_tx.send(std::fs::File::create(&file).is_ok()).unwrap();
                            serde_json::json!({"thumbnail": "no-picture"})
                        }
                        _ => serde_json::json!({"thumbnail": "no-picture"}),
                    }
                };
                let answer =
                    serde_json::json!({"jsonrpc": "2.0", "id": request["id"], "result": result});
                writer
                    .write_all(format!("{answer}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });

        let client = marlin_mount_client::Client::connect(
            &marlin_mount_client::Start {
                binary: dir.path().join("none"),
                socket,
                root: None,
                log: dir.path().join("log"),
            },
            |_| {},
        )
        .await
        .unwrap();
        *super::CLIENT.lock().await = Some(Arc::new(client));

        let a = drive.join("a.png");
        // A command that holds the connection for a moment only delays Drive's picture.
        let held = super::CLIENT.lock().await;
        let asking = tokio::spawn(ThumbnailGenerator::prepare(request(&a)));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        drop(held);
        let prepared = tokio::time::timeout(within, asking)
            .await
            .expect("the a.png thumbnail never finished")
            .unwrap()
            .unwrap();
        assert_eq!(prepared.request.path, a.to_str().unwrap());
        let Source::RemotePicture(picture) = &prepared.source else {
            panic!("a.png got no picture from the helper");
        };
        let folder = picture.file.parent().unwrap().to_path_buf();
        let mode = std::fs::metadata(&folder).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        let made = ThumbnailGenerator::generate(&prepared).unwrap();
        assert_eq!(
            (made.image_width, made.image_height),
            (Some(4000), Some(2000))
        );
        drop(prepared);
        assert!(!folder.exists());

        // A WebP request takes the format of the file's own extension: Drive's PNG picture of a JPEG photo stays JPEG.
        let f = drive.join("f.jpg");
        let webp = ThumbnailRequest {
            format: ThumbnailFormat::WebP,
            ..request(&f)
        };
        let prepared = ThumbnailGenerator::prepare(webp).await.unwrap();
        let made = ThumbnailGenerator::generate(&prepared).unwrap();
        assert!(made.data_url.starts_with("data:image/jpeg"));

        // No picture, unsent bytes, a picture that does not decode, a mount that is not Google Drive's, and a path in no mount: the file makes the thumbnail.
        let sftp_file = sftp.join("x.png");
        for path in [
            drive.join("b.png"),
            drive.join("c.png"),
            drive.join("d.png"),
            sftp_file.clone(),
            outside.clone(),
        ] {
            let prepared = ThumbnailGenerator::prepare(request(&path)).await.unwrap();
            let made = ThumbnailGenerator::generate(&prepared).unwrap();
            assert_eq!(
                (made.image_width, made.image_height),
                (Some(20), Some(10)),
                "{path:?}"
            );
        }

        let e = drive.join("e.png");
        let task = tokio::spawn(async move { ThumbnailGenerator::prepare(request(&e)).await });
        let picture = tokio::time::timeout(within, held_rx)
            .await
            .expect("prepare never asked the helper about e.png")
            .unwrap();
        task.abort();
        let _ = task.await;
        go_tx.send(()).unwrap();
        let made = tokio::time::timeout(within, made_rx)
            .await
            .expect("the fake helper never tried to make e.png's picture")
            .unwrap();
        assert!(!made);
        assert!(!picture.parent().unwrap().exists());

        let asked = asked.lock().unwrap();
        let paths: Vec<&str> = asked
            .iter()
            .map(|request| request["params"]["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            paths,
            [
                "My Drive/a.png",
                "My Drive/f.jpg",
                "My Drive/b.png",
                "My Drive/c.png",
                "My Drive/d.png",
                "My Drive/e.png"
            ]
        );
        for request in asked.iter() {
            assert_eq!(request["method"], "thumbnail");
            assert_eq!(request["params"]["provider"], "gdrive");
            assert_eq!(request["params"]["account"], "ana@example.com");
            assert_eq!(request["params"]["size"], 64);
            assert!(Path::new(request["params"]["file"].as_str().unwrap()).is_absolute());
        }
    }
}
