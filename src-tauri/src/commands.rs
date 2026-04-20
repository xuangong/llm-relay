use tauri::State;

use llm_relay_core::config_writer;
use llm_relay_core::database::{ActiveConfig, Gateway, GatewayWithHealth, HealthCache, HealthLogEntry, TrafficLogEntry, UsageSummary};
use crate::gateway::{self, ApiKey, DeviceCodeResponse, DevicePollResponse, LoginResult, ModelList};
use crate::proxy_server;
use crate::AppState;

// ─── Gateway CRUD ───

#[tauri::command]
pub async fn add_gateway(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
    name: String,
    url: String,
    auth_key: String,
    session_token: Option<String>,
    user_id: Option<String>,
    user_name: Option<String>,
) -> Result<Gateway, String> {
    let gw = Gateway {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        url,
        auth_key,
        is_admin: false,
        session_token,
        user_id,
        user_name,
        sort_order: 999,
        created_at: chrono::Utc::now().to_rfc3339(),
        claude_model: None,
        claude_small_model: None,
        codex_model: None,
        gemini_model: None,
    };
    state.db.add_gateway(&gw).map_err(|e| e.to_string())?;

    // Immediately check health for the new gateway
    let gw_clone = gw.clone();
    let db_clone = state.db.clone();
    let app_handle_clone = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        let (is_healthy, latency_ms, model_count) =
            gateway::health_check(&gw_clone.url, &gw_clone.auth_key).await;

        let now = chrono::Utc::now().to_rfc3339();
        let health = HealthCache {
            gateway_id: gw_clone.id.clone(),
            is_healthy,
            latency_ms,
            model_count,
            last_checked: Some(now),
        };

        let _ = db_clone.update_health(&health);

        // Emit health update event
        if let Ok(gateways_with_health) = db_clone.list_gateways_with_health() {
            use tauri::Emitter;
            let _ = app_handle_clone.emit("health-updated", &gateways_with_health);
        }

        // Refresh tray menu
        crate::tray::refresh_tray_menu(&app_handle_clone);
    });

    Ok(gw)
}

