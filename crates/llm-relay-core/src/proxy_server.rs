use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{HeaderName, HeaderValue, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
    Router,
};
use futures_util::StreamExt;

use crate::database::{ActiveConfig, Database};
use crate::events::SharedEventSink;

pub const PLACEHOLDER_KEY: &str = "llm-relay-local";

/// Consecutive error threshold before triggering auto-failover.
/// Set to 10 to allow more tolerance for transient errors.
const ERROR_FAILOVER_THRESHOLD: u32 = 10;

pub fn proxy_base_url() -> String {
    format!("http://127.0.0.1:{}", crate::paths::proxy_port())
}

#[derive(Clone)]
pub struct ProxyState {
    db: Arc<Database>,
    /// Kept for backwards-compatible `ProxyState::new` signature; not
    /// read directly — switch coordination is now owned by
    /// `Service::set_active` via its own lock.
    #[allow(dead_code)]
    switch_lock: Arc<tokio::sync::Mutex<()>>,
    sink: SharedEventSink,
    /// Set once `start_with_listener(s)` is given a `Service`. Used by
    /// `try_proxy_failover` to route auto-switch through the real apply
    /// pipeline. None only when constructed via `ProxyState::new` (older
    /// path) — failover degrades to a no-op in that case (logs a warning).
    service: Option<Arc<crate::Service>>,
    /// Counts consecutive request errors (network failures or 5xx).
    /// Reset to 0 on any successful response.
    consecutive_errors: Arc<AtomicU32>,
}

impl ProxyState {
    /// Construct from the same three Arcs that `Service` owns. Used by
    /// `start_with_listeners` so the proxy can be brought up *before* the
    /// `Service { proxy }` field is filled in (resolves the would-be cycle:
    /// Service.proxy is set via `with_proxy(handle)` once start returns).
    pub fn new(
        db: Arc<Database>,
        switch_lock: Arc<tokio::sync::Mutex<()>>,
        sink: SharedEventSink,
    ) -> Self {
        Self {
            db,
            switch_lock,
            sink,
            service: None,
            consecutive_errors: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Attach a Service so failover can run real apply. Call after the
    /// Service has been constructed.
    pub fn with_service(mut self, service: Arc<crate::Service>) -> Self {
        self.service = Some(service);
        self
    }
}

async fn relay_ping() -> &'static str {
    "ok"
}

async fn relay_reserved() -> (StatusCode, &'static str) {
    (StatusCode::NOT_FOUND, "unknown relay endpoint")
}

/// Build the axum Router shared by every listener.
///
/// `/_relay/*` routes are local — registered explicitly before
/// `.fallback(forward)` so unknown reserved-namespace paths return a
/// local 404 instead of leaking to the upstream gateway. New
/// `/_relay/*` endpoints register before the `{*rest}` wildcard.
pub fn build_router(state: ProxyState) -> Router {
    Router::new()
        .route("/_relay/ping", get(relay_ping))
        .route("/_relay/{*rest}", any(relay_reserved))
        .fallback(forward)
        .with_state(state)
}

pub async fn start(service: crate::Service) {
    start_with_listener(service, None).await
}

/// Start the proxy server, optionally with a pre-bound listener.
///
/// Passing a `std::net::TcpListener` lets the agent's `LifecycleGuard` bind the
/// port atomically with respect to the file lock — avoiding a TOCTOU window
/// where another process could grab port 18080 between the lifecycle probe
/// (which dropped its listener) and the proxy's own bind.
pub async fn start_with_listener(service: crate::Service, listener: Option<std::net::TcpListener>) {
    let state = ProxyState {
        db: service.db.clone(),
        switch_lock: service.switch_lock.clone(),
        sink: service.sink.clone(),
        service: None,
        consecutive_errors: Arc::new(AtomicU32::new(0)),
    };

    let app = build_router(state);

    let addr = format!("127.0.0.1:{}", crate::paths::proxy_port());
    let tokio_listener = if let Some(std_l) = listener {
        // Caller (lifecycle) already owns the bind. Convert std → tokio.
        if let Err(e) = std_l.set_nonblocking(true) {
            log::error!("Failed to set listener nonblocking on {}: {}", addr, e);
            return;
        }
        match tokio::net::TcpListener::from_std(std_l) {
            Ok(l) => l,
            Err(e) => {
                log::error!("Failed to wrap pre-bound listener on {}: {}", addr, e);
                return;
            }
        }
    } else {
        match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                log::error!("Failed to start proxy on {}: {}", addr, e);
                return;
            }
        }
    };
    log::info!("Local proxy started on {}", addr);
    if let Err(e) = axum::serve(tokio_listener, app).await {
        log::error!("Proxy server stopped: {}", e);
    }
}

// Restartable local/WSL listeners. Production starts with ProxyHandle::stopped;
// start_with_listeners also supports callers that already own bound sockets.

use std::net::IpAddr;
use std::sync::Mutex as StdMutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

// Wake and close sockets on stop, including clients that stopped reading and
// therefore prevent the response body from being polled for cancellation.
struct CancelIo {
    stream: tokio::net::TcpStream,
    cancelled: std::pin::Pin<Box<tokio_util::sync::WaitForCancellationFutureOwned>>,
    stopped: bool,
}

impl CancelIo {
    fn check(&mut self, cx: &mut std::task::Context<'_>) -> std::io::Result<()> {
        use std::future::Future;
        if self.stopped || self.cancelled.as_mut().poll(cx).is_ready() {
            self.stopped = true;
            Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "Relay stopped",
            ))
        } else {
            Ok(())
        }
    }
}

