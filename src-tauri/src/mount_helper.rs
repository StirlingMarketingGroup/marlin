//! The mount commands, and the request that stops the mount helper when the user quits.

use marlin_mount_client::protocol::{methods, AccountParams, DeadMount, MountParams, MountStatus};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tauri::AppHandle;

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
                        let _ = app.emit("mount-changed", status);
                    }
                    Event::Closed => {
                        let _ = app.emit("mount-helper-closed", ());
                    }
                })
                .await
                .map_err(MountCommandError::message)?;
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
    call(&app, methods::LIST, ()).await
}

#[tauri::command]
pub async fn get_mount_status(
    app: AppHandle,
    provider: String,
    account: String,
) -> Result<Option<MountStatus>, MountCommandError> {
    call(&app, methods::STATUS, AccountParams { provider, account }).await
}

#[tauri::command]
pub async fn mount_account(
    app: AppHandle,
    provider: String,
    account: String,
    name: String,
) -> Result<MountStatus, MountCommandError> {
    let params = MountParams {
        provider,
        account,
        name,
    };
    call(&app, methods::MOUNT, params).await
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

/// Asks the helper to unmount everything and exit. Never starts one.
///
/// The wait gives the request time to reach the helper, which answers `hello` first. Once the helper has the request, it finishes the shutdown even when the app has gone.
pub fn stop_at_quit() {
    #[cfg(unix)]
    if let Some(socket) = marlin_mount_client::default_socket() {
        stop(&socket);
    }
}

/// `stop_at_quit` for the helper on `socket`. Apart so that a test can name a socket of its own.
#[cfg(unix)]
fn stop(socket: &std::path::Path) {
    use marlin_mount_client::ClientError;

    // The timer is made inside `block_on`: it needs the runtime, and this runs on the main thread, outside it.
    let finished = tauri::async_runtime::block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            marlin_mount_client::shutdown(socket),
        )
        .await
    });
    match finished {
        Ok(Ok(_)) => {}
        Ok(Err(ClientError::Rpc(error))) => log::warn!(
            "The mount helper could not unmount: {}. It keeps serving that mount, and the next start of Marlin finds it.",
            error.message
        ),
        Ok(Err(error)) => log::warn!("The mount helper could not be stopped: {error}"),
        Err(_) => log::warn!("The mount helper did not answer the request to stop in time."),
    }
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn stopping_at_quit_works_outside_the_runtime_and_with_no_helper() {
        let dir = tempfile::tempdir().unwrap();
        super::stop(&dir.path().join("mount.sock"));
    }
}
