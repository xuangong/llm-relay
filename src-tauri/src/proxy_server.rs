use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderName, HeaderValue, Request, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use tauri::{Emitter, Manager};

use crate::database::Database;

pub const PROXY_PORT: u16 = 18080;
pub const PLACEHOLDER_KEY: &str = "llm-relay-local";

/// Consecutive error threshold before triggering auto-failover.
/// Set to 10 to allow more tolerance for transient errors.
const ERROR_FAILOVER_THRESHOLD: u32 = 10;

pub fn proxy_base_url() -> String {
    format!("http://127.0.0.1:{}", PROXY_PORT)
}

#[derive(Clone)]
pub struct ProxyState {
    db: Arc<Database>,
    app_handle: tauri::AppHandle,
    /// Counts consecutive request errors (network failures or 5xx).
    /// Reset to 0 on any successful response.
    consecutive_errors: Arc<AtomicU32>,
}

pub async fn start(db: Arc<Database>, app_handle: tauri::AppHandle) {
    let state = ProxyState {
        db,
        app_handle,
        consecutive_errors: Arc::new(AtomicU32::new(0)),
    };

    let app = Router::new()
        .fallback(forward)
        .with_state(state);

    let addr = format!("127.0.0.1:{}", PROXY_PORT);
    match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => {
            log::info!("Local proxy started on {}", addr);
            if let Err(e) = axum::serve(listener, app).await {
                log::error!("Proxy server stopped: {}", e);
            }
        }
        Err(e) => {
            log::error!("Failed to start proxy on {}: {}", addr, e);
        }
    }
}

