use std::sync::Mutex;
use tauri::menu::CheckMenuItem;

pub struct MenuState<R: tauri::Runtime> {
    pub show_hidden_item: Mutex<Option<CheckMenuItem<R>>>,
    pub show_hidden_checked: Mutex<bool>,
}
