use tauri::{
    menu::{MenuBuilder, SubmenuBuilder, CheckMenuItemBuilder, AboutMetadata},
    AppHandle, Emitter, Manager, Runtime,
};

pub fn create_menu<R: Runtime>(app: &AppHandle<R>) -> Result<(tauri::menu::Menu<R>, tauri::menu::CheckMenuItem<R>), tauri::Error> {
    // Create App submenu (appears under app name on macOS)
    let app_submenu = SubmenuBuilder::new(app, "Marlin File Browser")
        .about(Some(AboutMetadata {
            name: Some("Marlin File Browser".to_string()),
            version: Some("0.1.0".to_string()),
            ..Default::default()
        }))
        .separator()
        .text("menu:preferences", "Preferences...")
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .separator()
        .quit()
        .build()?;

    // Create File submenu
    let file_submenu = SubmenuBuilder::new(app, "File")
        .text("menu:new_folder", "New Folder")
        .separator()
        .text("menu:refresh", "Refresh")
        .separator()
        .close_window()
        .build()?;

    // Create Edit submenu
    let edit_submenu = SubmenuBuilder::new(app, "Edit")
        .cut()
        .copy()
        .paste()
        .separator()
        .select_all()
        .build()?;

    // Create the Show Hidden Files checkbox with explicit unchecked state
    let show_hidden_item = CheckMenuItemBuilder::with_id("menu:toggle_hidden", "Show Hidden Files")
        .checked(false)  // Explicitly set to unchecked to match store default
        .build(app)?;
    
    // Clone for returning (we need to return a copy for storing in state)
    let show_hidden_clone = show_hidden_item.clone();

    // Create View submenu
    let view_submenu = SubmenuBuilder::new(app, "View")
        .text("menu:view_grid", "as Grid")
        .text("menu:view_list", "as List")
        .separator()
        .item(&show_hidden_item)
        .separator()
        .text("menu:sort_name", "Sort by Name")
        .text("menu:sort_size", "Sort by Size")
        .text("menu:sort_type", "Sort by Type")
        .text("menu:sort_modified", "Sort by Date Modified")
        .build()?;

    // Create Window submenu
    let window_submenu = SubmenuBuilder::new(app, "Window")
        .minimize()
        .separator()
        .fullscreen()
        .build()?;

    // Build the complete menu with all submenus
    let menu = MenuBuilder::new(app)
        .items(&[
            &app_submenu,
            &file_submenu,
            &edit_submenu,
            &view_submenu,
            &window_submenu,
        ])
        .build()?;

    Ok((menu, show_hidden_clone))
}

pub fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, event: &tauri::menu::MenuEvent) {
    let event_id = event.id().0.as_str();
    match event_id {
        "menu:toggle_hidden" => {
            // Read and toggle the stored checked state, update the menu item, then emit the new value
            let state: tauri::State<crate::state::MenuState<R>> = app.state();
            let new_checked = {
                let mut checked_guard = state.show_hidden_checked.lock().expect("lock menu checked state");
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
        "menu:view_list" => { let _ = app.emit("menu:view_list", ()); }
        "menu:view_grid" => {
            let _ = app.emit("menu:view_grid", ());
        }
        "menu:sort_name" => {
            let _ = app.emit("menu:sort_name", ());
        }
        "menu:sort_size" => {
            let _ = app.emit("menu:sort_size", ());
        }
        "menu:sort_type" => {
            let _ = app.emit("menu:sort_type", ());
        }
        "menu:sort_modified" => {
            let _ = app.emit("menu:sort_modified", ());
        }
        "menu:sort_order_asc" => {
            let _ = app.emit("menu:sort_order_asc", ());
        }
        "menu:sort_order_desc" => {
            let _ = app.emit("menu:sort_order_desc", ());
        }
        "menu:refresh" => {
            let _ = app.emit("menu:refresh", ());
        }
        "menu:new_folder" => {
            let _ = app.emit("menu:new_folder", ());
        }
        _ => {}
    }
}
