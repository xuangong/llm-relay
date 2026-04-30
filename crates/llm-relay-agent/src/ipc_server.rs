use anyhow::Result;
use interprocess::local_socket::tokio::prelude::*;
use llm_relay_core::ipc::codec::{read_frame, write_frame};
use llm_relay_core::ipc::protocol::*;
use llm_relay_core::ipc::transport;
use llm_relay_core::Service;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use crate::login::LoginRegistry;

/// A bus that the agent's domain code pushes events into; each connected
/// client subscribes to it (filtered by their Topic set).
#[derive(Clone)]
pub struct EventBus(pub broadcast::Sender<Event>);

impl EventBus {
    pub fn new() -> Self { Self(broadcast::channel(1024).0) }
    pub fn publish(&self, ev: Event) { let _ = self.0.send(ev); }
    pub fn subscribe(&self) -> broadcast::Receiver<Event> { self.0.subscribe() }
}

/// Implement EventSink by forwarding emit() into the bus. We pattern-match
/// the JSON to convert the loose (name, json) into a typed Event.
pub struct BusSink { pub bus: EventBus }
impl llm_relay_core::EventSink for BusSink {
    fn emit(&self, name: &str, payload: serde_json::Value) {
        let ev: Option<Event> = match name {
            "health_changed" => serde_json::from_value(payload).ok().map(|v: HealthChanged| Event::HealthChanged { gateway_id: v.gateway_id, status: v.status }),
            "active_changed" => serde_json::from_value(payload).ok().map(|v: ActiveChanged| Event::ActiveChanged { gateway_id: v.gateway_id }),
            "traffic_error" => serde_json::from_value::<TrafficEntry>(payload).ok().map(Event::TrafficError),
            "usage_delta" => serde_json::from_value::<UsageDelta>(payload).ok().map(|v| Event::UsageDelta { gateway_id: v.gateway_id, model: v.model, input: v.input, output: v.output, cache: v.cache }),
            "login_completed" => serde_json::from_value::<LoginCompleted>(payload).ok().map(|v| Event::LoginCompleted { gateway_id: v.gateway_id, session_token: v.session_token, user_id: v.user_id, user_name: v.user_name }),
            "login_failed" => serde_json::from_value::<LoginFailed>(payload).ok().map(|v| Event::LoginFailed { gateway_id: v.gateway_id, message: v.message }),
            "login_expired" => serde_json::from_value::<LoginExpired>(payload).ok().map(|v| Event::LoginExpired { gateway_id: v.gateway_id }),
            other => { log::warn!("unmapped event name: {other}"); None }
        };
        if let Some(ev) = ev { self.bus.publish(ev); }
    }
}

#[derive(serde::Deserialize)] struct HealthChanged { gateway_id: uuid::Uuid, status: HealthStatus }
#[derive(serde::Deserialize)] struct ActiveChanged { gateway_id: Option<uuid::Uuid> }
#[derive(serde::Deserialize)] struct UsageDelta { gateway_id: uuid::Uuid, model: String, input: u64, output: u64, cache: u64 }
#[derive(serde::Deserialize)] struct LoginCompleted { gateway_id: uuid::Uuid, session_token: String, user_id: Option<String>, user_name: Option<String> }
#[derive(serde::Deserialize)] struct LoginFailed { gateway_id: uuid::Uuid, message: String }
#[derive(serde::Deserialize)] struct LoginExpired { gateway_id: uuid::Uuid }

pub struct ServerCtx {
    pub service: Service,
    pub bus: EventBus,
    pub agent_started_at: chrono::DateTime<chrono::Utc>,
    pub agent_pid: u32,
    pub keystore_kind: KeystoreKind,
    pub shutdown: Arc<tokio::sync::Notify>,
    pub login_registry: Arc<LoginRegistry>,
}

pub async fn run(sock_path: &Path, ctx: ServerCtx) -> Result<()> {
    let listener = transport::build_listener(sock_path)?;
    log::info!("ipc listening at {}", sock_path.display());

    loop {
        tokio::select! {
            _ = ctx.shutdown.notified() => {
                log::info!("ipc server shutting down");
                return Ok(());
            }
            res = listener.accept() => {
                let stream = match res {
                    Ok(s) => s,
                    Err(e) => { log::warn!("accept: {e}"); continue; }
                };
                let ctx = ctx.clone_for_conn();
                tokio::spawn(async move {
                    if let Err(e) = handle_conn(stream, ctx).await {
                        log::warn!("conn ended: {e}");
                    }
                });
            }
        }
    }
}

impl ServerCtx {
    fn clone_for_conn(&self) -> ConnCtx {
        ConnCtx {
            service: self.service.clone(),
            bus: self.bus.clone(),
            agent_started_at: self.agent_started_at,
            agent_pid: self.agent_pid,
            keystore_kind: self.keystore_kind,
            shutdown: self.shutdown.clone(),
            login_registry: self.login_registry.clone(),
        }
    }
}

