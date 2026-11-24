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
    mount: MountStatus,
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
}
