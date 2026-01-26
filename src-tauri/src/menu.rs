use tauri::{
    menu::{AboutMetadata, CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder},
    AppHandle, Emitter, Manager, Runtime,
};
use tauri_plugin_opener::OpenerExt;

const GITHUB_REPO_URL: &str = "https://github.com/StirlingMarketingGroup/marlin";

pub fn create_menu<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<
    (
        tauri::menu::Menu<R>,
        tauri::menu::CheckMenuItem<R>, // show_hidden
        tauri::menu::CheckMenuItem<R>, // folders_first
        tauri::menu::CheckMenuItem<R>, // sort_name
        tauri::menu::CheckMenuItem<R>, // sort_size
        tauri::menu::CheckMenuItem<R>, // sort_type
        tauri::menu::CheckMenuItem<R>, // sort_modified
        tauri::menu::CheckMenuItem<R>, // sort_order_asc
        tauri::menu::CheckMenuItem<R>, // sort_order_desc
    ),
    tauri::Error,
> {
    #[cfg(target_os = "macos")]
    let preferences_item = MenuItemBuilder::with_id("menu:preferences", "Preferences...")
        .accelerator("CmdOrCtrl+,")
        .build(app)?;

    #[cfg(not(target_os = "macos"))]
    let preferences_item = MenuItemBuilder::with_id("menu:preferences", "Settings...")
        .accelerator("CmdOrCtrl+,")
        .build(app)?;

    // Create App submenu (appears under app name on macOS)
    #[cfg(target_os = "macos")]
    let app_submenu = SubmenuBuilder::new(app, "Marlin")
        .about(Some(AboutMetadata {
            name: Some("Marlin".to_string()),
            version: Some("0.1.0".to_string()),
            ..Default::default()
        }))
        .separator()
        .item(&preferences_item)
        .text("menu:reset_folder_defaults", "Reset Folder Defaults...")
        .text("menu:clear_thumbnail_cache", "Clear Thumbnail Cache...")
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .separator()
        .quit()
        .build()?;

    #[cfg(not(target_os = "macos"))]
    let app_submenu = SubmenuBuilder::new(app, "Marlin")
        .about(Some(AboutMetadata {
            name: Some("Marlin".to_string()),
            version: Some("0.1.0".to_string()),
            ..Default::default()
        }))
        .separator()
        .text("menu:reset_folder_defaults", "Reset Folder Defaults...")
        .text("menu:clear_thumbnail_cache", "Clear Thumbnail Cache...")
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .separator()
        .quit()
        .build()?;

    // Create New Window menu item with accelerator
    let new_window_item = MenuItemBuilder::with_id("menu:new_window", "New Window")
        .accelerator("CmdOrCtrl+N")
        .build(app)?;
    let new_folder_item = MenuItemBuilder::with_id("menu:new_folder", "New Folder")
        .accelerator("CmdOrCtrl+Shift+N")
        .build(app)?;
    let new_file_item = MenuItemBuilder::with_id("menu:new_file", "New File")
        .accelerator("CmdOrCtrl+Alt+N")
        .build(app)?;

    // Create File submenu
    #[cfg(target_os = "macos")]
    let file_submenu = SubmenuBuilder::new(app, "File")
        .item(&new_window_item)
        .separator()
        .item(&new_file_item)
        .item(&new_folder_item)
        .separator()
        .text("menu:refresh", "Refresh")
        .separator()
        .close_window()
        .build()?;

    #[cfg(not(target_os = "macos"))]
    let file_submenu = SubmenuBuilder::new(app, "File")
        .item(&new_window_item)
        .separator()
        .item(&preferences_item)
        .separator()
        .item(&new_file_item)
        .item(&new_folder_item)
        .separator()
        .text("menu:refresh", "Refresh")
        .separator()
        .close_window()
        .build()?;

    // Custom clipboard items (frontend decides between text vs file operations)
    let copy_files_item = MenuItemBuilder::with_id("menu:copy_files", "Copy")
        .accelerator("CmdOrCtrl+C")
        .build(app)?;
    let cut_files_item = MenuItemBuilder::with_id("menu:cut_files", "Cut")
        .accelerator("CmdOrCtrl+X")
        .build(app)?;
    let paste_files_item = MenuItemBuilder::with_id("menu:paste_files", "Paste")
        .accelerator("CmdOrCtrl+V")
        .build(app)?;

    // Create Edit submenu
    let edit_submenu = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .item(&cut_files_item)
        .item(&copy_files_item)
        .item(&paste_files_item)
        .separator()
        .item(&PredefinedMenuItem::select_all(app, None)?)
        .build()?;

    // Create the Show Hidden Files checkbox with explicit unchecked state
    let show_hidden_item = CheckMenuItemBuilder::with_id("menu:toggle_hidden", "Show Hidden Files")
        .checked(false) // Explicitly set to unchecked to match store default
        .build(app)?;

    // Clone for returning (we need to return a copy for storing in state)
    let show_hidden_clone = show_hidden_item.clone();

    // Create Folders on Top checkbox (enabled by default)
    let folders_first_item = CheckMenuItemBuilder::with_id("menu:folders_first", "Folders on Top")
        .checked(true)
        .build(app)?;
    let folders_first_clone = folders_first_item.clone();

    // Build Sort submenu with checkable items
    let sort_name_item = CheckMenuItemBuilder::with_id("menu:sort_name", "Name")
        .checked(true)
        .build(app)?;
    let sort_size_item = CheckMenuItemBuilder::with_id("menu:sort_size", "Size")
        .checked(false)
        .build(app)?;
    let sort_type_item = CheckMenuItemBuilder::with_id("menu:sort_type", "Type")
        .checked(false)
        .build(app)?;
    let sort_modified_item = CheckMenuItemBuilder::with_id("menu:sort_modified", "Date Modified")
        .checked(false)
        .build(app)?;

    let sort_order_asc_item = CheckMenuItemBuilder::with_id("menu:sort_order_asc", "Ascending")
        .checked(true)
        .build(app)?;
    let sort_order_desc_item = CheckMenuItemBuilder::with_id("menu:sort_order_desc", "Descending")
        .checked(false)
        .build(app)?;

    let sort_submenu = SubmenuBuilder::new(app, "Sort by")
        .item(&sort_name_item)
        .item(&sort_size_item)
        .item(&sort_type_item)
        .item(&sort_modified_item)
        .separator()
        .item(&sort_order_asc_item)
        .item(&sort_order_desc_item)
        .build()?;

    // Create View submenu
    let view_submenu = SubmenuBuilder::new(app, "View")
        .text("menu:view_grid", "as Grid")
        .text("menu:view_list", "as List")
        .separator()
        .item(&show_hidden_item)
        .item(&folders_first_item)
        .separator()
        .item(&sort_submenu)
        .build()?;

    // Create Window submenu
    let window_submenu = SubmenuBuilder::new(app, "Window")
        .minimize()
        .separator()
        .fullscreen()
        .build()?;

    let help_submenu = SubmenuBuilder::new(app, "Help")
        .text("menu:view_github", "View on GitHub")
        .build()?;

    // Build the complete menu with all submenus
    let menu = MenuBuilder::new(app)
        .items(&[
            &app_submenu,
            &file_submenu,
            &edit_submenu,
            &view_submenu,
            &window_submenu,
            &help_submenu,
        ])
        .build()?;

    Ok((
        menu,
        show_hidden_clone,
        folders_first_clone,
        sort_name_item,
        sort_size_item,
        sort_type_item,
        sort_modified_item,
        sort_order_asc_item,
        sort_order_desc_item,
    ))
}