impl tokio::io::AsyncRead for CancelIo {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.check(cx)?;
        std::pin::Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for CancelIo {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        self.check(cx)?;
        std::pin::Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.check(cx)?;
        std::pin::Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

struct CancelListener {
    listener: tokio::net::TcpListener,
    token: CancellationToken,
}

impl axum::serve::Listener for CancelListener {
    type Io = CancelIo;
    type Addr = std::net::SocketAddr;
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.listener.accept().await {
                Ok((stream, addr)) => {
                    return (
                        CancelIo {
                            stream,
                            cancelled: Box::pin(self.token.clone().cancelled_owned()),
                            stopped: false,
                        },
                        addr,
                    )
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
            }
        }
    }
    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

/// Owns the running proxy. Drop alone does NOT stop the server — call
/// `shutdown()` and `.await` it.
pub struct ProxyHandle {
    lifecycle: tokio::sync::Mutex<()>,
    running: AtomicBool,
    primary: StdMutex<Option<BoundListener>>,
    wsl: StdMutex<Option<BoundListener>>,
    port: u16,
    state: ProxyState,
}

struct BoundListener {
    ip: IpAddr,
    token: CancellationToken,
    join: JoinHandle<()>,
}

// Cancel both pending upstream requests and streaming response bodies. Graceful
// listener shutdown alone would let an existing inference stream keep proxying.
async fn cancel_on_stop(
    State(token): State<CancellationToken>,
    request: Request<Body>,
    next: axum::middleware::Next,
) -> Response {
    tokio::select! {
        biased;
        _ = token.cancelled() => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        response = next.run(request) => {
            let (parts, body) = response.into_parts();
            let stream = body.into_data_stream().take_until(token.cancelled_owned());
            Response::from_parts(parts, Body::from_stream(stream))
        }
    }
}

fn serve_listener(
    state: ProxyState,
    listener: std::net::TcpListener,
) -> Result<BoundListener, crate::AppError> {
    serve_router(build_router(state), listener)
}

fn serve_router(
    router: Router,
    listener: std::net::TcpListener,
) -> Result<BoundListener, crate::AppError> {
    let ip = listener.local_addr()?.ip();
    listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(listener)?;
    let token = CancellationToken::new();
    let task_token = token.clone();
    let app = router.layer(axum::middleware::from_fn_with_state(
        token.clone(),
        cancel_on_stop,
    ));
    let listener = CancelListener {
        listener,
        token: token.clone(),
    };
    let join = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(task_token.cancelled_owned())
            .await;
    });
    Ok(BoundListener { ip, token, join })
}

