mod commands;
mod fs_utils;
mod menu;
mod state;
mod thumbnails;
#[cfg(target_os = "macos")]
mod macos_icons;

use std::sync::Mutex;
use tauri::Manager;
use state::MenuState;
use tauri_plugin_decorum::WebviewWindowExt;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .plugin(tauri_plugin_fs::init())
    .plugin(tauri_plugin_os::init())
    .plugin(tauri_plugin_shell::init())
    .plugin(tauri_plugin_dialog::init())
    .plugin(tauri_plugin_decorum::init())
    .invoke_handler(tauri::generate_handler![
      commands::get_home_directory,
      commands::read_directory,
      commands::get_file_metadata,
      commands::create_directory_command,
      commands::delete_file,
      commands::rename_file,
      commands::copy_file,
      commands::move_file,
      commands::get_system_accent_color,
      commands::get_application_icon,
      commands::update_hidden_files_menu,
      commands::get_system_drives,
      commands::eject_drive,
      commands::request_thumbnail,
      commands::cancel_thumbnail,
      commands::get_thumbnail_cache_stats,
      commands::clear_thumbnail_cache,
      commands::open_path,
    ])
    .setup(|app| {
      if cfg!(debug_assertions) {
        app.handle().plugin(
          tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .build(),
        )?;
      }

      // Position traffic lights similar to Finder's sidebar style
      #[cfg(target_os = "macos")]
      {
        if let Some(main_window) = app.get_webview_window("main") {
          // Position traffic lights inside the sidebar area (16px right, 24px down)
          let _ = main_window.set_traffic_lights_inset(16.0, 24.0);
        }
      }

      // Create and set the menu
      let (app_menu, show_hidden_item) = menu::create_menu(&app.handle())?;
      app.set_menu(app_menu)?;

      // Store the menu item in managed state
      app.manage(MenuState {
        show_hidden_item: Mutex::new(Some(show_hidden_item)),
        show_hidden_checked: Mutex::new(false),
      });

      Ok(())
    })
    .on_menu_event(|app, event| {
      menu::handle_menu_event(app, &event);
    })
    .on_window_event(|window, event| {
      #[cfg(target_os = "macos")]
      {
        use tauri::Manager;
        use tauri_plugin_decorum::WebviewWindowExt;
        
        // Reapply traffic light position after events that might reset it
        match event {
          tauri::WindowEvent::Resized(_) | 
          tauri::WindowEvent::Focused(_) |
          tauri::WindowEvent::ThemeChanged(_) => {
            // Get the WebviewWindow from the app handle to call decorum methods
            if let Some(webview_window) = window.app_handle().get_webview_window("main") {
              let _ = webview_window.set_traffic_lights_inset(16.0, 24.0);
            }
          }
          _ => {}
        }
      }
    })
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