pub fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, event: &tauri::menu::MenuEvent) {
    let event_id = event.id().0.as_str();
    match event_id {
        "menu:preferences" => {
            let _ = app.emit("menu:preferences", ());
        }
        "menu:toggle_hidden" => {
            // Read and toggle the stored checked state, update the menu item, then emit the new value
            let state: tauri::State<crate::state::MenuState<R>> = app.state();
            let new_checked = {
                let mut checked_guard = state
                    .show_hidden_checked
                    .lock()
                    .expect("lock menu checked state");
                *checked_guard = !*checked_guard;
                *checked_guard
            };
            if let Ok(item_guard) = state.show_hidden_item.lock() {
                if let Some(ref item) = *item_guard {
                    let _ = item.set_checked(new_checked);
                }
            }
            let _ = app.emit("menu:toggle_hidden", new_checked);
        }
        "menu:view_list" => {
            let _ = app.emit("menu:view_list", ());
        }
        "menu:view_grid" => {
            let _ = app.emit("menu:view_grid", ());
        }
        "menu:sort_name" => {
            // Update check states and emit
            let state: tauri::State<crate::state::MenuState<R>> = app.state();
            if let Ok(mut sb) = state.current_sort_by.lock() {
                *sb = "name".to_string();
            }
            if let Ok(item) = state.sort_name_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(true);
                }
            }
            if let Ok(item) = state.sort_size_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(false);
                }
            }
            if let Ok(item) = state.sort_type_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(false);
                }
            }
            if let Ok(item) = state.sort_modified_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(false);
                }
            }
            let _ = app.emit("menu:sort_name", ());
        }
        "menu:sort_size" => {
            let state: tauri::State<crate::state::MenuState<R>> = app.state();
            if let Ok(mut sb) = state.current_sort_by.lock() {
                *sb = "size".to_string();
            }
            if let Ok(item) = state.sort_name_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(false);
                }
            }
            if let Ok(item) = state.sort_size_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(true);
                }
            }
            if let Ok(item) = state.sort_type_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(false);
                }
            }
            if let Ok(item) = state.sort_modified_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(false);
                }
            }
            let _ = app.emit("menu:sort_size", ());
        }
        "menu:sort_type" => {
            let state: tauri::State<crate::state::MenuState<R>> = app.state();
            if let Ok(mut sb) = state.current_sort_by.lock() {
                *sb = "type".to_string();
            }
            if let Ok(item) = state.sort_name_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(false);
                }
            }
            if let Ok(item) = state.sort_size_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(false);
                }
            }
            if let Ok(item) = state.sort_type_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(true);
                }
            }
            if let Ok(item) = state.sort_modified_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(false);
                }
            }
            let _ = app.emit("menu:sort_type", ());
        }
        "menu:sort_modified" => {
            let state: tauri::State<crate::state::MenuState<R>> = app.state();
            if let Ok(mut sb) = state.current_sort_by.lock() {
                *sb = "modified".to_string();
            }
            if let Ok(item) = state.sort_name_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(false);
                }
            }
            if let Ok(item) = state.sort_size_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(false);
                }
            }
            if let Ok(item) = state.sort_type_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(false);
                }
            }
            if let Ok(item) = state.sort_modified_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(true);
                }
            }
            let _ = app.emit("menu:sort_modified", ());
        }
        "menu:sort_order_asc" => {
            let state: tauri::State<crate::state::MenuState<R>> = app.state();
            if let Ok(mut asc) = state.sort_order_asc_checked.lock() {
                *asc = true;
            }
            if let Ok(item) = state.sort_asc_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(true);
                }
            }
            if let Ok(item) = state.sort_desc_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(false);
                }
            }
            let _ = app.emit("menu:sort_order_asc", ());
        }
        "menu:sort_order_desc" => {
            let state: tauri::State<crate::state::MenuState<R>> = app.state();
            if let Ok(mut asc) = state.sort_order_asc_checked.lock() {
                *asc = false;
            }
            if let Ok(item) = state.sort_asc_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(false);
                }
            }
            if let Ok(item) = state.sort_desc_item.lock() {
                if let Some(i) = &*item {
                    let _ = i.set_checked(true);
                }
            }
            let _ = app.emit("menu:sort_order_desc", ());
        }
        "menu:refresh" => {
            let _ = app.emit("menu:refresh", ());
        }
        "menu:new_folder" => {
            let _ = app.emit("menu:new_folder", ());
        }
        "menu:new_file" => {
            let _ = app.emit("menu:new_file", ());
        }
        "menu:new_window" => {
            let _ = app.emit("menu:new_window", ());
        }
        "menu:view_github" => {
            if let Err(error) = app.opener().open_url(GITHUB_REPO_URL, None::<&str>) {
                log::warn!("Failed to open GitHub repo page: {error}");
            }
        }
        "ctx:toggle_hidden" => {
            let state: tauri::State<crate::state::MenuState<R>> = app.state();
            let new_checked = {
                let mut checked_guard = state
                    .show_hidden_checked
                    .lock()
                    .expect("lock menu checked state");
                *checked_guard = !*checked_guard;
                *checked_guard
            };
            if let Ok(item_guard) = state.show_hidden_item.lock() {
                if let Some(ref item) = *item_guard {
                    let _ = item.set_checked(new_checked);
                }
            }
            let _ = app.emit("menu:toggle_hidden", new_checked);
            // Also emit a contextual event for clients that listen to ctx-specific events
            let _ = app.emit("ctx:toggle_hidden", new_checked);
        }
        "ctx:folders_first" => {
            let state: tauri::State<crate::state::MenuState<R>> = app.state();
            let new_checked = {
                let mut checked_guard = state
                    .folders_first_checked
                    .lock()
                    .expect("lock menu checked state");
                *checked_guard = !*checked_guard;
                *checked_guard
            };
            if let Ok(item_guard) = state.folders_first_item.lock() {
                if let Some(ref item) = *item_guard {
                    let _ = item.set_checked(new_checked);
                }
            }
            let _ = app.emit("menu:folders_first", new_checked);
            let _ = app.emit("ctx:folders_first", new_checked);
        }
        "ctx:sort_name" => {
            let _ = app.emit("menu:sort_name", ());
        }
        "ctx:sort_size" => {
            let _ = app.emit("menu:sort_size", ());
        }
        "ctx:sort_type" => {
            let _ = app.emit("menu:sort_type", ());
        }
        "ctx:sort_modified" => {
            let _ = app.emit("menu:sort_modified", ());
        }
        "ctx:sort_order_asc" => {
            let state: tauri::State<crate::state::MenuState<R>> = app.state();
            if let Ok(mut asc) = state.sort_order_asc_checked.lock() {
                *asc = true;
            }
            let _ = app.emit("menu:sort_order_asc", ());
        }
        "ctx:sort_order_desc" => {
            let state: tauri::State<crate::state::MenuState<R>> = app.state();
            if let Ok(mut asc) = state.sort_order_asc_checked.lock() {
                *asc = false;
            }
            let _ = app.emit("menu:sort_order_desc", ());
        }
        "ctx:copy_name" => {
            let _ = app.emit("menu:copy_name", ());
        }
        "ctx:copy_full_name" => {
            let _ = app.emit("menu:copy_full_name", ());
        }
        "ctx:calculate_total_size" => {
            let _ = app.emit("menu:calculate_total_size", ());
        }
        "ctx:rename" => {
            let _ = app.emit("menu:rename", ());
        }
        "ctx:reveal_symlink" => {
            let _ = app.emit("menu:reveal_symlink", ());
        }
        "menu:folders_first" => {
            // Toggle and emit new value
            let state: tauri::State<crate::state::MenuState<R>> = app.state();
            let new_checked = {
                let mut checked_guard = state
                    .folders_first_checked
                    .lock()
                    .expect("lock menu checked state");
                *checked_guard = !*checked_guard;
                *checked_guard
            };
            if let Ok(item_guard) = state.folders_first_item.lock() {
                if let Some(ref item) = *item_guard {
                    let _ = item.set_checked(new_checked);
                }
            }
            let _ = app.emit("menu:folders_first", new_checked);
        }
        "menu:reset_folder_defaults" => {
            let _ = app.emit("menu:reset_folder_defaults", ());
        }
        "menu:clear_thumbnail_cache" => {
            let _ = app.emit("menu:clear_thumbnail_cache", ());
        }
        "menu:copy_files" => {
            let _ = app.emit("menu:copy_files", ());
        }
        "menu:cut_files" => {
            let _ = app.emit("menu:cut_files", ());
        }
        "menu:paste_files" => {
            let _ = app.emit("menu:paste_files", ());
        }
        _ => {}
    }
}