impl ProxyHandle {
    pub fn stopped(state: ProxyState, port: u16) -> Arc<Self> {
        Arc::new(Self {
            lifecycle: tokio::sync::Mutex::new(()),
            running: AtomicBool::new(false),
            primary: StdMutex::new(None),
            wsl: StdMutex::new(None),
            port,
            state,
        })
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
            && self
                .primary
                .lock()
                .unwrap()
                .as_ref()
                .map_or(true, |p| !p.join.is_finished())
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    fn emit_status(&self) {
        self.state.sink.emit(
            "relay-status-changed",
            serde_json::json!({
                "running": self.is_running(), "port": self.port,
            }),
        );
    }

    /// Start on Use, before writing any CLI configuration. Bind errors are
    /// returned to the user; a disabled app does not reserve the proxy port.
    pub async fn start(&self) -> Result<(), crate::AppError> {
        let _guard = self.lifecycle.lock().await;
        if self.is_running() {
            return Ok(());
        }
        let listener = std::net::TcpListener::bind(("127.0.0.1", self.port))
            .map_err(|e| crate::AppError::Config(format!("Local proxy port {}: {e}", self.port)))?;
        *self.primary.lock().unwrap() = Some(serve_listener(self.state.clone(), listener)?);
        self.running.store(true, Ordering::Release);
        if let Some(ip) = tokio::task::spawn_blocking(crate::wsl::network::find_wsl_gateway_ip)
            .await
            .ok()
            .flatten()
        {
            match std::net::TcpListener::bind((ip, self.port))
                .map_err(crate::AppError::from)
                .and_then(|listener| serve_listener(self.state.clone(), listener))
            {
                Ok(bound) => *self.wsl.lock().unwrap() = Some(bound),
                Err(error) => log::warn!("WSL listener startup: {error}"),
            }
        }
        self.emit_status();
        Ok(())
    }

    /// Stop both listeners and cancel active requests/streams before returning.
    /// Serialized with start/rebind so a WSL refresh cannot reopen a stopped server.
    pub async fn shutdown(self: Arc<Self>) {
        let _guard = self.lifecycle.lock().await;
        let primary = self.primary.lock().unwrap().take();
        let wsl = self.wsl.lock().unwrap().take();
        for bound in [&primary, &wsl].into_iter().flatten() {
            bound.token.cancel();
        }
        for bound in [primary, wsl].into_iter().flatten() {
            let _ = bound.join.await;
        }
        self.running.store(false, Ordering::Release);
        self.emit_status();
    }

    pub async fn rebind_wsl(
        self: &Arc<Self>,
        new_ip: Option<IpAddr>,
    ) -> Result<(), crate::AppError> {
        let _guard = self.lifecycle.lock().await;
        let old = self.wsl.lock().unwrap().take();
        if let Some(old) = old {
            old.token.cancel();
            let _ = old.join.await;
        }
        if !self.is_running() {
            return Ok(());
        }
        let Some(ip) = new_ip else {
            return Ok(());
        };
        let listener = std::net::TcpListener::bind((ip, self.port)).map_err(|e| {
            crate::AppError::Config(format!("WSL bind {ip}:{} failed: {e}", self.port))
        })?;
        *self.wsl.lock().unwrap() = Some(serve_listener(self.state.clone(), listener)?);
        Ok(())
    }

    pub fn wsl_ip(&self) -> Option<IpAddr> {
        self.wsl.lock().unwrap().as_ref().map(|w| w.ip)
    }
}

/// Compatibility entry point for callers holding pre-bound listeners.
pub async fn start_with_listeners(
    state: ProxyState,
    primary: std::net::TcpListener,
    initial_wsl: Option<(IpAddr, std::net::TcpListener)>,
) -> Arc<ProxyHandle> {
    let port = primary.local_addr().expect("primary address").port();
    let handle = ProxyHandle::stopped(state, port);
    *handle.primary.lock().unwrap() =
        Some(serve_listener(handle.state.clone(), primary).expect("primary listener"));
    handle.running.store(true, Ordering::Release);
    if let Some((_, listener)) = initial_wsl {
        match serve_listener(handle.state.clone(), listener) {
            Ok(bound) => *handle.wsl.lock().unwrap() = Some(bound),
            Err(error) => log::warn!("WSL listener: {error}"),
        }
    }
    handle
}

async fn forward(State(state): State<ProxyState>, req: Request<Body>) -> Response {
    let start = std::time::Instant::now();
    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

    // Resolve one consistent routing snapshot. Gateway switches hold this same
    // lock across external config writes and DB publication; wait here only until
    // the active gateway/key/model tuple is fully published, then release it
    // before reading the request body or doing any network I/O.
    let switch_guard = state.switch_lock.lock().await;
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

    // Refuse to forward with an empty bearer — upstream would silently 401.
    // Surface the issue locally instead so the user knows to log in.
    if api_key.is_empty() {
        return (
            StatusCode::UNAUTHORIZED,
            "{\"error\":\"gateway requires login: run device-code flow first\"}",
        )
            .into_response();
    }

    // Build target URL while the routing snapshot is protected, then let
    // switches proceed while this request is read and forwarded.
    let target_url = format!("{}{}", gw.url.trim_end_matches('/'), path);
    drop(switch_guard);

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

    // Extract model name from request JSON body (best-effort). Stays an
    // Option: plenty of requests legitimately name no model.
    let model = extract_model(&body_bytes, &path);

    // Skip usage tracking for requests without a body (e.g., GET /models)
    let should_track_usage = !body_bytes.is_empty();

    // Inject stream_options for OpenAI-compatible APIs to get usage data in streaming responses
    // Only inject for OpenAI/compatible APIs, NOT for Anthropic (which doesn't support stream_options)
    let body_bytes = inject_stream_options(&body_bytes, &path);

    // Build outbound request
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600)) // 10 minutes total timeout (same as cc-switch)
        .connect_timeout(std::time::Duration::from_secs(30)) // 30s connection timeout
        .pool_idle_timeout(std::time::Duration::from_secs(90)) // Keep connections alive
        .build()
        .unwrap_or_default();

    let reqwest_method =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);

    // Build headers once, reuse for retry
    let mut forward_headers = reqwest::header::HeaderMap::new();
    const SKIP: &[&str] = &[
        "host",
        "x-api-key",
        "authorization",
        "x-goog-api-key",
        "content-length",
        "transfer-encoding",
        "connection",
        // Let reqwest negotiate its own encoding so it transparently
        // decompresses the body (and strips content-encoding /
        // content-length for us). Forwarding the client's value would
        // hand us bytes we can't read: error bodies would be logged as
        // mojibake and the usage tap couldn't parse the stream.
        "accept-encoding",
    ];
    for (name, value) in in_headers.iter() {
        if SKIP.contains(&name.as_str()) {
            continue;
        }
        if let Ok(val_str) = value.to_str() {
            if let (Ok(n), Ok(v)) = (
                reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes()),
                reqwest::header::HeaderValue::from_str(val_str),
            ) {
                forward_headers.insert(n, v);
            }
        }
    }
    forward_headers.insert(
        "x-api-key",
        reqwest::header::HeaderValue::from_str(&api_key).unwrap(),
    );

    // Resolve variant modifiers from the selected role whose bare model id
    // matches this request. Subagent/Haiku requests must not inherit the main
    // model's reasoning/context flags, and GPT requests retain the beta header
    // Claude Code derived from their `[1m]` config suffix.
    if path.contains("/messages") {
        apply_selected_anthropic_variant_headers(&mut forward_headers, &config, model.as_deref());
    }

    // Send with one retry on network error
    let mut last_err = String::new();
    let mut resp_result = None;
    for attempt in 0..2u8 {
        if attempt > 0 {
            log::info!("Retry attempt {} for {}", attempt, target_url);
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let mut req_builder = client
            .request(reqwest_method.clone(), &target_url)
            .headers(forward_headers.clone());
        if !body_bytes.is_empty() {
            req_builder = req_builder.body(body_bytes.clone());
        }
        match req_builder.send().await {
            Ok(resp) => {
                // Retry on 502 Bad Gateway (transient upstream error)
                if resp.status().as_u16() == 502 && attempt == 0 {
                    last_err = "502 Bad Gateway".to_string();
                    log::warn!(
                        "Proxy got 502 (attempt {}) → {}, will retry",
                        attempt + 1,
                        target_url
                    );
                    continue;
                }
                resp_result = Some(resp);
                break;
            }
            Err(e) => {
                last_err = e.to_string();
                log::warn!(
                    "Proxy forward error (attempt {}) → {}: {}",
                    attempt + 1,
                    target_url,
                    e
                );
            }
        }
    }

    match resp_result {
        Some(resp) => {
            let status_code = resp.status().as_u16();
            let latency_ms = start.elapsed().as_millis() as u64;
            let is_server_error = status_code >= 500 || status_code == 429;
            let is_any_error = status_code >= 400;

            // For errors, read the response body to get detailed error message
            if is_any_error {
                let error_body = resp
                    .text()
                    .await
                    .unwrap_or_else(|_| format!("HTTP {}", status_code));

                // Include model in error detail for better debugging
                let error_detail = format_error_detail(model.as_deref(), &error_body);

                let _ = state.db.add_traffic_log(
                    &gateway_id,
                    &path,
                    status_code,
                    latency_ms,
                    Some(&error_detail),
                );
                log::warn!(
                    "Proxy error {} → {} (model:{}): {}",
                    target_url,
                    status_code,
                    model.as_deref().unwrap_or("none"),
                    error_body
                );

                if is_server_error {
                    let count = state.consecutive_errors.fetch_add(1, Ordering::SeqCst) + 1;
                    if count >= ERROR_FAILOVER_THRESHOLD {
                        state.consecutive_errors.store(0, Ordering::SeqCst);
                        try_proxy_failover(&state, &gateway_id, status_code).await;
                    }
                } else {
                    state.consecutive_errors.store(0, Ordering::SeqCst);
                }

                emit_traffic(&state, &path, status_code, latency_ms, &gateway_id);

                // Return error response to client
                return (
                    StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY),
                    error_body,
                )
                    .into_response();
            }

            // Success path: stream the response
            state.consecutive_errors.store(0, Ordering::SeqCst);
            emit_traffic(&state, &path, status_code, latency_ms, &gateway_id);

            let is_sse = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|ct| ct.contains("text/event-stream"))
                .unwrap_or(false);

            let axum_status = StatusCode::from_u16(status_code).unwrap_or(StatusCode::OK);
            let resp_headers = resp.headers().clone();

            // Tap the response stream to record token usage in real-time and capture stream errors
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Result<Vec<u8>, String>>();
            let db_usage = state.db.clone();
            let gw_usage = gateway_id.clone();
            // `usage_log` is keyed by model, so a request that recorded tokens
            // without naming one still needs some bucket to land in. That is a
            // genuine gap in what we know — unlike a request that simply has no
            // model, which never reaches here (`should_track_usage` is false
            // for bodyless requests).
            let model_usage = model.clone().unwrap_or_else(|| "unknown".to_string());
            let track_usage = should_track_usage;
            let db_error = state.db.clone();
            let gw_error = gateway_id.clone();
            let path_error = path.clone();
            let start_error = start;
            let sink_usage = state.sink.clone();
            let gw_id_event = gateway_id.clone();

            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut stream_error: Option<String> = None;
                let mut last_emitted_usage = (0i64, 0i64, 0i64, 0i64);

                while let Some(result) = rx.recv().await {
                    match result {
                        Ok(chunk) => {
                            buffer.extend_from_slice(&chunk);

                            // Real-time parse and emit usage updates (for live UI display)
                            if is_sse && track_usage {
                                if let Ok(text) = std::str::from_utf8(&buffer) {
                                    let (inp, out, cr, cc) = parse_sse_tokens_incremental(text);
                                    // Emit event if usage changed (for real-time UI updates)
                                    if (inp, out, cr, cc) != last_emitted_usage
                                        && (inp > 0 || out > 0)
                                    {
                                        let payload = serde_json::json!({
                                            "gatewayId": gw_id_event,
                                            "model": model_usage,
                                            "inputTokens": inp,
                                            "outputTokens": out,
                                            "cacheReadTokens": cr,
                                            "cacheCreationTokens": cc,
                                        });
                                        sink_usage.emit("usage-update", payload);
                                        last_emitted_usage = (inp, out, cr, cc);
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            stream_error = Some(err);
                            break;
                        }
                    }
                }

                // Record stream error if any
                if let Some(err_msg) = stream_error {
                    let latency_ms = start_error.elapsed().as_millis() as u64;
                    let _ = db_error.add_traffic_log(
                        &gw_error,
                        &path_error,
                        502,
                        latency_ms,
                        Some(&err_msg),
                    );
                    log::warn!("Stream error on {}: {}", path_error, err_msg);
                }

                // Record usage ONCE at the end (to database, avoiding duplicate counting)
                if track_usage && !buffer.is_empty() {
                    let (inp, out, cr, cc) = if is_sse {
                        parse_sse_tokens(&buffer)
                    } else {
                        parse_json_tokens(&buffer)
                    };
                    if inp > 0 || out > 0 {
                        let _ = db_usage.record_usage(&gw_usage, &model_usage, inp, out, cr, cc);
                        log::debug!(
                            "Usage recorded to DB: {} in={} out={} cr={} cc={} model={}",
                            gw_usage,
                            inp,
                            out,
                            cr,
                            cc,
                            model_usage
                        );
                    }
                }
            });

            let stream = resp.bytes_stream().map(move |chunk| {
                match &chunk {
                    Ok(b) => {
                        let _ = tx.send(Ok(b.to_vec()));
                    }
                    Err(e) => {
                        let err_msg = format!("Stream error: {}", e);
                        let _ = tx.send(Err(err_msg));
                    }
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
        None => {
            let latency_ms = start.elapsed().as_millis() as u64;

            let _ = state
                .db
                .add_traffic_log(&gateway_id, &path, 502, latency_ms, Some(&last_err));

            let count = state.consecutive_errors.fetch_add(1, Ordering::SeqCst) + 1;
            if count >= ERROR_FAILOVER_THRESHOLD {
                state.consecutive_errors.store(0, Ordering::SeqCst);
                try_proxy_failover(&state, &gateway_id, 502).await;
            }

            emit_traffic(&state, &path, 502, latency_ms, &gateway_id);

            (
                StatusCode::BAD_GATEWAY,
                format!("{{\"error\":\"Gateway unreachable: {}\"}}", last_err),
            )
                .into_response()
        }
    }
}

/// Apply modifiers only when the actual request belongs to a configured Claude
/// role. GPT and unknown models deliberately retain all client-supplied headers.
fn apply_selected_anthropic_variant_headers(
    headers: &mut reqwest::header::HeaderMap,
    config: &ActiveConfig,
    request_model: Option<&str>,
) {
    if let Some(active_model) = selected_claude_role_model(config, request_model) {
        apply_anthropic_variant_headers(headers, Some(active_model));
    }
}

/// Find the configured Claude role that owns this request model. Composite
/// request ids identify roles exactly when multiple roles use different variants
/// of the same bare model. A bare request is accepted only when every matching
/// role has equivalent modifiers, so array order cannot silently choose a variant.
fn selected_claude_role_model<'a>(
    config: &'a ActiveConfig,
    request_model: Option<&str>,
) -> Option<&'a str> {
    let request_model = request_model?;
    let selected = [
        config.claude_model.as_deref(),
        config.claude_subagent_model.as_deref(),
        config.claude_small_model.as_deref(),
    ];

    if let Some(exact) = selected
        .into_iter()
        .flatten()
        .find(|model| is_claude_model(model) && model.eq_ignore_ascii_case(request_model))
    {
        return Some(exact);
    }

    let request_model = crate::model_id::without_context_suffix(request_model);
    let mut matches = selected.into_iter().flatten().filter(|model| {
        if !is_claude_model(model) {
            return false;
        }
        let (model, _) = crate::model_id::split_context_suffix(model);
        let (base, _, _) = decompose_claude_id(model);
        base.eq_ignore_ascii_case(request_model)
    });
    let first = matches.next()?;
    let (first_model, first_bracket_context) = crate::model_id::split_context_suffix(first);
    let (_, first_effort, first_composite_context) = decompose_claude_id(first_model);
    let first_context1m = first_bracket_context || first_composite_context;
    matches
        .all(|model| {
            let (model, bracket_context) = crate::model_id::split_context_suffix(model);
            let (_, effort, composite_context) = decompose_claude_id(model);
            effort == first_effort && (bracket_context || composite_context) == first_context1m
        })
        .then_some(first)
}

fn is_claude_model(model: &str) -> bool {
    model
        .get(.."claude-".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("claude-"))
}

/// Apply Copilot variant modifiers derived from a matched role's composite id.
///
/// The CLI config files hold bare Claude ids; suffixes stay in active_config and
/// become request headers here. Callers invoke this only after matching the
/// actual request model to one configured Claude role.
///
/// Behavior:
/// Truncate `s` to at most `max` **characters**, returning the slice and
/// whether anything was cut. Byte-slicing (`&s[..max]`) panics when the
/// index lands inside a multi-byte character — which any non-ASCII error
/// body can trigger.
fn truncate_chars(s: &str, max: usize) -> (&str, bool) {
    match s.char_indices().nth(max) {
        Some((idx, _)) => (&s[..idx], true),
        None => (s, false),
    }
}

/// Compose the `error_detail` stored in the traffic log.
///
/// A request that names no model gets no `[model:...]` prefix at all. The old
/// `[model:unknown]` read as "we tried to find the model and failed", when for
/// a bodyless GET — a probe for some path the gateway doesn't serve, say —
/// there was never a model to find.
fn format_error_detail(model: Option<&str>, body: &str) -> String {
    let (head, cut) = truncate_chars(body, 500);
    let prefix = model.map(|m| format!("[model:{m}] ")).unwrap_or_default();
    let ellipsis = if cut { "..." } else { "" };
    format!("{prefix}{head}{ellipsis}")
}

/// - `-1m` suffix appends/merges `context-1m-2025-08-07` into `anthropic-beta`
/// - `-high|-xhigh` suffix sets `x-copilot-reasoning-effort`
/// - No suffix → strip any client-supplied stale modifiers so they can't
///   pin requests to a variant the user didn't pick
fn apply_anthropic_variant_headers(
    headers: &mut reqwest::header::HeaderMap,
    active_model: Option<&str>,
) {
    const BETA_HEADER: &str = "anthropic-beta";
    const EFFORT_HEADER: &str = "x-copilot-reasoning-effort";
    const CONTEXT_1M_BETA: &str = "context-1m-2025-08-07";

    let (effort, context1m) = match active_model {
        Some(m)
            if m.get(.."claude-".len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("claude-")) =>
        {
            let (model, bracket_context) = crate::model_id::split_context_suffix(m);
            let (_, e, composite_context) = decompose_claude_id(model);
            (e, bracket_context || composite_context)
        }
        _ => (None, false),
    };

    // Effort: always overwrite so a downgrade (e.g. switching from -xhigh back
    // to the base model) reliably clears the variant on the wire.
    headers.remove(EFFORT_HEADER);
    if let Some(e) = effort {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&e) {
            headers.insert(EFFORT_HEADER, v);
        }
    }

    // Beta: merge with whatever betas the client sent (it may have its own
    // entries like `interleaved-thinking-2025-05-14`). Strip a stale 1m entry
    // if the active model doesn't want it; add it if it does.
    let existing: Vec<String> = headers
        .get_all(BETA_HEADER)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(',').map(|p| p.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect();
    let mut merged: Vec<String> = Vec::new();
    for b in existing {
        if b == CONTEXT_1M_BETA {
            continue;
        } // dedupe; re-added below if needed
        if !merged.contains(&b) {
            merged.push(b);
        }
    }
    if context1m {
        merged.push(CONTEXT_1M_BETA.to_string());
    }
    headers.remove(BETA_HEADER);
    if !merged.is_empty() {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&merged.join(",")) {
            headers.insert(BETA_HEADER, v);
        }
    }
}

