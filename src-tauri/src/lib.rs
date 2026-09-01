mod commands;
mod tauri_sink;
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
    pub service: Arc<llm_relay_core::Service>,
}

pub fn run() {
    // Set up panic hook to log crashes
    let config_dir = get_app_config_dir();
    setup_panic_hook(&config_dir);

    // NOTE: The shared lifecycle guard (file lock + port bind) is acquired
    // inside `.setup()`, NOT here. `tauri_plugin_single_instance` must get
    // first crack at duplicate-GUI launches so it can focus the existing
    // window and exit the second process silently. If we acquired the guard
    // up front, the second GUI would hit the "AlreadyRunning" branch and
    // surface an error dialog before the plugin ever ran.

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Second instance launched — bring existing window to front
            show_main_window(app);
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
                hide_main_window(window);
            }
        })
        .setup(move |app| {
            // Acquire the shared LLM Relay lifecycle guard here, AFTER the
            // single-instance plugin has filtered out duplicate GUI launches.
            // This atomically:
            //   * grabs the global file lock at ~/.llm-relay/agent.lock
            //   * binds 127.0.0.1:18080
            //   * cleans stale pidfile / socket from prior unclean exits
            //   * writes a fresh pidfile
            // Reaching this point means we're the only GUI; any failure now
            // is a real conflict with the headless agent (or unrelated port
            // usage), so the daemon-takeover dialog is meaningful.
            let mut lifecycle_guard = match acquire_with_daemon_takeover() {
                Ok(g) => g,
                Err(()) => std::process::exit(1),
            };
            let proxy_listener = lifecycle_guard.take_listener();
            // Take the WSL listener before forget — once the guard is leaked
            // we can no longer reach the field.
            let initial_wsl = lifecycle_guard.wsl_listener.take();
            // Keep the guard alive for the lifetime of the app by leaking
            // it. Drop on process exit isn't reached anyway because Tauri
            // calls process::exit, but if we let the guard drop early the
            // lock would release while the GUI is still running.
            std::mem::forget(lifecycle_guard);

            // Init database
            let app_config_dir = get_app_config_dir();
            std::fs::create_dir_all(&app_config_dir).ok();
            llm_relay_core::keystore::init(&app_config_dir);
            let db = Arc::new(Database::init(&app_config_dir)?);

            // Build the shared event sink (Tauri implementation).
            let sink: llm_relay_core::SharedEventSink =
                std::sync::Arc::new(tauri_sink::TauriSink::new(app.handle().clone()));

            let service = std::sync::Arc::new(llm_relay_core::Service::new(db.clone(), sink.clone()));

            // Spawn proxy server with both listeners. ProxyState is built
            // directly from the same three Arcs Service holds, so the
            // proxy can come up before we attach its handle back onto
            // Service via with_proxy (avoiding a Service ↔ ProxyHandle
            // construction cycle).
            let proxy_state = llm_relay_core::proxy_server::ProxyState::new(
                service.db.clone(),
                service.switch_lock.clone(),
                service.sink.clone(),
            )
            .with_service(service.clone());
            let proxy_handle_fut = llm_relay_core::proxy_server::start_with_listeners(
                proxy_state,
                proxy_listener.expect("primary listener pre-bound by lifecycle"),
                initial_wsl,
            );
            // start_with_listeners is async only because it uses the tokio
            // runtime to spawn serve tasks; it doesn't await them. Drive
            // it on Tauri's runtime.
            let proxy_handle = tauri::async_runtime::block_on(proxy_handle_fut);
            // Stash the proxy handle on the app so future Tauri commands
            // (Reconnect WSL, etc.) can call rebind_wsl/shutdown.
            app.manage(proxy_handle.clone());

            let state = AppState {
                db: db.clone(),
                switch_lock: Arc::new(tokio::sync::Mutex::new(())),
                service: service.clone(),
            };

            // Build tray
            let menu = tray::create_tray_menu(app.handle(), &state)?;
            let icon = app.default_window_icon().cloned().unwrap();
            let tray_builder = TrayIconBuilder::with_id("main")
                .icon(icon)
                .menu(&menu)
                .on_menu_event(|app, event| {
                    tray::handle_tray_menu_event(app, &event.id.0);
                })
                // Windows convention: left-click opens the window, right-click
                // opens the menu. Worth honouring because with the window closed
                // the tray icon is the only way back in. macOS menu-bar extras
                // have no such split — one click *is* the menu — and Linux tray
                // backends deliver no click events at all, so both keep the
                // default of showing the menu on either button.
                .show_menu_on_left_click(!cfg!(target_os = "windows"));
            // Only registered where it can fire: Linux emits no click events,
            // and elsewhere an always-on handler would also be woken by every
            // Move event as the pointer crosses the icon.
            #[cfg(target_os = "windows")]
            let tray_builder = tray_builder.on_tray_icon_event(|tray, event| {
                // On the release edge, not the press: Windows sends Down and Up
                // as separate events, so matching either would open twice.
                if let tauri::tray::TrayIconEvent::Click {
                    button: tauri::tray::MouseButton::Left,
                    button_state: tauri::tray::MouseButtonState::Up,
                    ..
                } = event
                {
                    show_main_window(tray.app_handle());
                }
            });
            let _tray = tray_builder.build(app)?;

            app.manage(state);

            // Show window
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
            }

            // Start local proxy server (http://127.0.0.1:18080 + optional
            // WSL gateway-IP listener) — already done above via
            // start_with_listeners.

            // Start health check loop
            {
                let svc_for_health = service.clone();
                tauri::async_runtime::spawn(async move {
                    llm_relay_core::health::health_check_loop((*svc_for_health).clone()).await;
                });
            }

            // Start WSL detection state machine. Spawn requires the
            // proxy handle, which is on Service via with_proxy. Since
            // `service` here is Arc<Service> we need the proxy field
            // populated — we attach it on a fresh local Service clone.
            let svc_for_wsl: llm_relay_core::Service =
                (*service).clone().with_proxy(proxy_handle.clone());
            if let Some(sm) = svc_for_wsl.spawn_wsl_state_machine() {
                let sm_run = sm.clone();
                tauri::async_runtime::spawn(async move { sm_run.run().await; });
                app.manage(sm);
            }

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
            commands::list_target_snapshots,
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
            commands::list_suppressed_paths,
            commands::suppress_path,
            commands::unsuppress_path,
            commands::get_usage_stats,
            commands::start_device_login,
            commands::poll_device_login,
            commands::fetch_keys_with_token,
            commands::open_url,
            commands::list_wsl_distros,
            commands::toggle_wsl_distro,
            commands::refresh_wsl_distros,
        ]);

    let app = builder
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            show_main_window(app_handle);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (app_handle, event);
    });
}

