use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tauri::menu::CheckMenuItem;

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
