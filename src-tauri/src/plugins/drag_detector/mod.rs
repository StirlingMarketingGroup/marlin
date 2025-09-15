use serde::{Deserialize, Serialize};
use tauri::{Emitter, Runtime, Window};

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
mod linux;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropLocation {
    pub x: f64,
    pub y: f64,
    pub target_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DragDropEvent {
    pub paths: Vec<String>,
    pub location: DropLocation,
    pub event_type: DragEventType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DragEventType {
    DragEnter,
    DragOver,
    DragLeave,
    Drop,
}

#[tauri::command]
pub async fn enable_drag_detection<R: Runtime>(window: Window<R>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::setup_drag_handlers(&window)?;
    }

    let _ = window; // suppress unused warning on other platforms
    Ok(())
}

#[tauri::command]
pub async fn set_drop_zone(
    zone_id: String,
    enabled: bool,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::set_drop_zone(&zone_id, enabled);
    }

    Ok(())
}

#[tauri::command]
pub async fn simulate_drop<R: Runtime>(window: Window<R>, paths: Vec<String>, target_id: Option<String>) -> Result<(), String> {
    let event = DragDropEvent {
        paths,
        location: DropLocation {
            x: 0.0,
            y: 0.0,
            target_id,
        },
        event_type: DragEventType::Drop,
    };

    window
        .emit("drag-drop-event", event)
        .map_err(|e| e.to_string())?;
    Ok(())
}