/// Closing the window parks the app in the tray / menu bar instead of quitting
/// — the proxy has to keep serving whether or not anyone is looking at it.
///
/// Hiding is the whole story on Windows and Linux: a hidden window has no
/// taskbar button, and the tray icon is already there. macOS keeps the Dock
/// icon for as long as the process is a regular app, so it also has to drop to
/// UIElement (accessory) — that leaves only the menu-bar item, which is what a
/// background app should look like there.
fn hide_main_window(window: &tauri::Window) {
    let _ = window.hide();
    #[cfg(target_os = "macos")]
    {
        // tao swallows a hide that lands within a second of the last show, to
        // dodge a macOS bug that strands duplicate Dock icons. So open-then-
        // immediately-close keeps the icon until the next close — rare enough,
        // and far better than the duplicates.
        let _ = window.app_handle().set_dock_visibility(false);
    }
}

/// Bring the main window back from the tray / menu bar.
///
/// On macOS the Dock icon has to return *before* the window is shown: a
/// UIElement app cannot become frontmost, so `set_focus` alone would leave the
/// window sitting behind whatever the user was actually looking at.
pub(crate) fn show_main_window(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    let _ = app.set_dock_visibility(true);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn get_app_config_dir() -> std::path::PathBuf {
    llm_relay_core::paths::config_dir()
}

/// Acquire the lifecycle guard. If another LLM Relay process is detected and
/// it appears to be the headless agent (live pidfile + reachable IPC socket),
/// offer the user a one-click "Stop daemon and start GUI" option. The GUI
/// always wins ties so users can't get locked out by a forgotten daemon.
fn acquire_with_daemon_takeover() -> Result<llm_relay_core::lifecycle::LifecycleGuard, ()> {
    use llm_relay_core::lifecycle::{self, AcquireError, LifecycleGuard};

    match LifecycleGuard::acquire() {
        Ok(g) => return Ok(g),
        Err(AcquireError::AlreadyRunning) => {
            // Try the takeover path: only if a live agent pid + socket exist.
            let pid = lifecycle::live_agent_pid();
            let sock_exists = llm_relay_core::paths::sock_file().exists();
            if let (Some(pid), true) = (pid, sock_exists) {
                let confirm = rfd::MessageDialog::new()
                    .set_title("LLM Relay")
                    .set_description(format!(
                        "The LLM Relay daemon (PID {pid}) is already running.\n\n\
                         The GUI cannot start while the daemon holds the port.\n\
                         Stop the daemon and launch the GUI instead?"
                    ))
                    .set_level(rfd::MessageLevel::Warning)
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show();
                if matches!(confirm, rfd::MessageDialogResult::Yes) {
                    match lifecycle::request_agent_stop(std::time::Duration::from_secs(5)) {
                        Ok(()) => match LifecycleGuard::acquire() {
                            Ok(g) => return Ok(g),
                            Err(e) => {
                                log::error!("re-acquire after daemon stop failed: {e}");
                                rfd::MessageDialog::new()
                                    .set_title("LLM Relay")
                                    .set_description(format!(
                                        "Stopped the daemon, but couldn't start the GUI:\n{e}"
                                    ))
                                    .set_level(rfd::MessageLevel::Error)
                                    .show();
                                return Err(());
                            }
                        },
                        Err(msg) => {
                            log::error!("daemon stop request failed: {msg}");
                            rfd::MessageDialog::new()
                                .set_title("LLM Relay")
                                .set_description(format!(
                                    "Could not stop the daemon: {msg}\n\n\
                                     Try `kill {pid}` from a terminal, then relaunch."
                                ))
                                .set_level(rfd::MessageLevel::Error)
                                .show();
                            return Err(());
                        }
                    }
                }
                // User declined — exit silently.
                return Err(());
            }
            // No live agent — must be another GUI instance (single-instance
            // plugin should bring it to front; we just bail).
            rfd::MessageDialog::new()
                .set_title("LLM Relay")
                .set_description(
                    "Another LLM Relay process is already running. \
                     Please quit it before starting a new instance.",
                )
                .set_level(rfd::MessageLevel::Error)
                .show();
            Err(())
        }
        Err(AcquireError::PortInUse(_)) => {
            rfd::MessageDialog::new()
                .set_title("LLM Relay")
                .set_description(format!(
                    "Port {} is in use by another process. \
                     Free the port (or quit the process holding it) and try again.",
                    llm_relay_core::paths::proxy_port()
                ))
                .set_level(rfd::MessageLevel::Error)
                .show();
            Err(())
        }
        Err(AcquireError::Io(io)) => {
            rfd::MessageDialog::new()
                .set_title("LLM Relay")
                .set_description(format!("LLM Relay failed to initialize: {io}"))
                .set_level(rfd::MessageLevel::Error)
                .show();
            Err(())
        }
    }
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
