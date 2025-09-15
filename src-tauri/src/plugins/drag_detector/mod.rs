use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::{
    plugin::{Builder, TauriPlugin},
    AppHandle, Emitter, Manager, Runtime, State,
};

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

pub struct DragDetector {
    drop_zones: Mutex<Vec<String>>,
}

impl DragDetector {
    pub fn new() -> Self {
        Self {
            drop_zones: Mutex::new(Vec::new()),
        }
    }

    pub fn add_drop_zone(&self, zone_id: String) {
        let mut zones = self.drop_zones.lock().unwrap();
        if !zones.contains(&zone_id) {
            zones.push(zone_id);
        }
    }

    pub fn remove_drop_zone(&self, zone_id: &str) {
        let mut zones = self.drop_zones.lock().unwrap();
        zones.retain(|z| z != zone_id);
    }
}

#[tauri::command]
async fn enable_drag_detection() -> Result<(), String> {
    // Simplified implementation for now
    Ok(())
}

#[tauri::command]
async fn set_drop_zone(
    zone_id: String,
    enabled: bool,
    detector: State<'_, DragDetector>,
) -> Result<(), String> {
    if enabled {
        detector.add_drop_zone(zone_id);
    } else {
        detector.remove_drop_zone(&zone_id);
    }
    Ok(())
}

#[tauri::command]
async fn simulate_drop(paths: Vec<String>, target_id: String) -> Result<(), String> {
    // Simplified for now - would emit events in full implementation
    Ok(())
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::<R>::new("drag-detector")
        .invoke_handler(tauri::generate_handler![
            enable_drag_detection,
            set_drop_zone,
            simulate_drop
        ])
        .setup(|app, _api| {
            let detector = DragDetector::new();
            app.manage(detector);
            Ok(())
        })
        .build()
}
