use tauri::{Runtime, Window};

pub fn setup_drag_handlers<R: Runtime>(_window: &Window<R>) -> Result<(), String> {
    Err("Windows drag detection not yet implemented".to_string())
}

pub fn set_drop_zone(_zone_id: &str, _enabled: bool) {
    // TODO: Implement Windows drag detection using IDropTarget
}
