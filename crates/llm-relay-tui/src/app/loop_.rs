//! Main loop: drains crossterm key events and IPC events, applies them to
//! `AppState`, and re-renders.
//!
//! A `client_slot` wraps the current `Arc<IpcClient>` behind an
//! `Arc<tokio::sync::Mutex<>>` so the reconnect path can swap in a fresh
//! client while all other code paths borrow through the same slot.

use crate::app::{
    event::AppEvent,
    modal::{EditGatewayForm, Modal, ModalOutcome, ModalSubmit, SelectField, SelectKeyModelForm},
    state::{AppState, GatewayRow, Tab},
    terminal::Tui,
};
use crate::bootstrap::{self, AgentHandle, EnsureMode};
use crate::ipc_client::IpcClient;
use crossterm::event::{self, Event as CtEvent, KeyCode, KeyEventKind};
use llm_relay_core::ipc::{GatewaySummary, Request, Response};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};

fn into_row(g: GatewaySummary) -> GatewayRow {
    GatewayRow {
        id: g.id,
        name: g.name,
        url: g.url,
        healthy: g.healthy,
        latency_ms: g.latency_ms,
        starred: g.starred,
        expanded: false,
        needs_login: g.needs_login,
        active_key_name: g.active_key_name,
        claude_model: g.claude_model,
        claude_small_model: g.claude_small_model,
        codex_model: g.codex_model,
        gemini_model: g.gemini_model,
        user_name: g.user_name,
    }
}

fn gw_rows_from_response(resp: Response) -> Option<Vec<GatewayRow>> {
    if let Response::GatewayList { gateways } = resp {
        Some(gateways.into_iter().map(into_row).collect())
    } else {
        None
    }
}

/// Fetch usage rows from the agent and store them in state.
async fn fetch_usage(client: &IpcClient, state: &mut AppState) {
    match client.request(Request::GetUsageRows { range: state.usage.range }).await {
        Ok(Response::UsageRows { rows }) => {
            state.usage.rows = rows;
            state.usage.error = None;
        }
        Ok(Response::Error { message }) => {
            state.usage.rows.clear();
            state.usage.error = Some(message);
        }
        Ok(_) => {}
        Err(e) => {
            state.usage.rows.clear();
            state.usage.error = Some(e.to_string());
        }
    }
}

/// Fetch error rows from the agent and store them in state.
async fn fetch_errors(client: &IpcClient, state: &mut AppState) {
    match client.request(Request::GetErrors { limit: 100 }).await {
        Ok(Response::ErrorRows { rows }) => {
            state.errors.rows = rows;
            state.errors.error = None;
        }
        Ok(Response::Error { message }) => {
            state.errors.rows.clear();
            state.errors.error = Some(message);
        }
        Ok(_) => {}
        Err(e) => {
            state.errors.rows.clear();
            state.errors.error = Some(e.to_string());
        }
    }
}

/// Fetch TUI settings snapshot from the agent and store in state.
async fn fetch_settings(client: &IpcClient, state: &mut AppState) {
    if let Ok(Response::TuiSettings(s)) = client.request(Request::GetTuiSettings).await {
        state.settings.snapshot = Some(s);
    }
}

