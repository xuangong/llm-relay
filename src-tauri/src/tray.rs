use tauri::menu::{CheckMenuItem, Menu, MenuBuilder, MenuItem};
use tauri::Manager;

use llm_relay_core::AppError;
use crate::AppState;

/// Create the system tray menu.
///
/// Layout:
/// ```text
/// LLM Relay
/// ─────────────
/// [check] Local Docker (12ms)    ← current
/// [ ] CF Workers (89ms)
/// [x] Staging (offline)
/// ─────────────
/// Key: xian
/// ─────────────
/// Open Main Window
/// Quit
/// ```
pub fn create_tray_menu(
    app: &tauri::AppHandle,
    state: &AppState,
) -> Result<Menu<tauri::Wry>, AppError> {
    let mut builder = MenuBuilder::new(app);

    // Header
    let header = MenuItem::with_id(app, "header", "LLM Relay", false, None::<&str>)
        .map_err(|e| AppError::Config(format!("Failed to create header: {e}")))?;
    builder = builder.item(&header).separator();

    // Gateway list
    let gateways = state.db.list_gateways_with_health()?;
    let active_config = state.db.get_active_config()?;
    let active_gw_id = active_config.gateway_id.as_deref();

    if gateways.is_empty() {
        let empty = MenuItem::with_id(app, "no_gateways", "(no gateways)", false, None::<&str>)
            .map_err(|e| AppError::Config(format!("Failed to create empty item: {e}")))?;
        builder = builder.item(&empty);
    } else {
        for gw in &gateways {
            let is_active = active_gw_id == Some(gw.gateway.id.as_str());
            let label = if gw.is_healthy {
                let latency = gw.latency_ms.map(|ms| format!("{ms}ms")).unwrap_or_default();
                format!("{} ({})", gw.gateway.name, latency)
            } else {
                format!("{} (offline)", gw.gateway.name)
            };

            let item = CheckMenuItem::with_id(
                app,
                format!("gw_{}", gw.gateway.id),
                &label,
                true,
                is_active,
                None::<&str>,
            )
            .map_err(|e| AppError::Config(format!("Failed to create gateway item: {e}")))?;
            builder = builder.item(&item);
        }
    }

    builder = builder.separator();

    // Key info
    if let Some(ref key_name) = active_config.key_name {
        let key_info =
            MenuItem::with_id(app, "key_info", &format!("Key: {key_name}"), false, None::<&str>)
                .map_err(|e| AppError::Config(format!("Failed to create key info: {e}")))?;
        builder = builder.item(&key_info).separator();
    }

    // Auto-switch status
    let auto_label = if active_config.auto_switch {
        "Auto-switch: ON"
    } else {
        "Auto-switch: OFF"
    };
    let auto_item = CheckMenuItem::with_id(
        app,
        "auto_switch",
        auto_label,
        true,
        active_config.auto_switch,
        None::<&str>,
    )
    .map_err(|e| AppError::Config(format!("Failed to create auto-switch item: {e}")))?;
    builder = builder.item(&auto_item).separator();

    // Show main window
    let show_main =
        MenuItem::with_id(app, "show_main", "Open Main Window", true, None::<&str>)
            .map_err(|e| AppError::Config(format!("Failed to create show_main: {e}")))?;
    builder = builder.item(&show_main);

    // Quit
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)
        .map_err(|e| AppError::Config(format!("Failed to create quit: {e}")))?;
    builder = builder.item(&quit);

    builder
        .build()
        .map_err(|e| AppError::Config(format!("Failed to build menu: {e}")))
}

/// Handle tray menu events.
pub fn handle_tray_menu_event(app: &tauri::AppHandle, event_id: &str) {
    match event_id {
        "show_main" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "quit" => {
            app.exit(0);
        }
        "auto_switch" => {
            // Toggle auto-switch
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Some(state) = app.try_state::<AppState>() {
                    if let Ok(mut config) = state.db.get_active_config() {
                        config.auto_switch = !config.auto_switch;
                        let _ = state.db.set_active_config(&config);
                        refresh_tray_menu(&app);
                    }
                }
            });
        }
        _ if event_id.starts_with("gw_") => {
            // Switch gateway
            let gw_id = event_id[3..].to_string();
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Some(state) = app.try_state::<AppState>() {
                    handle_gateway_switch(&app, state.inner(), &gw_id);
                }
            });
        }
        _ => {}
    }
}

/// Switch to a different gateway via tray menu click.
fn handle_gateway_switch(app: &tauri::AppHandle, state: &AppState, gw_id: &str) {
    let gw = match state.db.get_gateway(gw_id) {
        Ok(Some(gw)) => gw,
        _ => return,
    };

    let config = match state.db.get_active_config() {
        Ok(c) => c,
        Err(_) => crate::database::ActiveConfig {
            gateway_id: None,
            key_id: None,
            key_name: None,
            key_value: None,
            claude_model: None,
            claude_small_model: None,
            codex_model: None,
            gemini_model: None,
            auto_switch: true,
            applied_at: None,
            last_switched_at: None,
        },
    };

    let api_key = config.key_value.as_deref().unwrap_or(&gw.auth_key);

    let _ = crate::config_writer::apply_all_configs(
        &gw.url,
        api_key,
        config.claude_model.as_deref(),
        config.claude_small_model.as_deref(),
        config.codex_model.as_deref(),
        config.gemini_model.as_deref(),
    );

    let now = chrono::Utc::now().to_rfc3339();
    let new_config = crate::database::ActiveConfig {
        gateway_id: Some(gw.id.clone()),
        key_id: config.key_id,
        key_name: config.key_name,
        key_value: config.key_value,
        claude_model: config.claude_model,
        claude_small_model: config.claude_small_model,
        codex_model: config.codex_model,
        gemini_model: config.gemini_model,
        auto_switch: config.auto_switch,
        applied_at: Some(now.clone()),
        last_switched_at: Some(now),
    };
    let _ = state.db.set_active_config(&new_config);

    refresh_tray_menu(app);

    use tauri::Emitter;
    log::info!("Tray switch to gateway: {} ({})", gw.id, gw.name);
    let event_payload = serde_json::json!({
        "gatewayId": gw.id,
        "gatewayName": gw.name,
    });
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.emit("gateway-switched", &event_payload);
    } else {
        let _ = app.emit("gateway-switched", &event_payload);
    }
}

/// Refresh the tray menu with current state.
pub fn refresh_tray_menu(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(new_menu) = create_tray_menu(app, state.inner()) {
            if let Some(tray) = app.tray_by_id("main") {
                let _ = tray.set_menu(Some(new_menu));
            }
        }
    }
}