/// Mirror of copilot-api-gateway's `parseCompositeModelId` (variants.ts):
/// strips up to two known suffixes (`-high|-xhigh` → effort, `-1m` → context1m)
/// from the tail in any order. Non-Claude ids pass through unchanged.
fn decompose_claude_id(id: &str) -> (String, Option<String>, bool) {
    if !id
        .get(.."claude-".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("claude-"))
    {
        return (id.to_string(), None, false);
    }
    let mut rest = id.to_string();
    let mut effort: Option<String> = None;
    let mut context1m = false;
    for _ in 0..2 {
        let Some(dash) = rest.rfind('-') else { break };
        let suffix = rest[dash + 1..].to_string();
        if suffix == "1m" && !context1m {
            context1m = true;
            rest.truncate(dash);
        } else if (suffix == "high" || suffix == "xhigh") && effort.is_none() {
            effort = Some(suffix);
            rest.truncate(dash);
        } else {
            break;
        }
    }
    (rest, effort, context1m)
}

/// Extract the `model` field from a JSON request body.
/// Returns None if body is empty, not JSON, or model field doesn't exist.
fn extract_model(body: &[u8], path: &str) -> Option<String> {
    if body.is_empty() {
        return None;
    }

    // Try to parse JSON and extract model field
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) {
        if let Some(model_str) = v.get("model").and_then(|m| m.as_str()) {
            return Some(model_str.to_string());
        }
    }

    // Fallback: try to infer from path
    // Gemini format: /v1beta/models/gemini-1.5-pro:generateContent
    if path.contains("/models/") {
        if let Some(model_part) = path.split("/models/").nth(1) {
            // Extract model name (stop at ':' for Gemini, or '/' for others)
            let model_name = model_part.split(&[':', '/'][..]).next().unwrap_or("");
            if !model_name.is_empty() {
                log::debug!("Inferred model from path: {}", model_name);
                return Some(model_name.to_string());
            }
        }
    }

    None
}

