mod commands;
mod fs_utils;
mod fs_watcher;
#[cfg(target_os = "macos")]
mod macos_icons;
mod menu;
#[cfg(target_os = "macos")]
mod native_drag;
mod plugins;
mod state;
mod thumbnails;

use state::MenuState;
use std::sync::Mutex;
#[cfg(target_os = "linux")]
use std::sync::OnceLock;

#[cfg(target_os = "linux")]
fn apply_linux_menu_css() {
    static CSS_ONCE: OnceLock<()> = OnceLock::new();
    CSS_ONCE.get_or_init(|| {
        use gtk::prelude::*;

        if !gtk::is_initialized() && !gtk::is_initialized_main_thread() {
            return;
        }

        const MENU_CSS: &str = r#"
            menu menuitem {
                padding-left: 6px;
            }

            menu menuitem > box {
                margin-left: -28px;
                padding-left: 0;
            }

            menu menuitem > box > image {
                margin-right: 6px;
            }

            menu menuitem > box > label,
            menu menuitem > box > accel-label {
                margin-left: 0;
            }
        "#;

        let provider = gtk::CssProvider::new();
        if provider.load_from_data(MENU_CSS.as_bytes()).is_err() {
            log::warn!("Failed to load menu CSS override for Linux");
            return;
        }

        if let Some(screen) = gtk::gdk::Screen::default() {
            gtk::StyleContext::add_provider_for_screen(
                &screen,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        } else {
            log::warn!("Failed to acquire default GDK screen for menu CSS application");
        }
    });
}
use tauri::Manager;
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_decorum::init())
        .plugin(tauri_plugin_drag::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_home_directory,
            commands::read_directory,
            commands::get_file_metadata,
            commands::resolve_symlink_parent_command,
            commands::create_directory_command,
            commands::delete_file,
            commands::rename_file,
            commands::copy_file,
            commands::move_file,
            commands::get_system_accent_color,
            commands::get_application_icon,
            commands::update_hidden_files_menu,
            commands::update_folders_first_menu,
            commands::update_sort_menu_state,
            commands::get_system_drives,
            commands::eject_drive,
            commands::request_thumbnail,
            commands::cancel_thumbnail,
            commands::get_thumbnail_cache_stats,
            commands::clear_thumbnail_cache,
            commands::open_path,
            commands::new_window,
            commands::show_native_context_menu,
            commands::update_selection_menu_state,
            commands::render_svg_to_png,
            commands::read_preferences,
            commands::write_preferences,
            commands::get_dir_prefs,
            commands::set_dir_prefs,
            commands::clear_all_dir_prefs,
            commands::set_last_dir,
            commands::toggle_menu_visibility,
            commands::start_native_drag,
            commands::start_watching_directory,
            commands::stop_watching_directory,
            commands::stop_all_watchers,
            commands::is_watching_directory,
            commands::get_watched_directories,
            commands::get_pinned_directories,
            commands::add_pinned_directory,
            commands::remove_pinned_directory,
            commands::reorder_pinned_directories,
            plugins::drag_detector::enable_drag_detection,
            plugins::drag_detector::set_drop_zone,
            plugins::drag_detector::simulate_drop,
        ])
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Initialize the file system watcher
            fs_watcher::init_watcher(app.handle().clone());

            // Position traffic lights similar to Finder's sidebar style
            #[cfg(target_os = "macos")]
            {
                use tauri_plugin_decorum::WebviewWindowExt;
                if let Some(main_window) = app.get_webview_window("main") {
                    // Position traffic lights inside the sidebar area (16px right, 24px down)
                    let _ = main_window.set_traffic_lights_inset(16.0, 24.0);
                }
            }

            // Create and set the menu
            let (
                app_menu,
                show_hidden_item,
                folders_first_item,
                sort_name_item,
                sort_size_item,
                sort_type_item,
                sort_modified_item,
                sort_order_asc_item,
                sort_order_desc_item,
            ) = menu::create_menu(&app.handle())?;
            app.set_menu(app_menu)?;

            #[cfg(target_os = "linux")]
            {
                if let Some(main_window) = app.get_webview_window("main") {
                    if let Err(err) = main_window.set_decorations(false) {
                        log::warn!("Failed to disable window decorations: {err}");
                    }
                    if let Err(err) = main_window.hide_menu() {
                        log::warn!("Failed to hide menu on startup: {err}");
                    }
                }

                apply_linux_menu_css();
            }

            // Store the menu item in managed state
            app.manage(MenuState {
                show_hidden_item: Mutex::new(Some(show_hidden_item)),
                show_hidden_checked: Mutex::new(false),
                folders_first_item: Mutex::new(Some(folders_first_item)),
                folders_first_checked: Mutex::new(true),
                sort_order_asc_checked: Mutex::new(true),
                current_sort_by: Mutex::new("name".to_string()),
                sort_name_item: Mutex::new(Some(sort_name_item)),
                sort_size_item: Mutex::new(Some(sort_size_item)),
                sort_type_item: Mutex::new(Some(sort_type_item)),
                sort_modified_item: Mutex::new(Some(sort_modified_item)),
                sort_asc_item: Mutex::new(Some(sort_order_asc_item)),
                sort_desc_item: Mutex::new(Some(sort_order_desc_item)),
                has_selection: Mutex::new(false),
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
                    tauri::WindowEvent::Resized(_)
                    | tauri::WindowEvent::Focused(_)
                    | tauri::WindowEvent::ThemeChanged(_) => {
                        let label = window.label().to_string();
                        if let Some(webview_window) = window.app_handle().get_webview_window(&label)
                        {
                            let _ = webview_window.set_traffic_lights_inset(16.0, 24.0);
                        }
                    }
                    _ => {}
                }
            }

            #[cfg(not(target_os = "macos"))]
            {
                let _ = window;
                let _ = event;
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