#[derive(Clone)]
struct ConnCtx {
    service: Service,
    bus: EventBus,
    agent_started_at: chrono::DateTime<chrono::Utc>,
    agent_pid: u32,
    keystore_kind: KeystoreKind,
    shutdown: Arc<tokio::sync::Notify>,
    login_registry: Arc<LoginRegistry>,
}

async fn handle_conn<S>(stream: S, ctx: ConnCtx) -> Result<()>
where S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static {
    // Security note (Windows):
    // The named-pipe DACL granted by `interprocess` permits any local interactive user.
    // The ideal hardening is `GetNamedPipeClientProcessId` -> `OpenProcessToken` ->
    // `GetTokenInformation(TokenUser)` to verify the peer's SID matches the agent's.
    // However `interprocess::local_socket::tokio::Stream` (the wrapper enum) does not
    // expose `AsRawHandle`, so we cannot reach the underlying HANDLE without forking
    // the crate. On Unix the kernel enforces the chmod 0600 on the socket file (see
    // `ipc/transport.rs`), which is the equivalent and primary mitigation. On Windows
    // operators should rely on host-level controls until upstream exposes the handle
    // or DACL hook.
    use tokio::io::split;
    let (mut rd, mut wr) = split(stream);
    let topics: Arc<Mutex<HashSet<Topic>>> = Arc::new(Mutex::new(HashSet::new()));
    let mut events_rx = ctx.bus.subscribe();
    let topics_for_pump = topics.clone();
    let (write_tx, mut write_rx) = tokio::sync::mpsc::channel::<ServerFrame>(64);

    // Writer task: serializes everything onto the wire.
    let writer = tokio::spawn(async move {
        while let Some(frame) = write_rx.recv().await {
            if let Err(e) = write_frame(&mut wr, &frame).await { log::warn!("write: {e}"); break; }
        }
    });

    // Event pump: relays bus events to the writer if subscribed.
    let pump_tx = write_tx.clone();
    let pump = tokio::spawn(async move {
        loop {
            match events_rx.recv().await {
                Ok(ev) => {
                    let topic = match &ev {
                        Event::HealthChanged { .. } => Topic::Health,
                        Event::ActiveChanged { .. } => Topic::Active,
                        Event::TrafficError { .. } => Topic::Traffic,
                        Event::UsageDelta { .. } => Topic::Usage,
                        Event::LoginCompleted { .. } | Event::LoginFailed { .. } | Event::LoginExpired { .. } => Topic::Login,
                    };
                    if topics_for_pump.lock().await.contains(&topic) {
                        let _ = pump_tx.send(ServerFrame::Event(ev)).await;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => log::warn!("event lag {n}"),
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // Request loop.
    loop {
        let frame: ClientFrame = match read_frame(&mut rd).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };
        let resp = dispatch(&ctx, frame.payload, &topics).await;
        if write_tx.send(ServerFrame::Response { request_id: frame.request_id, payload: resp }).await.is_err() {
            break;
        }
    }

    drop(write_tx);
    let _ = writer.await;
    pump.abort();
    Ok(())
}

async fn dispatch(ctx: &ConnCtx, req: Request, topics: &Arc<Mutex<HashSet<Topic>>>) -> Response {
    macro_rules! ok_or_err { ($e:expr) => { match $e { Ok(_) => Response::Ok, Err(e) => Response::Error { message: e.to_string() } } } }

    match req {
        Request::Ping => Response::Pong,
        Request::Subscribe { topics: t } => { topics.lock().await.extend(t); Response::Ok }
        Request::Unsubscribe { topics: t } => { for x in t { topics.lock().await.remove(&x); } Response::Ok }
        Request::GetSnapshot => match ctx.service.snapshot(ctx.agent_pid, ctx.agent_started_at, llm_relay_core::paths::proxy_port(), ctx.keystore_kind).await {
            Ok(s) => Response::Snapshot(s), Err(e) => Response::Error { message: e.to_string() },
        },
        Request::AddGateway(input) => match ctx.service.add_gateway(input).await {
            Ok(_) => Response::Ok, Err(e) => Response::Error { message: e.to_string() },
        },
        Request::UpdateGateway { id, fields } => ok_or_err!(ctx.service.update_gateway(id, fields).await),
        Request::DeleteGateway { id } => ok_or_err!(ctx.service.delete_gateway(id).await),
        Request::SetActive { gateway_id, key_id, models } => ok_or_err!(ctx.service.set_active(gateway_id, key_id, models).await),
        Request::ClearActive => ok_or_err!(ctx.service.clear_active().await),
        Request::SetAutoFailover { enabled } => ok_or_err!(ctx.service.set_auto_failover(enabled).await),
        Request::Reorder { ids } => ok_or_err!(ctx.service.reorder(ids).await),
        Request::FetchKeys { gateway_id } => match ctx.service.fetch_keys(gateway_id).await {
            Ok(v) => Response::Keys { keys: v }, Err(e) => Response::Error { message: e.to_string() },
        },
        Request::FetchModels { gateway_id, key_id } => match ctx.service.fetch_models(gateway_id, key_id).await {
            Ok(c) => Response::Models { catalog: c }, Err(e) => Response::Error { message: e.to_string() },
        },
        Request::GetUsage { range, gateway_id } => match ctx.service.get_usage(range, gateway_id).await {
            Ok(u) => Response::Usage(u), Err(e) => Response::Error { message: e.to_string() },
        },
        Request::GetTrafficLog { gateway_id } => match ctx.service.get_traffic_log(gateway_id).await {
            Ok(v) => Response::TrafficLog { entries: v }, Err(e) => Response::Error { message: e.to_string() },
        },
        Request::GetSettings => match ctx.service.get_settings().await {
            Ok(s) => Response::Settings(s), Err(e) => Response::Error { message: e.to_string() },
        },
        Request::UpdateSettings(u) => ok_or_err!(ctx.service.update_settings(u).await),
        Request::StartLogin { gateway_id } => {
            let url = match ctx.service.get_gateway_url(gateway_id).await {
                Ok(u) => u,
                Err(e) => return Response::Error { message: e.to_string() },
            };
            let base = url.trim_end_matches('/');
            let verification_uri = format!("{base}/device/login");
            match ctx.login_registry.start(gateway_id, url).await {
                Ok(code) => Response::LoginInitiated {
                    gateway_id,
                    user_code: code.user_code,
                    verification_uri,
                    expires_in_secs: code.expires_in,
                },
                Err(e) => Response::Error { message: e.to_string() },
            }
        }
        Request::CancelLogin { gateway_id } => {
            ctx.login_registry.cancel(gateway_id).await;
            Response::LoginCancelled { gateway_id }
        }
        Request::Shutdown => {
            let sd = ctx.shutdown.clone();
            tokio::spawn(async move { tokio::time::sleep(std::time::Duration::from_millis(50)).await; sd.notify_one(); });
            Response::Ok
        }
        Request::ListGateways => {
            match ctx.service.list_gateways() {
                Ok(gateways) => Response::GatewayList { gateways },
                Err(e) => Response::Error { message: e.to_string() },
            }
        }
        Request::GetUsageRows { range } => match ctx.service.get_usage_rows(range).await {
            Ok(rows) => Response::UsageRows { rows },
            Err(e) => Response::Error { message: e.to_string() },
        },
        Request::GetErrors { limit } => match ctx.service.get_errors(limit).await {
            Ok(rows) => Response::ErrorRows { rows },
            Err(e) => Response::Error { message: e.to_string() },
        },
        Request::GetTuiSettings => {
            let socket_path = llm_relay_core::paths::sock_file().to_string_lossy().into_owned();
            let log_path = llm_relay_core::paths::log_file().to_string_lossy().into_owned();
            match ctx.service.get_tui_settings(
                ctx.agent_pid,
                ctx.keystore_kind,
                socket_path,
                llm_relay_core::paths::proxy_port(),
                log_path,
            ).await {
                Ok(s) => Response::TuiSettings(s),
                Err(e) => Response::Error { message: e.to_string() },
            }
        }
        Request::SetAutoLaunch { enabled } => match ctx.service.set_auto_launch(enabled).await {
            Ok(_) => Response::SettingsAck,
            Err(e) => Response::Error { message: e.to_string() },
        },
        Request::AddGatewaySimple { name, url } => {
            match ctx.service.add_gateway_simple(name, url).await {
                Ok(id) => Response::GatewayCreated { id },
                Err(e) => Response::Error { message: e.to_string() },
            }
        }
        Request::UpdateGatewaySimple { id, name, url } => {
            match ctx.service.update_gateway_simple(id, name, url).await {
                Ok(_) => Response::GatewayUpdated { id },
                Err(e) => Response::Error { message: e.to_string() },
            }
        }
        Request::GetGatewayConfig { gateway_id } => {
            match ctx.service.get_gateway_config(gateway_id).await {
                Ok((active_key_id, claude, claude_small, codex, gemini)) => Response::GatewayConfig {
                    active_key_id,
                    claude,
                    claude_small,
                    codex,
                    gemini,
                },
                Err(e) => Response::Error { message: e.to_string() },
            }
        }
        Request::SaveGatewayConfig { gateway_id, key_id, models } => {
            ok_or_err!(ctx.service.save_gateway_config(gateway_id, key_id, models).await)
        }
    }
}
