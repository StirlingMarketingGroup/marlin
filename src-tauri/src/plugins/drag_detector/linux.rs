use tauri::{Runtime, Window};

use super::DropZoneConfig;

pub fn setup_drag_handlers<R: Runtime>(_window: &Window<R>) -> Result<(), String> {
    Err("Linux drag detection not yet implemented".to_string())
}

pub fn set_drop_zone(_zone_id: &str, _enabled: bool, _config: Option<DropZoneConfig>) {
    // TODO: Implement Linux drag detection using GTK drag-and-drop
}
