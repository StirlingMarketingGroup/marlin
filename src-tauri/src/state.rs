use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tauri::menu::CheckMenuItem;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use trash::TrashItem;

pub struct MenuState<R: tauri::Runtime> {
    pub show_hidden_item: Mutex<Option<CheckMenuItem<R>>>,
    pub show_hidden_checked: Mutex<bool>,
    pub folders_first_item: Mutex<Option<CheckMenuItem<R>>>,
    pub folders_first_checked: Mutex<bool>,
    pub sort_order_asc_checked: Mutex<bool>,
    pub current_sort_by: Mutex<String>,
    // System menu: sort items
    pub sort_name_item: Mutex<Option<CheckMenuItem<R>>>,
    pub sort_size_item: Mutex<Option<CheckMenuItem<R>>>,
    pub sort_type_item: Mutex<Option<CheckMenuItem<R>>>,
    pub sort_modified_item: Mutex<Option<CheckMenuItem<R>>>,
    pub sort_asc_item: Mutex<Option<CheckMenuItem<R>>>,
    pub sort_desc_item: Mutex<Option<CheckMenuItem<R>>>,
    // Selection state to drive context menu enable/visibility
    pub has_selection: Mutex<bool>,
}

#[derive(Clone)]
pub struct FolderSizeTaskHandle {
    pub cancel_flag: Arc<AtomicBool>,
}

pub struct FolderSizeState {
    pub tasks: Mutex<HashMap<String, FolderSizeTaskHandle>>,
}

impl Default for FolderSizeState {
    fn default() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
        }
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
pub struct MacTrashUndoItem {
    pub trashed_path: String,
    pub original_path: String,
}

pub enum TrashUndoKind {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    Items(Vec<TrashItem>),
    #[cfg(target_os = "macos")]
    MacItems(Vec<MacTrashUndoItem>),
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    Unsupported,
}

#[cfg_attr(not(any(target_os = "windows", target_os = "linux", target_os = "macos")), allow(dead_code))]
pub struct TrashUndoRecord {
    pub kind: TrashUndoKind,
    pub original_paths: Vec<String>,
    pub created_at: Instant,
}

pub struct TrashUndoState {
    pub records: Mutex<HashMap<String, TrashUndoRecord>>,
}

impl Default for TrashUndoState {
    fn default() -> Self {
        Self {
            records: Mutex::new(HashMap::new()),
        }
    }
}
