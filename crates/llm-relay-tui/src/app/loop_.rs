//! Main loop: drains crossterm key events and IPC events, applies them to
//! `AppState`, and re-renders.

use crate::app::{
    event::AppEvent,
    modal::{EditGatewayForm, Modal, ModalOutcome, ModalSubmit},
    state::{AppState, GatewayRow, Tab},
    terminal::Tui,
};
use crate::ipc_client::IpcClient;
use crossterm::event::{self, Event as CtEvent, KeyCode, KeyEventKind};
use llm_relay_core::ipc::{GatewaySummary, Request, Response};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn into_row(g: GatewaySummary) -> GatewayRow {
    GatewayRow {
        id: g.id,
        name: g.name,
        url: g.url,
        healthy: g.healthy,
        latency_ms: g.latency_ms,
        starred: g.starred,
        expanded: false,
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
    if let Ok(Response::UsageRows { rows }) =
        client.request(Request::GetUsageRows { range: state.usage.range }).await
    {
        state.usage.rows = rows;
    }
}

/// Fetch error rows from the agent and store them in state.
async fn fetch_errors(client: &IpcClient, state: &mut AppState) {
    if let Ok(Response::ErrorRows { rows }) =
        client.request(Request::GetErrors { limit: 100 }).await
    {
        state.errors.rows = rows;
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
    }
}

pub async fn run(mut term: Tui, client: Arc<IpcClient>) -> std::io::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

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
                        KeyCode::Enter => AppEvent::Enter,
                        KeyCode::Esc => AppEvent::Esc,
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

    // Spawn IPC event forwarder.
    {
        let tx = tx.clone();
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
    if let Ok(resp) = client.request(Request::ListGateways).await {
        if let Some(rows) = gw_rows_from_response(resp) {
            state.replace_gateways(rows);
        }
    }

    // Initial render.
    term.draw(|f| crate::view::render(f, &state))?;

    while let Some(evt) = rx.recv().await {
        // ── Modal intercept ──────────────────────────────────────────────────
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
                    state.handle(evt);
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

        // ── Context-sensitive pre-checks before state.handle ─────────────────
        let is_toggle_auto_launch =
            matches!(&evt, AppEvent::Char('a')) && state.active_tab == Tab::Settings;

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

        // ── Normal event dispatch ─────────────────────────────────────────────
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
            // Re-fetch to reflect the change.
            fetch_settings(&client, &mut state).await;
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
    Ok(())
}
