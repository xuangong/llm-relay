use tauri::Emitter;

use crate::config_writer;
use crate::database::{ActiveConfig, HealthCache};
use crate::gateway;
use crate::tray;
use crate::AppState;

/// Background health check loop.
/// Runs every 30 seconds, checks all gateways concurrently.
/// Implements priority-based auto-switch:
/// - Gateways are ordered by sort_order (drag-and-drop priority)
/// - Always use the first healthy gateway in the list
/// - If current gateway goes down, switch to next healthy one
/// - If a higher-priority gateway recovers, switch back
pub async fn health_check_loop(state: &AppState, app_handle: &tauri::AppHandle) {
    loop {
        check_and_switch(state, app_handle).await;
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    }
}

/// Run a single round of health checks and auto-switch if needed.
pub async fn check_and_switch(state: &AppState, app_handle: &tauri::AppHandle) {
    let gateways = match state.db.list_gateways() {
        Ok(gws) => gws,
        Err(_) => return,
    };

    if gateways.is_empty() {
        return;
    }

    // Check all gateways concurrently
    let db = state.db.clone();
    let mut handles = Vec::new();

    for gw in &gateways {
        let gw_id = gw.id.clone();
        let gw_url = gw.url.clone();
        let gw_auth = gw.auth_key.clone();
        let db_ref = db.clone();

        handles.push(tokio::spawn(async move {
            let (is_healthy, latency_ms, model_count) =
                gateway::health_check(&gw_url, &gw_auth).await;

            let now = chrono::Utc::now().to_rfc3339();
            let health = HealthCache {
                gateway_id: gw_id.clone(),
                is_healthy,
                latency_ms,
                model_count,
                last_checked: Some(now),
            };

            let _ = db_ref.update_health(&health);
            (gw_id, is_healthy)
        }));
    }

    // Wait for all health checks
    let mut results = Vec::new();
    for handle in handles {
        if let Ok(result) = handle.await {
            results.push(result);
        }
    }

    // Emit health-updated event
    if let Ok(gateways_with_health) = state.db.list_gateways_with_health() {
        let _ = app_handle.emit("health-updated", &gateways_with_health);
    }

    // Auto-switch logic
    let config = match state.db.get_active_config() {
        Ok(c) => c,
        Err(_) => return,
    };

    if !config.auto_switch {
        return;
    }

    // Find the first healthy gateway by sort_order (priority)
    let best_healthy = gateways.iter().find(|gw| {
        results
            .iter()
            .any(|(id, healthy)| id == &gw.id && *healthy)
    });

    let current_gw_id = config.gateway_id.as_deref();

    match (best_healthy, current_gw_id) {
        (Some(best), Some(current_id)) if best.id != current_id => {
            // Switch to higher-priority healthy gateway
            log::info!(
                "Auto-switch: {} -> {} (priority-based)",
                current_id,
                best.name
            );
            do_switch(state, app_handle, &best.id, &config).await;
        }
        (Some(best), None) => {
            // No current gateway set, use best available
            log::info!("Auto-switch: none -> {} (first healthy)", best.name);
            do_switch(state, app_handle, &best.id, &config).await;
        }
        (None, Some(_)) => {
            // All gateways down, nothing to do
            log::warn!("All gateways are offline");
        }
        _ => {
            // Current gateway is already the best, do nothing
        }
    }
}

async fn do_switch(
    state: &AppState,
    app_handle: &tauri::AppHandle,
    new_gw_id: &str,
    current_config: &ActiveConfig,
) {
    let gw = match state.db.get_gateway(new_gw_id) {
        Ok(Some(gw)) => gw,
        _ => return,
    };

    // Rewrite CLI configs with the new gateway
    let api_key = current_config
        .key_value
        .as_deref()
        .unwrap_or(&gw.auth_key);

    let _ = config_writer::apply_all_configs(
        &gw.url,
        api_key,
        current_config.claude_model.as_deref(),
        current_config.claude_small_model.as_deref(),
        current_config.codex_model.as_deref(),
        current_config.gemini_model.as_deref(),
    );

    // Update active config in DB
    let now = chrono::Utc::now().to_rfc3339();
    let new_config = ActiveConfig {
        gateway_id: Some(gw.id.clone()),
        key_id: current_config.key_id.clone(),
        key_name: current_config.key_name.clone(),
        key_value: current_config.key_value.clone(),
        claude_model: current_config.claude_model.clone(),
        claude_small_model: current_config.claude_small_model.clone(),
        codex_model: current_config.codex_model.clone(),
        gemini_model: current_config.gemini_model.clone(),
        auto_switch: current_config.auto_switch,
        applied_at: Some(now),
    };
    let _ = state.db.set_active_config(&new_config);

    // Update tray menu
    tray::refresh_tray_menu(app_handle);

    // Emit gateway-switched event
    let _ = app_handle.emit("gateway-switched", &serde_json::json!({
        "gatewayId": gw.id,
        "gatewayName": gw.name,
    }));
}