async fn forward(State(state): State<ProxyState>, req: Request<Body>) -> Response {
    let start = std::time::Instant::now();
    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

    // Resolve active gateway + API key
    let config = match state.db.get_active_config() {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("{{\"error\":\"No active config: {}\"}}", e),
            )
                .into_response()
        }
    };

    let gateway_id = match config.gateway_id.as_deref() {
        Some(id) => id.to_string(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "{\"error\":\"No active gateway configured\"}",
            )
                .into_response()
        }
    };

    let gw = match state.db.get_gateway(&gateway_id) {
        Ok(Some(gw)) => gw,
        _ => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "{\"error\":\"Gateway not found\"}",
            )
                .into_response()
        }
    };

    let api_key = config
        .key_value
        .as_deref()
        .unwrap_or(&gw.auth_key)
        .to_string();

    // Build target URL
    let target_url = format!("{}{}", gw.url.trim_end_matches('/'), path);

    // Extract method + body
    let method = req.method().clone();
    let in_headers = req.headers().clone();
    let body_bytes = match axum::body::to_bytes(req.into_body(), usize::MAX).await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("{{\"error\":\"Failed to read body: {}\"}}", e),
            )
                .into_response()
        }
    };

    // Extract model name from request JSON body (best-effort)
    let model = extract_model(&body_bytes).unwrap_or_else(|| "unknown".to_string());

    // Build outbound request
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .unwrap_or_default();

    let reqwest_method =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);

    let mut req_builder = client.request(reqwest_method, &target_url);

    // Forward headers, stripping auth and hop-by-hop
    const SKIP: &[&str] = &[
        "host",
        "x-api-key",
        "authorization",
        "x-goog-api-key",
        "content-length",
        "transfer-encoding",
        "connection",
    ];
    for (name, value) in in_headers.iter() {
        if SKIP.contains(&name.as_str()) {
            continue;
        }
        if let Ok(val_str) = value.to_str() {
            req_builder = req_builder.header(name.as_str(), val_str);
        }
    }

    req_builder = req_builder.header("x-api-key", &api_key);

    if !body_bytes.is_empty() {
        req_builder = req_builder.body(body_bytes);
    }

    // Send and stream response back
    match req_builder.send().await {
        Ok(resp) => {
            let status_code = resp.status().as_u16();
            let latency_ms = start.elapsed().as_millis() as u64;
            let is_server_error = status_code >= 500 || status_code == 429;
            let is_any_error = status_code >= 400;

            if is_any_error {
                let detail = format!("HTTP {status_code}");
                let _ = state.db.add_traffic_log(&gateway_id, &path, status_code, latency_ms, Some(&detail));
            }

            if is_server_error {
                let count = state.consecutive_errors.fetch_add(1, Ordering::SeqCst) + 1;
                log::warn!(
                    "Proxy: {} {} → {}ms (consecutive errors: {})",
                    target_url, status_code, latency_ms, count
                );
                if count >= ERROR_FAILOVER_THRESHOLD {
                    state.consecutive_errors.store(0, Ordering::SeqCst);
                    try_proxy_failover(&state, &gateway_id, status_code).await;
                }
            } else {
                state.consecutive_errors.store(0, Ordering::SeqCst);
            }

            emit_traffic(&state, &path, status_code, latency_ms, &gateway_id);

            let is_sse = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|ct| ct.contains("text/event-stream"))
                .unwrap_or(false);

            let axum_status = StatusCode::from_u16(status_code).unwrap_or(StatusCode::OK);
            let resp_headers = resp.headers().clone();

            // Tap the response stream to record token usage in the background
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let db_usage = state.db.clone();
            let gw_usage = gateway_id.clone();
            let model_usage = model.clone();

            tokio::spawn(async move {
                let mut all = Vec::new();
                while let Some(chunk) = rx.recv().await {
                    all.extend_from_slice(&chunk);
                }
                if all.is_empty() {
                    return;
                }
                let (inp, out, cr, cc) = if is_sse {
                    parse_sse_tokens(&all)
                } else {
                    parse_json_tokens(&all)
                };
                if inp > 0 || out > 0 {
                    let _ = db_usage.record_usage(&gw_usage, &model_usage, inp, out, cr, cc);
                    log::debug!("Usage recorded: {} in={} out={} model={}", gw_usage, inp, out, model_usage);
                }
            });

            let stream = resp.bytes_stream().map(move |chunk| {
                if let Ok(ref b) = chunk {
                    let _ = tx.send(b.to_vec());
                }
                chunk.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
            });
            let body = Body::from_stream(stream);

            let mut response = Response::new(body);
            *response.status_mut() = axum_status;

            for (name, value) in resp_headers.iter() {
                if matches!(name.as_str(), "transfer-encoding" | "connection") {
                    continue;
                }
                if let (Ok(n), Ok(v)) = (
                    HeaderName::from_bytes(name.as_str().as_bytes()),
                    HeaderValue::from_bytes(value.as_bytes()),
                ) {
                    response.headers_mut().insert(n, v);
                }
            }

            response
        }
        Err(e) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            log::warn!("Proxy forward error → {}: {}", target_url, e);

            let err_str = e.to_string();
            let _ = state.db.add_traffic_log(&gateway_id, &path, 502, latency_ms, Some(&err_str));

            let count = state.consecutive_errors.fetch_add(1, Ordering::SeqCst) + 1;
            if count >= ERROR_FAILOVER_THRESHOLD {
                state.consecutive_errors.store(0, Ordering::SeqCst);
                try_proxy_failover(&state, &gateway_id, 502).await;
            }

            emit_traffic(&state, &path, 502, latency_ms, &gateway_id);

            (
                StatusCode::BAD_GATEWAY,
                format!("{{\"error\":\"Gateway unreachable: {}\"}}", e),
            )
                .into_response()
        }
    }
}

/// Extract the `model` field from a JSON request body.
fn extract_model(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("model")?.as_str().map(|s| s.to_string())
}