/// Inject `stream_options: {include_usage: true}` into OpenAI streaming requests.
/// This enables token usage reporting in the final SSE chunk.
/// Only injects if: request has `stream: true` and no existing `stream_options`.
/// IMPORTANT: Do NOT inject for Anthropic Messages API (path contains /messages).
fn inject_stream_options(body: &[u8], path: &str) -> Vec<u8> {
    // Anthropic Messages API doesn't support stream_options
    if path.contains("/messages") {
        return body.to_vec();
    }

    let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return body.to_vec();
    };

    // Only inject if streaming is enabled
    let is_streaming = v.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);

    if !is_streaming {
        return body.to_vec();
    }

    // Only inject if stream_options doesn't already exist
    if v.get("stream_options").is_none() {
        v["stream_options"] = serde_json::json!({"include_usage": true});

        if let Ok(modified) = serde_json::to_vec(&v) {
            log::debug!("Injected stream_options for usage tracking");
            return modified;
        }
    }

    body.to_vec()
}

/// Parse token usage from a streaming SSE response body.
/// Handles Anthropic Messages format (message_start / message_delta events).
/// Also handles OpenAI Chat Completions streaming format (final chunk with usage).
/// Also handles Google Gemini format (usageMetadata).
fn parse_sse_tokens(data: &[u8]) -> (i64, i64, i64, i64) {
    let text = match std::str::from_utf8(data) {
        Ok(t) => t,
        Err(_) => return (0, 0, 0, 0),
    };
    parse_sse_tokens_incremental(text)
}

