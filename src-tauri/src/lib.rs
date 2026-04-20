mod commands;
mod health;
mod proxy_server;
mod tray;

use std::sync::Arc;
use tauri::Manager;
use tauri::tray::TrayIconBuilder;

pub use llm_relay_core::Database;
pub use llm_relay_core::AppError;

pub struct AppState {
    pub db: Arc<Database>,
    /// Held during any gateway switch to prevent concurrent switches.
    pub switch_lock: Arc<tokio::sync::Mutex<()>>,
}

pub fn run() {
    // Set up panic hook to log crashes
    let config_dir = get_app_config_dir();
    setup_panic_hook(&config_dir);

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Second instance launched — bring existing window to front
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_process::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .build(),
        )
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

            let state = AppState {
                db: db.clone(),
                switch_lock: Arc::new(tokio::sync::Mutex::new(())),
            };

            // Build tray
            let menu = tray::create_tray_menu(app.handle(), &state)?;
            let icon = app.default_window_icon().cloned().unwrap();
            let _tray = TrayIconBuilder::with_id("main")
                .icon(icon)
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

            // Start local proxy server (http://127.0.0.1:18080)
            let db_for_proxy = db.clone();
            let proxy_app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                proxy_server::start(db_for_proxy, proxy_app_handle).await;
            });

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
            commands::test_heartbeat,
            commands::apply_config,
            commands::read_current_config,
            commands::clear_config,
            commands::get_active_config_cmd,
            commands::get_settings,
            commands::update_settings,
            commands::update_tray_menu,
            commands::get_health_log,
            commands::get_client_name,
            commands::set_client_name,
            commands::get_autostart,
            commands::set_autostart,
            commands::get_traffic_logs,
            commands::get_usage_stats,
            commands::start_device_login,
            commands::poll_device_login,
            commands::fetch_keys_with_token,
            commands::open_url,
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

fn setup_panic_hook(config_dir: &std::path::Path) {
    let log_path = config_dir.join("crash.log");
    std::panic::set_hook(Box::new(move |info| {
        let now = chrono::Utc::now().to_rfc3339();
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown".to_string());
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(|s| s.as_str()))
            .unwrap_or("unknown panic");
        let entry = format!("[{now}] PANIC at {location}: {msg}\n");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let _ = std::io::Write::write_all(&mut f, entry.as_bytes());
        }
    }));
}