async fn handle_submit(client: &Arc<IpcClient>, state: &mut AppState, submit: ModalSubmit) {
    match submit {
        ModalSubmit::AddGateway { name, url } => {
            if let Some(Modal::AddGateway(f)) = state.modal.as_mut() {
                f.submitting = true;
                f.error = None;
            }
            match client.request(Request::AddGatewaySimple { name, url }).await {
                Ok(Response::GatewayCreated { .. }) => {
                    state.modal = None;
                    if let Ok(resp) = client.request(Request::ListGateways).await {
                        if let Some(rows) = gw_rows_from_response(resp) {
                            state.replace_gateways(rows);
                        }
                    }
                }
                Ok(Response::Error { message }) => {
                    if let Some(Modal::AddGateway(f)) = state.modal.as_mut() {
                        f.submitting = false;
                        f.error = Some(message);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    if let Some(Modal::AddGateway(f)) = state.modal.as_mut() {
                        f.submitting = false;
                        f.error = Some(e.to_string());
                    }
                }
            }
        }
        ModalSubmit::EditGateway { id, name, url } => {
            match client.request(Request::UpdateGatewaySimple { id, name, url }).await {
                Ok(Response::GatewayUpdated { .. }) => {
                    state.modal = None;
                    if let Ok(resp) = client.request(Request::ListGateways).await {
                        if let Some(rows) = gw_rows_from_response(resp) {
                            state.replace_gateways(rows);
                        }
                    }
                }
                Ok(Response::Error { message }) => {
                    if let Some(Modal::EditGateway(f)) = state.modal.as_mut() {
                        f.error = Some(message);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    if let Some(Modal::EditGateway(f)) = state.modal.as_mut() {
                        f.error = Some(e.to_string());
                    }
                }
            }
        }
        ModalSubmit::SaveConfig { gateway_id, key_id, models } => {
            if let Some(Modal::SelectKeyModel(f)) = state.modal.as_mut() {
                f.submitting = true;
                f.error = None;
            }
            match client.request(Request::SaveGatewayConfig { gateway_id, key_id, models }).await {
                Ok(Response::Ok) => {
                    state.modal = None;
                    state.status_message = Some("Config saved".to_string());
                    if let Ok(resp) = client.request(Request::ListGateways).await {
                        if let Some(rows) = gw_rows_from_response(resp) {
                            state.replace_gateways(rows);
                        }
                    }
                }
                Ok(Response::Error { message }) => {
                    if let Some(Modal::SelectKeyModel(f)) = state.modal.as_mut() {
                        f.submitting = false;
                        f.error = Some(message);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    if let Some(Modal::SelectKeyModel(f)) = state.modal.as_mut() {
                        f.submitting = false;
                        f.error = Some(e.to_string());
                    }
                }
            }
        }
    }
}

/// Open SelectKeyModel modal: show loading state immediately, then fetch in background.
async fn open_select_key_model(
    client: &Arc<IpcClient>,
    state: &mut AppState,
    gateway_id: uuid::Uuid,
    gateway_name: String,
    term: &mut crate::app::terminal::Tui,
) -> std::io::Result<()> {
    log::info!("open_select_key_model: gateway_id={gateway_id}");
    // Show loading modal immediately.
    state.modal = Some(Modal::SelectKeyModel(SelectKeyModelForm {
        gateway_id,
        gateway_name: gateway_name.clone(),
        keys: vec![],
        selected_key_idx: 0,
        catalog: None,
        claude_idx: 0,
        claude_small_idx: 0,
        codex_idx: 0,
        gemini_idx: 0,
        focus: SelectField::Key,
        error: None,
        submitting: false,
        loading_models: true,
    }));
    term.draw(|f| crate::view::render(f, state))?;

    // Fetch keys.
    let keys = match client.request(Request::FetchKeys { gateway_id }).await {
        Ok(Response::Keys { keys: k }) => k,
        Ok(Response::Error { message }) => {
            log::warn!("FetchKeys error: {message}");
            if let Some(Modal::SelectKeyModel(f)) = state.modal.as_mut() {
                f.loading_models = false;
                f.error = Some(format!("FetchKeys: {message}"));
            }
            return Ok(());
        }
        Ok(_other) => {
            if let Some(Modal::SelectKeyModel(f)) = state.modal.as_mut() {
                f.loading_models = false;
                f.error = Some("FetchKeys: unexpected response".into());
            }
            return Ok(());
        }
        Err(e) => {
            log::warn!("FetchKeys failed: {e}");
            if let Some(Modal::SelectKeyModel(f)) = state.modal.as_mut() {
                f.loading_models = false;
                f.error = Some(format!("FetchKeys: {e}"));
            }
            return Ok(());
        }
    };
    if keys.is_empty() {
        if let Some(Modal::SelectKeyModel(f)) = state.modal.as_mut() {
            f.loading_models = false;
            f.error = Some("No keys available".into());
        }
        return Ok(());
    }

    // Fetch saved config to pre-select key and models.
    let (saved_key_id, saved_claude, saved_claude_small, saved_codex, saved_gemini) =
        match client.request(Request::GetGatewayConfig { gateway_id }).await {
            Ok(Response::GatewayConfig { active_key_id, claude, claude_small, codex, gemini }) => {
                (active_key_id, claude, claude_small, codex, gemini)
            }
            _ => (None, None, None, None, None),
        };

    // Pre-select the saved key, or default to first.
    let selected_key_idx = saved_key_id
        .and_then(|kid| keys.iter().position(|k| k.id == kid))
        .unwrap_or(0);
    let key_id = keys[selected_key_idx].id;

    let catalog = match client.request(Request::FetchModels { gateway_id, key_id }).await {
        Ok(Response::Models { catalog: c }) => Some(c),
        _ => None,
    };

    // Use saved models if available, otherwise auto-suggest.
    let (claude_idx, claude_small_idx, codex_idx, gemini_idx) =
        saved_model_indexes(&catalog, &saved_claude, &saved_claude_small, &saved_codex, &saved_gemini);

    if let Some(Modal::SelectKeyModel(f)) = state.modal.as_mut() {
        f.keys = keys;
        f.selected_key_idx = selected_key_idx;
        f.catalog = catalog;
        f.claude_idx = claude_idx;
        f.claude_small_idx = claude_small_idx;
        f.codex_idx = codex_idx;
        f.gemini_idx = gemini_idx;
        f.loading_models = false;
    }
    Ok(())
}

fn saved_model_indexes(
    catalog: &Option<llm_relay_core::ipc::ModelCatalog>,
    saved_claude: &Option<String>,
    saved_claude_small: &Option<String>,
    saved_codex: &Option<String>,
    saved_gemini: &Option<String>,
) -> (usize, usize, usize, usize) {
    let cat = match catalog {
        Some(c) => c,
        None => return (0, 0, 0, 0),
    };
    let find = |list: &[String], saved: &Option<String>, fallback: &str| -> usize {
        if let Some(s) = saved {
            if let Some(pos) = list.iter().position(|m| m == s) {
                return pos;
            }
        }
        list.iter().position(|m| m.contains(fallback)).unwrap_or(0)
    };
    (
        find(&cat.claude, saved_claude, "opus"),
        find(&cat.claude, saved_claude_small, "haiku"),
        find(&cat.codex, saved_codex, ""),
        find(&cat.gemini, saved_gemini, ""),
    )
}

pub async fn run(mut term: Tui, initial_client: Arc<IpcClient>, socket: PathBuf) -> std::io::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

    // Wrap client in a slot so the reconnect path can swap it.
    let client_slot: Arc<Mutex<Arc<IpcClient>>> = Arc::new(Mutex::new(initial_client));

    // Spawn key reader.
    {
        let tx = tx.clone();
        std::thread::spawn(move || loop {
            if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                if let Ok(CtEvent::Key(k)) = event::read() {
                    if k.kind != KeyEventKind::Press {
                        continue;
                    }
                    let app_evt = match k.code {
                        KeyCode::Char('q') => AppEvent::Quit,
                        KeyCode::Tab => AppEvent::NextTab,
                        KeyCode::BackTab => AppEvent::PrevTab,
                        KeyCode::Up => AppEvent::Up,
                        KeyCode::Down => AppEvent::Down,
                        KeyCode::Left => AppEvent::Left,
                        KeyCode::Right => AppEvent::Right,
                        KeyCode::Enter => AppEvent::Enter,
                        KeyCode::Esc => AppEvent::Esc,
                        KeyCode::Backspace => AppEvent::Backspace,
                        KeyCode::Char('r') => AppEvent::Refresh,
                        // 'a' is context-sensitive: disambiguated after state.handle() below.
                        KeyCode::Char(c) => AppEvent::Char(c),
                        _ => continue,
                    };
                    if tx.send(app_evt).is_err() {
                        break;
                    }
                }
            }
        });
    }

    // Spawn IPC event forwarder (re-spawned on reconnect below).
    {
        let tx = tx.clone();
        let client = client_slot.lock().await.clone();
        let mut sub = client.subscribe();
        tokio::spawn(async move {
            while let Ok(evt) = sub.recv().await {
                if tx.send(AppEvent::Ipc(evt)).is_err() {
                    break;
                }
            }
        });
    }

    let mut state = AppState::new();
    let mut prev_tab = state.active_tab;

    // Initial gateway load.
    {
        let client = client_slot.lock().await.clone();
        // Subscribe to event topics. The agent filters per-connection by topic,
        // so without this the bus pump would drop every event before it reaches
        // the writer. (See `bootstrap::default_topics` for the canonical set.)
        let _ = client
            .request(Request::Subscribe { topics: bootstrap::default_topics() })
            .await;
        if let Ok(resp) = client.request(Request::ListGateways).await {
            if let Some(rows) = gw_rows_from_response(resp) {
                state.replace_gateways(rows);
            }
        }
    }

    // Initial render.
    term.draw(|f| crate::view::render(f, &state))?;

    // Bind initial disconnect watch.
    let mut disc = client_slot.lock().await.disconnected();

    loop {
        tokio::select! {
            // ── Disconnect / reconnect arm ───────────────────────────────────
            _ = disc.changed() => {
                if *disc.borrow() {
                    state.status_message = Some("Agent disconnected — reconnecting...".into());
                    term.draw(|f| crate::view::render(f, &state))?;

                    // Retry until we get a new connection.
                    loop {
                        match bootstrap::ensure_agent(&socket, EnsureMode::AttachOrSpawn).await {
                            Ok(AgentHandle::Attached(c)) | Ok(AgentHandle::Spawned { client: c, .. }) => {
                                // Swap the client.
                                *client_slot.lock().await = c.clone();
                                disc = c.disconnected();

                                // Re-subscribe: the agent filters events per-connection by
                                // topic set, and that set lives on the old (now-dead) conn.
                                // Without this, no events would ever reach the new client.
                                let _ = c
                                    .request(Request::Subscribe { topics: bootstrap::default_topics() })
                                    .await;

                                // Re-spawn IPC event forwarder for the new client.
                                {
                                    let tx = tx.clone();
                                    let mut sub = c.subscribe();
                                    tokio::spawn(async move {
                                        while let Ok(evt) = sub.recv().await {
                                            if tx.send(AppEvent::Ipc(evt)).is_err() {
                                                break;
                                            }
                                        }
                                    });
                                }

                                // Refetch state.
                                if let Ok(resp) = c.request(Request::ListGateways).await {
                                    if let Some(rows) = gw_rows_from_response(resp) {
                                        state.replace_gateways(rows);
                                    }
                                }
                                match state.active_tab {
                                    Tab::Usage => fetch_usage(&c, &mut state).await,
                                    Tab::Errors => fetch_errors(&c, &mut state).await,
                                    Tab::Settings => fetch_settings(&c, &mut state).await,
                                    _ => {}
                                }
                                break;
                            }
                            Err(_) => tokio::time::sleep(Duration::from_secs(2)).await,
                        }
                    }

                    state.status_message = None;
                    term.draw(|f| crate::view::render(f, &state))?;
                }
                // Loop back to select!
            }

            // ── Normal event arm ─────────────────────────────────────────────
            evt = rx.recv() => {
                let Some(evt) = evt else { break; };
                let client = client_slot.lock().await.clone();

                // ── Modal intercept ──────────────────────────────────────────
                if state.modal.is_some() {
                    // Clipboard copy: check before consuming the event.
                    if let AppEvent::Char('c') = &evt {
                        if let Some(Modal::Login(lf)) = &state.modal {
                            if let crate::app::modal::LoginUiState::WaitingForUser { user_code, .. } =
                                &lf.state
                            {
                                let code = user_code.clone();
                                let _ = arboard::Clipboard::new()
                                    .and_then(|mut c| c.set_text(code));
                                state.status_message = Some("Code copied".into());
                            }
                        }
                    }

                    let outcome = state.modal.as_mut().unwrap().handle(&evt);
                    match outcome {
                        ModalOutcome::Consumed => {}
                        ModalOutcome::Close => {
                            // If closing a Login modal while still pending, cancel it.
                            if let Some(Modal::Login(f)) = &state.modal {
                                use crate::app::modal::LoginUiState;
                                if matches!(
                                    f.state,
                                    LoginUiState::Initiating | LoginUiState::WaitingForUser { .. }
                                ) {
                                    let gid = f.gateway_id;
                                    let _ = client
                                        .request(Request::CancelLogin { gateway_id: gid })
                                        .await;
                                }
                            }
                            state.modal = None;
                        }
                        ModalOutcome::PassThrough => {
                            // Check if this is a LoginCompleted event — auto-open key/model select.
                            let login_gw = if let AppEvent::Ipc(llm_relay_core::ipc::Event::LoginCompleted { gateway_id, .. }) = &evt {
                                // Grab gateway info before state.handle consumes the event.
                                let gname = state.gateways.iter()
                                    .find(|r| r.id == *gateway_id)
                                    .map(|r| r.name.clone());
                                Some((*gateway_id, gname))
                            } else {
                                None
                            };
                            state.handle(evt);
                            // After login completes, auto-open SelectKeyModel.
                            if let Some((gid, Some(gname))) = login_gw {
                                open_select_key_model(&client, &mut state, gid, gname, &mut term).await?;
                            }
                        }
                        ModalOutcome::Submit(ms) => {
                            handle_submit(&client, &mut state, ms).await;
                        }
                    }
                    term.draw(|f| crate::view::render(f, &state))?;
                    if state.should_quit {
                        break;
                    }
                    continue;
                }

                // ── Context-sensitive pre-checks before state.handle ─────────
                let is_toggle_auto_launch =
                    matches!(&evt, AppEvent::Char('a')) && state.active_tab == Tab::Settings;
                let is_toggle_auto_failover =
                    matches!(&evt, AppEvent::Char('f')) && state.active_tab == Tab::Settings;
                let is_shutdown_agent =
                    matches!(&evt, AppEvent::Char('S')) && state.active_tab == Tab::Settings;

                // Open AddGateway modal on 'a' in Gateways tab.
                if matches!(&evt, AppEvent::Char('a'))
                    && state.active_tab == Tab::Gateways
                    && state.modal.is_none()
                {
                    state.modal = Some(Modal::AddGateway(Default::default()));
                    term.draw(|f| crate::view::render(f, &state))?;
                    continue;
                }

                // Open EditGateway modal on 'e' in Gateways tab.
                if matches!(&evt, AppEvent::Char('e'))
                    && state.active_tab == Tab::Gateways
                    && state.modal.is_none()
                {
                    if let Some(row) = state.gateways.get(state.selected_row) {
                        state.modal = Some(Modal::EditGateway(EditGatewayForm {
                            id: row.id,
                            name: row.name.clone(),
                            url: row.url.clone(),
                            focus: crate::app::modal::AddField::Name,
                            error: None,
                        }));
                    }
                    term.draw(|f| crate::view::render(f, &state))?;
                    continue;
                }

                // Open Login modal on 'l' in Gateways tab.
                if matches!(&evt, AppEvent::Char('l'))
                    && state.active_tab == Tab::Gateways
                    && state.modal.is_none()
                {
                    if let Some(row) = state.gateways.get(state.selected_row) {
                        let gid = row.id;
                        let gname = row.name.clone();
                        use crate::app::modal::{LoginForm, LoginUiState};
                        state.modal = Some(Modal::Login(LoginForm {
                            gateway_id: gid,
                            gateway_name: gname,
                            state: LoginUiState::Initiating,
                        }));
                        term.draw(|f| crate::view::render(f, &state))?;
                        // Fire StartLogin then patch modal state with response.
                        match client.request(Request::StartLogin { gateway_id: gid }).await {
                            Ok(Response::LoginInitiated {
                                user_code,
                                verification_uri,
                                expires_in_secs,
                                ..
                            }) => {
                                if let Some(Modal::Login(f)) = state.modal.as_mut() {
                                    f.state = LoginUiState::WaitingForUser {
                                        user_code,
                                        verification_uri,
                                        expires_in_secs,
                                    };
                                }
                            }
                            Ok(Response::Error { message }) => {
                                if let Some(Modal::Login(f)) = state.modal.as_mut() {
                                    f.state = LoginUiState::Failed(message);
                                }
                            }
                            Ok(_) => {}
                            Err(e) => {
                                if let Some(Modal::Login(f)) = state.modal.as_mut() {
                                    f.state = LoginUiState::Failed(e.to_string());
                                }
                            }
                        }
                    }
                    term.draw(|f| crate::view::render(f, &state))?;
                    continue;
                }

                // Set active gateway on 's' in Gateways tab.
                if matches!(&evt, AppEvent::Char('s'))
                    && state.active_tab == Tab::Gateways
                {
                    if let Some(row) = state.gateways.get(state.selected_row) {
                        let gid = row.id;
                        match client.request(Request::GetGatewayConfig { gateway_id: gid }).await {
                            Ok(Response::GatewayConfig { active_key_id: Some(key_id), claude, claude_small, codex, gemini }) => {
                                let models = llm_relay_core::ipc::ModelSelection {
                                    claude, claude_small, codex, gemini,
                                };
                                match client.request(Request::SetActive { gateway_id: gid, key_id, models }).await {
                                    Ok(Response::Ok) => {
                                        state.status_message = Some(format!("Activated: {}", row.name));
                                    }
                                    Ok(Response::Error { message }) => {
                                        state.status_message = Some(format!("SetActive failed: {message}"));
                                    }
                                    _ => {}
                                }
                                // Refresh gateway list to reflect active state.
                                if let Ok(resp) = client.request(Request::ListGateways).await {
                                    if let Some(rows) = gw_rows_from_response(resp) {
                                        state.replace_gateways(rows);
                                    }
                                }
                            }
                            Ok(Response::GatewayConfig { active_key_id: None, .. }) => {
                                state.status_message = Some("No key configured — press 'k' to set up".to_string());
                            }
                            Ok(Response::Error { message }) => {
                                state.status_message = Some(format!("Error: {message}"));
                            }
                            _ => {}
                        }
                    }
                    term.draw(|f| crate::view::render(f, &state))?;
                    continue;
                }

                // Open SelectKeyModel modal on 'k' in Gateways tab.
                if matches!(&evt, AppEvent::Char('k'))
                    && state.active_tab == Tab::Gateways
                    && state.modal.is_none()
                {
                    if let Some(row) = state.gateways.get(state.selected_row) {
                        let gid = row.id;
                        let gname = row.name.clone();
                        open_select_key_model(&client, &mut state, gid, gname, &mut term).await?;
                    }
                    term.draw(|f| crate::view::render(f, &state))?;
                    continue;
                }

                // Move gateway up/down with U/D in Gateways tab.
                if matches!(&evt, AppEvent::Char('U') | AppEvent::Char('D'))
                    && state.active_tab == Tab::Gateways
                    && state.gateways.len() > 1
                {
                    let moving_up = matches!(&evt, AppEvent::Char('U'));
                    let idx = state.selected_row;
                    let can_move = if moving_up { idx > 0 } else { idx + 1 < state.gateways.len() };
                    if can_move {
                        let swap_idx = if moving_up { idx - 1 } else { idx + 1 };
                        state.gateways.swap(idx, swap_idx);
                        state.selected_row = swap_idx;
                        // Rebuild index.
                        state.gateway_index.clear();
                        for (i, row) in state.gateways.iter().enumerate() {
                            state.gateway_index.insert(row.id, i);
                        }
                        // Persist new order.
                        let ids: Vec<uuid::Uuid> = state.gateways.iter().map(|r| r.id).collect();
                        let _ = client.request(Request::Reorder { ids }).await;
                    }
                    term.draw(|f| crate::view::render(f, &state))?;
                    continue;
                }

                // ── Normal event dispatch ────────────────────────────────────
                if matches!(evt, AppEvent::Refresh) {
                    if let Ok(resp) = client.request(Request::ListGateways).await {
                        if let Some(rows) = gw_rows_from_response(resp) {
                            state.replace_gateways(rows);
                        }
                    }
                    // Also refresh whichever data tab is active.
                    match state.active_tab {
                        Tab::Usage => fetch_usage(&client, &mut state).await,
                        Tab::Errors => fetch_errors(&client, &mut state).await,
                        Tab::Settings => fetch_settings(&client, &mut state).await,
                        _ => {}
                    }
                } else {
                    state.handle(evt);
                }

                // Context-sensitive 'a' on Settings tab: toggle auto-launch.
                if is_toggle_auto_launch {
                    let current = state
                        .settings
                        .snapshot
                        .as_ref()
                        .map(|s| s.auto_launch)
                        .unwrap_or(false);
                    let _ = client
                        .request(Request::SetAutoLaunch { enabled: !current })
                        .await;
                    fetch_settings(&client, &mut state).await;
                }

                // Context-sensitive 'f' on Settings tab: toggle auto-failover.
                if is_toggle_auto_failover {
                    let current = state
                        .settings
                        .snapshot
                        .as_ref()
                        .map(|s| s.auto_failover)
                        .unwrap_or(false);
                    let _ = client
                        .request(Request::SetAutoFailover { enabled: !current })
                        .await;
                    fetch_settings(&client, &mut state).await;
                }

                // Context-sensitive 'S' on Settings tab: shutdown agent + quit TUI.
                if is_shutdown_agent {
                    let _ = client.request(Request::Shutdown).await;
                    state.should_quit = true;
                }

                // On tab switch, fetch data for the newly activated tab.
                if state.active_tab != prev_tab {
                    match state.active_tab {
                        Tab::Usage => fetch_usage(&client, &mut state).await,
                        Tab::Errors => fetch_errors(&client, &mut state).await,
                        Tab::Settings => fetch_settings(&client, &mut state).await,
                        _ => {}
                    }
                    prev_tab = state.active_tab;
                }

                term.draw(|f| crate::view::render(f, &state))?;
                if state.should_quit {
                    break;
                }
            }
        }
    }
    Ok(())
}