/// Incremental SSE token parser - works with partial streams.
/// Returns usage info as soon as it's found, without waiting for stream end.
fn parse_sse_tokens_incremental(text: &str) -> (i64, i64, i64, i64) {
    let mut input: i64 = 0;
    let mut output: i64 = 0;
    let mut cache_read: i64 = 0;
    let mut cache_creation: i64 = 0;

    for line in text.lines() {
        let json_str = if let Some(s) = line.strip_prefix("data: ") {
            s
        } else {
            continue;
        };
        if json_str == "[DONE]" {
            continue;
        }

        let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) else {
            continue;
        };

        match v.get("type").and_then(|t| t.as_str()) {
            Some("message_start") => {
                // Anthropic: message_start.message.usage
                if let Some(usage) = v.pointer("/message/usage") {
                    input += usage
                        .get("input_tokens")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0);
                    cache_read += usage
                        .get("cache_read_input_tokens")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0);
                    cache_creation += usage
                        .get("cache_creation_input_tokens")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0);
                }
            }
            Some("message_delta") => {
                // Anthropic: message_delta.usage.output_tokens
                if let Some(usage) = v.get("usage") {
                    output += usage
                        .get("output_tokens")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0);
                }
            }
            Some("response.completed") => {
                // OpenAI Responses API (Codex CLI): response.completed.response.usage
                if let Some(usage) = v.pointer("/response/usage") {
                    let pt = usage
                        .get("input_tokens")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0);
                    let ct = usage
                        .get("output_tokens")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0);
                    if pt > 0 {
                        input = pt;
                    }
                    if ct > 0 {
                        output = ct;
                    }
                }
            }
            _ => {
                // Gemini: usageMetadata (check first as it's more specific)
                if let Some(usage_meta) = v.get("usageMetadata") {
                    let pt = usage_meta
                        .get("promptTokenCount")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0);
                    let ct = usage_meta
                        .get("candidatesTokenCount")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0);
                    if pt > 0 {
                        input = pt;
                    }
                    if ct > 0 {
                        output = ct;
                    }
                }
                // OpenAI Chat Completions streaming: last chunk may contain usage object
                else if let Some(usage) = v.get("usage") {
                    let pt = usage
                        .get("prompt_tokens")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0);
                    let ct = usage
                        .get("completion_tokens")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0);
                    if pt > 0 {
                        input = pt;
                    }
                    if ct > 0 {
                        output = ct;
                    }
                }
            }
        }
    }

    (input, output, cache_read, cache_creation)
}

/// Parse token usage from a non-streaming JSON response body.
/// Handles Anthropic, OpenAI, and Google Gemini formats.
fn parse_json_tokens(data: &[u8]) -> (i64, i64, i64, i64) {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(data) else {
        return (0, 0, 0, 0);
    };

    // Try Gemini format first (usageMetadata)
    if let Some(usage_meta) = v.get("usageMetadata") {
        let input = usage_meta
            .get("promptTokenCount")
            .and_then(|x| x.as_i64())
            .unwrap_or(0);
        let output = usage_meta
            .get("candidatesTokenCount")
            .and_then(|x| x.as_i64())
            .unwrap_or(0);
        return (input, output, 0, 0);
    }

    // Try OpenAI/Anthropic format (usage)
    let Some(usage) = v.get("usage") else {
        return (0, 0, 0, 0);
    };

    // Anthropic: input_tokens / output_tokens
    // OpenAI:    prompt_tokens / completion_tokens
    let input = usage
        .get("input_tokens")
        .and_then(|x| x.as_i64())
        .or_else(|| usage.get("prompt_tokens").and_then(|x| x.as_i64()))
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .and_then(|x| x.as_i64())
        .or_else(|| usage.get("completion_tokens").and_then(|x| x.as_i64()))
        .unwrap_or(0);
    let cache_read = usage
        .get("cache_read_input_tokens")
        .and_then(|x| x.as_i64())
        .unwrap_or(0);
    let cache_creation = usage
        .get("cache_creation_input_tokens")
        .and_then(|x| x.as_i64())
        .unwrap_or(0);

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
    state.sink.emit("proxy-traffic", payload);
}

