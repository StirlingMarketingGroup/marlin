use std::sync::Mutex;
use tauri::menu::CheckMenuItem;

pub struct MenuState<R: tauri::Runtime> {
    pub show_hidden_item: Mutex<Option<CheckMenuItem<R>>>,
    pub show_hidden_checked: Mutex<bool>,
    pub folders_first_item: Mutex<Option<CheckMenuItem<R>>>,
    pub folders_first_checked: Mutex<bool>,
    pub sort_order_asc_checked: Mutex<bool>,
}
