mod commands;
mod config_writer;
mod database;
mod error;
mod gateway;
mod health;
mod tray;

use std::sync::Arc;
use tauri::Manager;
use tauri::tray::TrayIconBuilder;

pub use database::Database;
pub use error::AppError;

pub struct AppState {
    pub db: Arc<Database>,
}

pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_process::init())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            // Init database
            let app_config_dir = get_app_config_dir();
            std::fs::create_dir_all(&app_config_dir).ok();
            let db = Arc::new(Database::init(&app_config_dir)?);

            let state = AppState { db: db.clone() };

            // Build tray
            let menu = tray::create_tray_menu(app.handle(), &state)?;
            let _tray = TrayIconBuilder::with_id("main")
                .menu(&menu)
                .on_menu_event(|app, event| {
                    tray::handle_tray_menu_event(app, &event.id.0);
                })
                .show_menu_on_left_click(true)
                .build(app)?;

            app.manage(state);

            // Show window
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
            }

            // Start health check loop
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state = app_handle.state::<AppState>();
                health::health_check_loop(state.inner(), &app_handle).await;
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::add_gateway,
            commands::list_gateways,
            commands::update_gateway,
            commands::delete_gateway,
            commands::reorder_gateways,
            commands::login_gateway,
            commands::fetch_keys,
            commands::fetch_models,
            commands::check_all_health,
            commands::apply_config,
            commands::read_current_config,
            commands::clear_config,
            commands::get_active_config_cmd,
            commands::get_settings,
            commands::update_settings,
            commands::update_tray_menu,
        ]);

    let app = builder
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            if let Some(window) = app_handle.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (app_handle, event);
    });
}

fn get_app_config_dir() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".llm-relay")
}