#[tauri::command]
pub async fn list_gateways(
    state: State<'_, AppState>,
) -> Result<Vec<GatewayWithHealth>, String> {
    state
        .db
        .list_gateways_with_health()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_gateway(
    state: State<'_, AppState>,
    id: String,
    name: String,
    url: String,
    auth_key: String,
) -> Result<(), String> {
    state
        .db
        .update_gateway(&id, &name, &url, &auth_key)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_gateway(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
    id: String,
) -> Result<(), String> {
    state.db.delete_gateway(&id).map_err(|e| e.to_string())?;

    // Refresh tray menu after deletion
    crate::tray::refresh_tray_menu(&app_handle);

    Ok(())
}

#[tauri::command]
pub async fn reorder_gateways(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
    ids: Vec<String>,
) -> Result<(), String> {
    state
        .db
        .reorder_gateways(&ids)
        .map_err(|e| e.to_string())?;

    // Refresh tray menu after reordering
    crate::tray::refresh_tray_menu(&app_handle);

    Ok(())
}

// ─── Gateway API ───

#[tauri::command]
pub async fn login_gateway(url: String, key: String) -> Result<LoginResult, String> {
    let result = gateway::login(&url, &key).await.map_err(|e| e.to_string())?;

    if !result.ok {
        return Err("Login failed".to_string());
    }

    Ok(result)
}

#[tauri::command]
pub async fn fetch_keys(
    state: State<'_, AppState>,
    gateway_id: String,
) -> Result<Vec<ApiKey>, String> {
    let gw = state
        .db
        .get_gateway(&gateway_id)
        .map_err(|e| e.to_string())?
        .ok_or("Gateway not found")?;

    // Use session_token if available, otherwise use auth_key
    let auth = gw.session_token.as_deref().unwrap_or(&gw.auth_key);
    gateway::fetch_keys(&gw.url, auth)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn fetch_models(
    state: State<'_, AppState>,
    gateway_id: String,
    key_value: Option<String>,
) -> Result<ModelList, String> {
    let gw = state
        .db
        .get_gateway(&gateway_id)
        .map_err(|e| e.to_string())?
        .ok_or("Gateway not found")?;

    let auth = key_value.as_deref()
        .or(gw.session_token.as_deref())
        .unwrap_or(&gw.auth_key);
    gateway::fetch_models(&gw.url, auth)
        .await
        .map_err(|e| e.to_string())
}

// ─── Health Check ───

#[tauri::command]
pub async fn check_all_health(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<Vec<GatewayWithHealth>, String> {
    crate::health::check_and_switch(&state, &app_handle).await;
    state
        .db
        .list_gateways_with_health()
        .map_err(|e| e.to_string())
}

/// Test heartbeat manually - returns debug info
#[tauri::command]
pub async fn test_heartbeat(state: State<'_, AppState>) -> Result<String, String> {
    log::info!("Manual heartbeat test triggered");
    crate::health::send_heartbeat(state.inner()).await;
    Ok("Heartbeat sent - check gateway's Clients panel or logs".to_string())
}

// ─── Config ───

#[tauri::command]
pub async fn apply_config(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
    gateway_id: String,
    key_id: Option<String>,
    key_name: Option<String>,
    key_value: Option<String>,
    claude_model: Option<String>,
    claude_small_model: Option<String>,
    codex_model: Option<String>,
    gemini_model: Option<String>,
) -> Result<(), String> {
    let gw = state
        .db
        .get_gateway(&gateway_id)
        .map_err(|e| e.to_string())?
        .ok_or("Gateway not found")?;

    // Load existing config so omitted fields (e.g. on a plain re-apply) are
    // preserved instead of being wiped out.
    let existing = state.db.get_active_config().ok();
    let same_gateway = existing
        .as_ref()
        .and_then(|c| c.gateway_id.as_deref())
        .map(|id| id == gateway_id)
        .unwrap_or(false);

    // Merge priority for each model field:
    //   1. Value passed from the UI (user just picked it)
    //   2. Per-gateway stored model (this gateway's own remembered choice)
    //   3. Active config value — only if we are re-applying the SAME gateway
    //      (otherwise those values belong to a different gateway and must not leak)
    let merged_key_id = key_id.or_else(|| if same_gateway { existing.as_ref().and_then(|c| c.key_id.clone()) } else { None });
    let merged_key_name = key_name.or_else(|| if same_gateway { existing.as_ref().and_then(|c| c.key_name.clone()) } else { None });
    let merged_key_value = key_value.or_else(|| if same_gateway { existing.as_ref().and_then(|c| c.key_value.clone()) } else { None });

    let merged_claude = claude_model.clone()
        .or_else(|| gw.claude_model.clone())
        .or_else(|| if same_gateway { existing.as_ref().and_then(|c| c.claude_model.clone()) } else { None });
    let merged_claude_small = claude_small_model.clone()
        .or_else(|| gw.claude_small_model.clone())
        .or_else(|| if same_gateway { existing.as_ref().and_then(|c| c.claude_small_model.clone()) } else { None });
    let merged_codex = codex_model.clone()
        .or_else(|| gw.codex_model.clone())
        .or_else(|| if same_gateway { existing.as_ref().and_then(|c| c.codex_model.clone()) } else { None });
    let merged_gemini = gemini_model.clone()
        .or_else(|| gw.gemini_model.clone())
        .or_else(|| if same_gateway { existing.as_ref().and_then(|c| c.gemini_model.clone()) } else { None });

    let proxy_url = proxy_server::proxy_base_url();

    config_writer::apply_all_configs(
        &proxy_url,
        proxy_server::PLACEHOLDER_KEY,
        merged_claude.as_deref(),
        merged_claude_small.as_deref(),
        merged_codex.as_deref(),
        merged_gemini.as_deref(),
    )
    .map_err(|e| e.to_string())?;

    // Save active config
    let now = chrono::Utc::now().to_rfc3339();
    let config = ActiveConfig {
        gateway_id: Some(gateway_id.clone()),
        key_id: merged_key_id,
        key_name: merged_key_name,
        key_value: merged_key_value,
        claude_model: merged_claude,
        claude_small_model: merged_claude_small,
        codex_model: merged_codex,
        gemini_model: merged_gemini,
        auto_switch: existing.as_ref().map(|c| c.auto_switch).unwrap_or(true),
        applied_at: Some(now.clone()),
        last_switched_at: Some(now),
    };
    state
        .db
        .set_active_config(&config)
        .map_err(|e| e.to_string())?;

    // Persist the resolved models on the gateway row itself so they survive
    // switching to another gateway and back.
    state
        .db
        .update_gateway_models(
            &gateway_id,
            config.claude_model.as_deref(),
            config.claude_small_model.as_deref(),
            config.codex_model.as_deref(),
            config.gemini_model.as_deref(),
        )
        .map_err(|e| e.to_string())?;

    // Update tray menu
    crate::tray::refresh_tray_menu(&app_handle);

    Ok(())
}

#[tauri::command]
pub fn read_current_config() -> Result<config_writer::CurrentCliConfig, String> {
    config_writer::read_all_configs().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn clear_config(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    config_writer::clear_all_configs().map_err(|e| e.to_string())?;

    // Clear active config in DB
    let config = ActiveConfig {
        gateway_id: None,
        key_id: None,
        key_name: None,
        key_value: None,
        claude_model: None,
        claude_small_model: None,
        codex_model: None,
        gemini_model: None,
        auto_switch: state
            .db
            .get_active_config()
            .map(|c| c.auto_switch)
            .unwrap_or(true),
        applied_at: None,
        last_switched_at: None,
    };
    state
        .db
        .set_active_config(&config)
        .map_err(|e| e.to_string())?;

    crate::tray::refresh_tray_menu(&app_handle);

    Ok(())
}

// ─── Settings ───

#[tauri::command]
pub async fn get_active_config_cmd(
    state: State<'_, AppState>,
) -> Result<ActiveConfig, String> {
    state.db.get_active_config().map_err(|e| e.to_string())
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub auto_switch: bool,
}

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, String> {
    let config = state.db.get_active_config().map_err(|e| e.to_string())?;
    Ok(AppSettings {
        auto_switch: config.auto_switch,
    })
}

#[tauri::command]
pub async fn update_settings(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
    auto_switch: bool,
) -> Result<(), String> {
    let mut config = state.db.get_active_config().map_err(|e| e.to_string())?;
    config.auto_switch = auto_switch;
    state
        .db
        .set_active_config(&config)
        .map_err(|e| e.to_string())?;

    crate::tray::refresh_tray_menu(&app_handle);

    Ok(())
}

// ─── Tray ───

#[tauri::command]
pub async fn update_tray_menu(app_handle: tauri::AppHandle) -> Result<(), String> {
    crate::tray::refresh_tray_menu(&app_handle);
    Ok(())
}

// ─── Health Log ───

#[tauri::command]
pub async fn get_health_log(
    state: State<'_, AppState>,
    gateway_id: String,
) -> Result<Vec<HealthLogEntry>, String> {
    state.db.get_health_log(&gateway_id, 1440).map_err(|e| e.to_string())
}

// ─── Client Name ───

#[tauri::command]
pub async fn get_client_name(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state
        .db
        .get_setting("client_name")
        .map_err(|e| e.to_string())?
        .unwrap_or_default())
}

#[tauri::command]
pub async fn set_client_name(
    state: State<'_, AppState>,
    name: String,
) -> Result<(), String> {
    state.db.set_setting("client_name", &name).map_err(|e| e.to_string())
}

// ─── Autostart ───

#[tauri::command]
pub async fn get_autostart(app_handle: tauri::AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app_handle.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_autostart(app_handle: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let mgr = app_handle.autolaunch();
    if enabled {
        mgr.enable().map_err(|e| e.to_string())
    } else {
        mgr.disable().map_err(|e| e.to_string())
    }
}

// ─── Traffic Log ───

#[tauri::command]
pub async fn get_traffic_logs(
    state: State<'_, AppState>,
    gateway_id: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<TrafficLogEntry>, String> {
    state
        .db
        .get_traffic_log(gateway_id.as_deref(), limit.unwrap_or(200))
        .map_err(|e| e.to_string())
}

// ─── Usage Stats ───

#[tauri::command]
pub async fn get_usage_stats(
    state: State<'_, AppState>,
    period: String,
    gateway_id: Option<String>,
) -> Result<Vec<UsageSummary>, String> {
    state
        .db
        .get_usage_stats(gateway_id.as_deref(), &period)
        .map_err(|e| e.to_string())
}

// ─── Device Authorization Flow ───

#[tauri::command]
pub async fn start_device_login(url: String) -> Result<DeviceCodeResponse, String> {
    let result = gateway::request_device_code(&url)
        .await
        .map_err(|e| e.to_string())?;
    Ok(result)
}

#[tauri::command]
pub async fn poll_device_login(
    url: String,
    device_code: String,
) -> Result<DevicePollResponse, String> {
    let result = gateway::poll_device_code(&url, &device_code)
        .await
        .map_err(|e| e.to_string())?;
    Ok(result)
}

#[tauri::command]
pub async fn fetch_keys_with_token(
    url: String,
    token: String,
) -> Result<Vec<ApiKey>, String> {
    gateway::fetch_keys(&url, &token)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_url(url: String) -> Result<(), String> {
    open::that(&url).map_err(|e| e.to_string())
}
