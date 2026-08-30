use std::sync::Arc;

use crate::database::{ActiveConfig, HealthCache};
use crate::gateway;
use crate::ipc::protocol::ModelSelection;
use crate::Database;

/// Minimum seconds between auto-switches to prevent flip-flopping.
const SWITCH_HYSTERESIS_SECS: i64 = 60;

/// Background health check loop.
/// Runs every 60 seconds, checks all gateways concurrently.
/// Implements priority-based auto-switch:
/// - Gateways are ordered by sort_order (drag-and-drop priority)
/// - Always use the first healthy gateway in the list
/// - If current gateway goes down, switch to next healthy one
/// - If a higher-priority gateway recovers, switch back (with hysteresis)
pub async fn health_check_loop(service: crate::Service) {
    log::info!("Health check loop started");
    loop {
        check_and_switch(&service).await;
        send_heartbeat(service.db.clone()).await;
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}

/// Run a single round of health checks and auto-switch if needed.
pub async fn check_and_switch(service: &crate::Service) {
    let db = service.db.clone();
    let sink = service.sink.clone();
    let gateways = match db.list_gateways() {
        Ok(gws) => gws,
        Err(_) => return,
    };

    if gateways.is_empty() {
        return;
    }

    // Check all gateways concurrently
    let mut handles = Vec::new();

    for gw in &gateways {
        let gw_id = gw.id.clone();
        let gw_url = gw.url.clone();
        let gw_auth = if gw.auth_key.is_empty() {
            gw.session_token.clone().unwrap_or_default()
        } else {
            gw.auth_key.clone()
        };
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
            let _ = db_ref.add_health_log(&gw_id, is_healthy, latency_ms);
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
    if let Ok(gateways_with_health) = db.list_gateways_with_health() {
        crate::events::emit_typed(&*sink, "health-updated", &gateways_with_health);
    }

    // Signal tray refresh (Tauri sink intercepts this and calls refresh_tray_menu)
    sink.emit(crate::TRAY_REFRESH_EVENT, serde_json::Value::Null);

    // Auto-switch logic
    let config = match db.get_active_config() {
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
            let current_is_healthy = results
                .iter()
                .any(|(id, healthy)| id == current_id && *healthy);

            if current_is_healthy {
                // Switching to higher-priority gateway — apply hysteresis
                if !should_switch_now(&config) {
                    log::info!(
                        "Hysteresis: skipping switch to {} (switched too recently)",
                        best.name
                    );
                    return;
                }
            }

            log::info!(
                "Auto-switch: {} -> {} (priority-based)",
                current_id,
                best.name
            );
            do_switch(service, &best.id, &config).await;
        }
        (Some(best), None) => {
            log::info!("Auto-switch: none -> {} (first healthy)", best.name);
            do_switch(service, &best.id, &config).await;
        }
        (None, Some(_)) => {
            log::warn!("All gateways are offline");
        }
        _ => {}
    }
}

/// Returns true if enough time has passed since the last switch.
fn should_switch_now(config: &ActiveConfig) -> bool {
    let Some(last) = config.last_switched_at.as_deref() else {
        return true;
    };
    let Ok(last_time) = chrono::DateTime::parse_from_rfc3339(last) else {
        return true;
    };
    let elapsed = chrono::Utc::now() - last_time.with_timezone(&chrono::Utc);
    elapsed.num_seconds() >= SWITCH_HYSTERESIS_SECS
}

pub async fn do_switch(
    service: &crate::Service,
    new_gw_id: &str,
    current_config: &ActiveConfig,
) {
    let gw = match service.db.get_gateway(new_gw_id) {
        Ok(Some(gw)) => gw,
        _ => return,
    };

    // Pick a key to apply: prefer the gateway's preferred_key_id (set by a
    // prior login/apply on this gateway), falling back to the current active
    // key only when it belongs to this same gateway. Without a key_id, we
    // cannot run a real apply — log + bail rather than silently rewriting the
    // DB row to a state that doesn't match disk.
    let Some(key_id_str) = crate::service::pick_key_id(&gw, Some(current_config)) else {
        log::warn!(
            "Auto-switch to {} skipped: no key_id available (no preferred key on gateway, \
             no current active key). Active config left unchanged.",
            gw.name
        );
        return;
    };

    let gw_uuid = match uuid::Uuid::parse_str(&gw.id) {
        Ok(u) => u,
        Err(e) => {
            log::warn!("Auto-switch: bad gateway uuid {}: {e}", gw.id);
            return;
        }
    };
    let key_uuid = match uuid::Uuid::parse_str(&key_id_str) {
        Ok(u) => u,
        Err(e) => {
            log::warn!("Auto-switch: bad key uuid {key_id_str}: {e}");
            return;
        }
    };

    // Use per-gateway model preferences captured at last apply.
    let models = ModelSelection {
        claude: gw.claude_model.clone(),
        claude_small: gw.claude_small_model.clone(),
        codex: gw.codex_model.clone(),
        gemini: gw.gemini_model.clone(),
    };

    if let Err(e) = service.set_active(gw_uuid, key_uuid, models).await {
        log::warn!(
            "Auto-switch apply failed for {}: {e}. Active config left unchanged.",
            gw.name
        );
        return;
    }

    // Update last_switched_at so hysteresis works.
    if let Ok(mut cfg) = service.db.get_active_config() {
        cfg.last_switched_at = Some(chrono::Utc::now().to_rfc3339());
        let _ = service.db.set_active_config(&cfg);
    }

    // Signal tray refresh (Tauri sink intercepts this)
    service.sink.emit(crate::TRAY_REFRESH_EVENT, serde_json::Value::Null);

    let event_payload = serde_json::json!({
        "gatewayId": gw.id,
        "gatewayName": gw.name,
    });
    service.sink.emit("gateway-switched", event_payload);
}

/// Send a heartbeat to the active gateway so the server knows this client is online.
/// Fire-and-forget: errors are logged but don't affect health check behavior.
pub async fn send_heartbeat(db: Arc<Database>) {
    log::info!("send_heartbeat called");

    let config = match db.get_active_config() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("Heartbeat skipped: no active config ({})", e);
            return;
        }
    };

    let gateway_id = match config.gateway_id.as_deref() {
        Some(id) => id.to_string(),
        None => {
            log::warn!("Heartbeat skipped: no gateway_id in config");
            return;
        }
    };

    // Only send if there's an API key set (needed for auth)
    let api_key = match config.key_value.as_deref() {
        Some(k) => k.to_string(),
        None => {
            // Fall back to gateway's auth_key or session_token
            match db.get_gateway(&gateway_id) {
                Ok(Some(gw)) => {
                    if !gw.auth_key.is_empty() {
                        gw.auth_key.clone()
                    } else {
                        gw.session_token.clone().unwrap_or_default()
                    }
                }
                _ => return,
            }
        }
    };

    let gw = match db.get_gateway(&gateway_id) {
        Ok(Some(gw)) => gw,
        _ => return,
    };

    // Get user-set client name (empty if not customized)
    let client_name = db
        .get_setting("client_name")
        .ok()
        .flatten()
        .unwrap_or_default();

    // Always send hostname separately; server will format the display name
    let hostname = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string());

    // Generate a stable client_id from settings or derive from hostname
    let client_id = db
        .get_setting("client_id")
        .ok()
        .flatten()
        .unwrap_or_else(|| {
            let id = uuid::Uuid::new_v4().to_string();
            let _ = db.set_setting("client_id", &id);
            id
        });

    let payload = serde_json::json!({
        "clientId": client_id,
        "clientName": client_name,
        "hostname": hostname,
        "gatewayUrl": gw.url,
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let url = format!("{}/api/heartbeat", gw.url.trim_end_matches('/'));
    log::info!("Sending heartbeat to {} (client_id: {}, client_name: '{}', hostname: '{}')",
        url, client_id, client_name, hostname);

    match client
        .post(&url)
        .header("x-api-key", &api_key)
        .header("content-type", "application/json")
        .json(&payload)
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                log::info!("Heartbeat successful to {} (status: {})", gw.name, status);
            } else {
                let body = resp.text().await.unwrap_or_default();
                log::warn!("Heartbeat failed to {} (status: {}, body: {})", gw.name, status, body);
            }
        }
        Err(e) => {
            log::warn!("Heartbeat network error to {} ({}): {}", gw.name, url, e);
        }
    }
}