/// Attempt to switch to the next healthy gateway after consecutive proxy errors.
async fn try_proxy_failover(state: &ProxyState, current_gateway_id: &str, error_status: u16) {
    let config = match state.db.get_active_config() {
        Ok(c) => c,
        Err(_) => return,
    };

    if !config.auto_switch {
        log::info!(
            "Proxy failover skipped (auto_switch disabled), status={}",
            error_status
        );
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
        // Skip gateways without an auth_key — they can't forward API traffic.
        if gw.auth_key.is_empty() {
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
            current_gateway_id,
            next_gw.name,
            ERROR_FAILOVER_THRESHOLD,
            error_status
        );
        let Some(service) = state.service.as_ref() else {
            log::warn!(
                "Proxy failover skipped: no Service attached to ProxyState. \
                 Active config unchanged. Use ProxyState::with_service() to enable."
            );
            return;
        };
        crate::health::do_switch(service, &next_gw.id, &config).await;
    } else {
        log::warn!(
            "Proxy failover: no healthy alternative to {} (status={})",
            current_gateway_id,
            error_status
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn active_config(
        main: Option<&str>,
        subagent: Option<&str>,
        haiku: Option<&str>,
    ) -> ActiveConfig {
        ActiveConfig {
            gateway_id: None,
            key_id: None,
            key_name: None,
            key_value: None,
            claude_model: main.map(str::to_owned),
            claude_subagent_model: subagent.map(str::to_owned),
            claude_small_model: haiku.map(str::to_owned),
            codex_model: None,
            codex_subagent_model: None,
            gemini_model: None,
            claude_extra_config_id: None,
            auto_switch: false,
            applied_at: None,
            last_switched_at: None,
        }
    }

    #[test]
    fn role_specific_claude_variants_do_not_leak_between_requests() {
        let config = active_config(
            Some("claude-opus-4.7-xhigh-1m"),
            Some("claude-sonnet-4.7-high"),
            Some("claude-haiku-4.5"),
        );

        let mut subagent_headers = reqwest::header::HeaderMap::new();
        apply_selected_anthropic_variant_headers(
            &mut subagent_headers,
            &config,
            Some("claude-sonnet-4.7"),
        );
        assert_eq!(
            subagent_headers
                .get("x-copilot-reasoning-effort")
                .and_then(|v| v.to_str().ok()),
            Some("high")
        );
        assert!(subagent_headers.get("anthropic-beta").is_none());

        let mut haiku_headers = reqwest::header::HeaderMap::new();
        apply_selected_anthropic_variant_headers(
            &mut haiku_headers,
            &config,
            Some("claude-haiku-4.5"),
        );
        assert!(haiku_headers.get("x-copilot-reasoning-effort").is_none());
        assert!(haiku_headers.get("anthropic-beta").is_none());
    }

    #[test]
    fn bracket_context_suffix_applies_only_to_the_matching_main_role() {
        let config = active_config(
            Some("claude-opus-5[1m]"),
            Some("claude-sonnet-5"),
            Some("claude-haiku-4-5"),
        );
        let mut main_headers = reqwest::header::HeaderMap::new();
        apply_selected_anthropic_variant_headers(
            &mut main_headers,
            &config,
            Some("claude-opus-5[1m]"),
        );
        assert_eq!(
            main_headers
                .get("anthropic-beta")
                .and_then(|value| value.to_str().ok()),
            Some("context-1m-2025-08-07")
        );

        let mut subagent_headers = reqwest::header::HeaderMap::new();
        apply_selected_anthropic_variant_headers(
            &mut subagent_headers,
            &config,
            Some("claude-sonnet-5"),
        );
        assert!(subagent_headers.get("anthropic-beta").is_none());
    }

    #[test]
    fn same_base_claude_variants_are_selected_by_composite_request_id() {
        let config = active_config(
            Some("claude-opus-4.7-xhigh"),
            Some("claude-opus-4.7-high"),
            Some("claude-opus-4.7-1m"),
        );

        let mut main_headers = reqwest::header::HeaderMap::new();
        apply_selected_anthropic_variant_headers(
            &mut main_headers,
            &config,
            Some("claude-opus-4.7-xhigh"),
        );
        assert_eq!(
            main_headers
                .get("x-copilot-reasoning-effort")
                .and_then(|value| value.to_str().ok()),
            Some("xhigh")
        );

        let mut subagent_headers = reqwest::header::HeaderMap::new();
        apply_selected_anthropic_variant_headers(
            &mut subagent_headers,
            &config,
            Some("claude-opus-4.7-high"),
        );
        assert_eq!(
            subagent_headers
                .get("x-copilot-reasoning-effort")
                .and_then(|value| value.to_str().ok()),
            Some("high")
        );

        let mut haiku_headers = reqwest::header::HeaderMap::new();
        apply_selected_anthropic_variant_headers(
            &mut haiku_headers,
            &config,
            Some("claude-opus-4.7-1m"),
        );
        assert_eq!(
            haiku_headers
                .get("anthropic-beta")
                .and_then(|value| value.to_str().ok()),
            Some("context-1m-2025-08-07")
        );

        let mut ambiguous_headers = reqwest::header::HeaderMap::new();
        ambiguous_headers.insert(
            "anthropic-beta",
            reqwest::header::HeaderValue::from_static("client-beta"),
        );
        apply_selected_anthropic_variant_headers(
            &mut ambiguous_headers,
            &config,
            Some("claude-opus-4.7"),
        );
        assert_eq!(
            ambiguous_headers
                .get("anthropic-beta")
                .and_then(|value| value.to_str().ok()),
            Some("client-beta")
        );
    }

    #[test]
    fn gpt_request_preserves_client_context_beta() {
        let config = active_config(
            Some("gpt-5.6-sol-fast"),
            Some("gpt-5.6-sol"),
            Some("claude-haiku-4.5"),
        );
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "anthropic-beta",
            reqwest::header::HeaderValue::from_static("context-1m-2025-08-07"),
        );

        apply_selected_anthropic_variant_headers(&mut headers, &config, Some("gpt-5.6-sol-fast"));
        assert_eq!(
            headers.get("anthropic-beta").and_then(|v| v.to_str().ok()),
            Some("context-1m-2025-08-07")
        );
    }

    #[test]
    fn uppercase_claude_composite_id_keeps_variant_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        apply_anthropic_variant_headers(&mut headers, Some("CLAUDE-opus-4.7-xhigh-1m"));
        assert_eq!(
            headers
                .get("x-copilot-reasoning-effort")
                .and_then(|v| v.to_str().ok()),
            Some("xhigh")
        );
        assert_eq!(
            headers.get("anthropic-beta").and_then(|v| v.to_str().ok()),
            Some("context-1m-2025-08-07")
        );
    }

    #[test]
    fn truncate_chars_keeps_short_strings_whole() {
        assert_eq!(truncate_chars("hello", 500), ("hello", false));
    }

    #[test]
    fn truncate_chars_cuts_at_char_count() {
        assert_eq!(truncate_chars("abcdef", 3), ("abc", true));
    }

    /// A body of U+FFFD replacement chars is 3 bytes per char, so the old
    /// `&body[..500]` byte-slice landed mid-character and panicked.
    #[test]
    fn truncate_chars_survives_multibyte_at_the_cut() {
        let body: String = "\u{FFFD}".repeat(600);
        let (head, cut) = truncate_chars(&body, 500);
        assert!(cut);
        assert_eq!(head.chars().count(), 500);
        assert!(
            !body.is_char_boundary(500),
            "precondition: byte 500 is mid-char"
        );
    }

    /// `GET /api/hello` — no body, so no model, and a gateway that doesn't
    /// serve the path answers 404 with nothing. There is nothing to report,
    /// and an empty detail is what makes the panel hide the row's expander.
    #[test]
    fn a_request_with_no_model_gets_no_prefix() {
        assert_eq!(format_error_detail(None, ""), "");
        assert_eq!(format_error_detail(None, "Not Found"), "Not Found");
    }

    #[test]
    fn a_named_model_is_prefixed() {
        assert_eq!(
            format_error_detail(Some("claude-opus-5"), "rate limited"),
            "[model:claude-opus-5] rate limited"
        );
    }

    #[test]
    fn long_bodies_are_cut_with_an_ellipsis() {
        let body = "x".repeat(600);
        let detail = format_error_detail(Some("m"), &body);
        assert!(detail.starts_with("[model:m] xxx"));
        assert!(detail.ends_with("..."));
        assert_eq!(detail.chars().count(), "[model:m] ".len() + 500 + 3);
    }

    fn test_state() -> ProxyState {
        let db = Database::open_in_memory().expect("open_in_memory");
        ProxyState {
            db: Arc::new(db),
            switch_lock: Arc::new(tokio::sync::Mutex::new(())),
            sink: Arc::new(crate::events::NullSink),
            service: None,
            consecutive_errors: Arc::new(AtomicU32::new(0)),
        }
    }

    #[tokio::test]
    async fn shutdown_releases_both_ports_and_wsl_cannot_restart_it() {
        let primary = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = primary.local_addr().unwrap();
        let wsl = std::net::TcpListener::bind("127.0.0.2:0").unwrap();
        let wsl_addr = wsl.local_addr().unwrap();
        let proxy = start_with_listeners(test_state(), primary, Some((wsl_addr.ip(), wsl))).await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for address in [addr, wsl_addr] {
            assert_eq!(
                client
                    .get(format!("http://{address}/_relay/ping"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::OK
            );
        }
        tokio::time::timeout(std::time::Duration::from_secs(2), proxy.clone().shutdown())
            .await
            .unwrap();
        assert!(!proxy.is_running());
        let primary_owner = std::net::TcpListener::bind(addr).unwrap();
        let _wsl_owner = std::net::TcpListener::bind(wsl_addr).unwrap();
        proxy.rebind_wsl(Some(wsl_addr.ip())).await.unwrap();
        assert!(proxy.wsl_ip().is_none());
        assert!(proxy.start().await.is_err());
        assert!(!proxy.is_running());
        drop(primary_owner);
        proxy.start().await.unwrap();
        assert_eq!(
            client
                .get(format!("http://{addr}/_relay/ping"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        proxy.shutdown().await;
        std::net::TcpListener::bind(addr).unwrap();
    }

    #[tokio::test]
    async fn shutdown_cancels_streams_and_pending_requests() {
        use std::convert::Infallible;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let signal = entered.clone();
        let router = Router::new()
            .route(
                "/flood",
                get(|| async {
                    Body::from_stream(futures_util::stream::repeat(Ok::<_, Infallible>(vec![
                        b'x';
                        65536
                    ])))
                }),
            )
            .route(
                "/stream",
                get(|| async {
                    Body::from_stream(
                        futures_util::stream::once(async { Ok::<_, Infallible>("first chunk") })
                            .chain(futures_util::stream::pending()),
                    )
                }),
            )
            .route(
                "/pending",
                get(move || {
                    let signal = signal.clone();
                    async move {
                        signal.notify_one();
                        std::future::pending::<String>().await
                    }
                }),
            );
        let proxy = ProxyHandle::stopped(test_state(), addr.port());
        *proxy.primary.lock().unwrap() = Some(serve_router(router, listener).unwrap());
        proxy.running.store(true, Ordering::Release);
        let mut streaming = tokio::net::TcpStream::connect(addr).await.unwrap();
        streaming
            .write_all(b"GET /stream HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut first = [0; 1024];
        assert!(streaming.read(&mut first).await.unwrap() > 0);
        let mut pending = tokio::net::TcpStream::connect(addr).await.unwrap();
        pending
            .write_all(b"GET /pending HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        entered.notified().await;
        // An idle connection with incomplete headers must not hold shutdown open.
        let mut idle = tokio::net::TcpStream::connect(addr).await.unwrap();
        idle.write_all(b"GET /pending HTTP/1.1\r\n").await.unwrap();
        let mut slow = tokio::net::TcpStream::connect(addr).await.unwrap();
        slow.write_all(b"GET /flood HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        slow.read(&mut first).await.unwrap();
        // Allow this client's unread response to fill the socket buffer.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        tokio::time::timeout(std::time::Duration::from_secs(2), proxy.clone().shutdown())
            .await
            .unwrap();
        for mut connection in [streaming, pending, idle] {
            let mut remaining = Vec::new();
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                connection.read_to_end(&mut remaining),
            )
            .await
            .unwrap();
        }
        std::net::TcpListener::bind(addr).unwrap();
    }

    #[tokio::test]
    async fn relay_ping_returns_200_ok() {
        let app = build_router(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/_relay/ping")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn relay_reserved_namespace_returns_404_locally() {
        let app = build_router(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/_relay/unknown")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // 404 produced by relay_reserved, NOT forwarded upstream (which
        // would return 503 "No active config" from forward()). The body
        // proves it.
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"unknown relay endpoint");
    }
}
