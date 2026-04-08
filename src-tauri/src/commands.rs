use tauri::State;

use crate::config_writer;
use crate::database::{ActiveConfig, Gateway, GatewayWithHealth};
use crate::gateway::{self, ApiKey, LoginResult, ModelList};
use crate::AppState;

// ─── Gateway CRUD ───

#[tauri::command]
pub async fn add_gateway(
    state: State<'_, AppState>,
    name: String,
    url: String,
    auth_key: String,
) -> Result<Gateway, String> {
    let gw = Gateway {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        url,
        auth_key,
        is_admin: false,
        session_token: None,
        sort_order: 999,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    state.db.add_gateway(&gw).map_err(|e| e.to_string())?;
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
pub async fn delete_gateway(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.db.delete_gateway(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn reorder_gateways(
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<(), String> {
    state
        .db
        .reorder_gateways(&ids)
        .map_err(|e| e.to_string())
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
) -> Result<ModelList, String> {
    let gw = state
        .db
        .get_gateway(&gateway_id)
        .map_err(|e| e.to_string())?
        .ok_or("Gateway not found")?;

    let auth = gw.session_token.as_deref().unwrap_or(&gw.auth_key);
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

    // Use provided key_value, or fall back to gateway's auth_key
    let api_key = key_value.as_deref().unwrap_or(&gw.auth_key);

    config_writer::apply_all_configs(
        &gw.url,
        api_key,
        claude_model.as_deref(),
        claude_small_model.as_deref(),
        codex_model.as_deref(),
        gemini_model.as_deref(),
    )
    .map_err(|e| e.to_string())?;

    // Save active config
    let now = chrono::Utc::now().to_rfc3339();
    let config = ActiveConfig {
        gateway_id: Some(gateway_id),
        key_id,
        key_name,
        key_value,
        claude_model,
        claude_small_model,
        codex_model,
        gemini_model,
        auto_switch: state
            .db
            .get_active_config()
            .map(|c| c.auto_switch)
            .unwrap_or(true),
        applied_at: Some(now),
    };
    state
        .db
        .set_active_config(&config)
        .map_err(|e| e.to_string())?;

    // Update tray menu
    crate::tray::refresh_tray_menu(&app_handle);

    Ok(())
}

#[tauri::command]
pub async fn read_current_config() -> Result<config_writer::CurrentCliConfig, String> {
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