/// Parse token usage from a streaming SSE response body.
/// Handles Anthropic Messages format (message_start / message_delta events).
/// Also handles OpenAI Chat Completions streaming format (final chunk with usage).
fn parse_sse_tokens(data: &[u8]) -> (i64, i64, i64, i64) {
    let text = match std::str::from_utf8(data) {
        Ok(t) => t,
        Err(_) => return (0, 0, 0, 0),
    };

    let mut input: i64 = 0;
    let mut output: i64 = 0;
    let mut cache_read: i64 = 0;
    let mut cache_creation: i64 = 0;

    for line in text.lines() {
        let json_str = if let Some(s) = line.strip_prefix("data: ") { s } else { continue };
        if json_str == "[DONE]" { continue; }

        let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) else { continue };

        match v.get("type").and_then(|t| t.as_str()) {
            Some("message_start") => {
                // Anthropic: message_start.message.usage
                if let Some(usage) = v.pointer("/message/usage") {
                    input      += usage.get("input_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
                    cache_read += usage.get("cache_read_input_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
                    cache_creation += usage.get("cache_creation_input_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
                }
            }
            Some("message_delta") => {
                // Anthropic: message_delta.usage.output_tokens
                if let Some(usage) = v.get("usage") {
                    output += usage.get("output_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
                }
            }
            _ => {
                // OpenAI streaming: last chunk may contain usage object
                if let Some(usage) = v.get("usage") {
                    let pt = usage.get("prompt_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
                    let ct = usage.get("completion_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
                    if pt > 0 { input = pt; }
                    if ct > 0 { output = ct; }
                }
            }
        }
    }

    (input, output, cache_read, cache_creation)
}

/// Parse token usage from a non-streaming JSON response body.
/// Handles both Anthropic and OpenAI formats.
fn parse_json_tokens(data: &[u8]) -> (i64, i64, i64, i64) {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(data) else {
        return (0, 0, 0, 0);
    };
    let Some(usage) = v.get("usage") else {
        return (0, 0, 0, 0);
    };

    // Anthropic: input_tokens / output_tokens
    // OpenAI:    prompt_tokens / completion_tokens
    let input = usage.get("input_tokens").and_then(|x| x.as_i64())
        .or_else(|| usage.get("prompt_tokens").and_then(|x| x.as_i64()))
        .unwrap_or(0);
    let output = usage.get("output_tokens").and_then(|x| x.as_i64())
        .or_else(|| usage.get("completion_tokens").and_then(|x| x.as_i64()))
        .unwrap_or(0);
    let cache_read = usage.get("cache_read_input_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
    let cache_creation = usage.get("cache_creation_input_tokens").and_then(|x| x.as_i64()).unwrap_or(0);

    (input, output, cache_read, cache_creation)
}

/// Emit proxy-traffic event to the frontend.
fn emit_traffic(state: &ProxyState, path: &str, status: u16, latency_ms: u64, gateway_id: &str) {
    let payload = serde_json::json!({
        "path": path,
        "status": status,
        "latencyMs": latency_ms,
        "gatewayId": gateway_id,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    if let Some(window) = state.app_handle.get_webview_window("main") {
        let _ = window.emit("proxy-traffic", payload);
    }
}

/// Attempt to switch to the next healthy gateway after consecutive proxy errors.
async fn try_proxy_failover(state: &ProxyState, current_gateway_id: &str, error_status: u16) {
    let config = match state.db.get_active_config() {
        Ok(c) => c,
        Err(_) => return,
    };

    if !config.auto_switch {
        log::info!("Proxy failover skipped (auto_switch disabled), status={}", error_status);
        return;
    }

    let gateways = match state.db.list_gateways() {
        Ok(gws) => gws,
        Err(_) => return,
    };

    let next = gateways.iter().find(|gw| {
        if gw.id == current_gateway_id {
            return false;
        }
        state
            .db
            .get_health(&gw.id)
            .ok()
            .flatten()
            .map(|h| h.is_healthy)
            .unwrap_or(false)
    });

    if let Some(next_gw) = next {
        log::warn!(
            "Proxy failover: {} → {} after {} consecutive errors (status={})",
            current_gateway_id, next_gw.name, ERROR_FAILOVER_THRESHOLD, error_status
        );
        let app_state = state.app_handle.state::<crate::AppState>();
        crate::health::do_switch(app_state.inner(), &state.app_handle, &next_gw.id, &config).await;
    } else {
        log::warn!(
            "Proxy failover: no healthy alternative to {} (status={})",
            current_gateway_id, error_status
        );
    }
}
