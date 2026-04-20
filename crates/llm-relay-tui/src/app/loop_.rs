//! Main loop: drains crossterm key events and IPC events, applies them to
//! `AppState`, and re-renders.

use crate::app::{event::AppEvent, state::{AppState, GatewayRow, Tab}, terminal::Tui};
use crate::ipc_client::IpcClient;
use crossterm::event::{self, Event as CtEvent, KeyCode, KeyEventKind};
use llm_relay_core::ipc::{Request, Response};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn gw_rows_from_response(resp: Response) -> Option<Vec<GatewayRow>> {
    if let Response::GatewayList { gateways } = resp {
        Some(
            gateways
                .into_iter()
                .map(|g| GatewayRow {
                    id: g.id,
                    name: g.name,
                    url: g.url,
                    healthy: g.healthy,
                    latency_ms: g.latency_ms,
                    starred: g.starred,
                    expanded: false,
                })
                .collect(),
        )
    } else {
        None
    }
}

/// Fetch usage rows from the agent and store them in state.
async fn fetch_usage(client: &IpcClient, state: &mut AppState) {
    if let Ok(Response::UsageRows { rows }) = client.request(Request::GetUsageRows { range: state.usage.range }).await {
        state.usage.rows = rows;
    }
}

/// Fetch error rows from the agent and store them in state.
async fn fetch_errors(client: &IpcClient, state: &mut AppState) {
    if let Ok(Response::ErrorRows { rows }) = client.request(Request::GetErrors { limit: 100 }).await {
        state.errors.rows = rows;
    }
}

/// Fetch TUI settings snapshot from the agent and store in state.
async fn fetch_settings(client: &IpcClient, state: &mut AppState) {
    if let Ok(Response::TuiSettings(s)) = client.request(Request::GetTuiSettings).await {
        state.settings.snapshot = Some(s);
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
        // Capture whether this is a Char('a') on the Settings tab before mutating state.
        let is_toggle_auto_launch = matches!(&evt, AppEvent::Char('a'))
            && state.active_tab == Tab::Settings;

        // Refresh short-circuits before state.handle so we can do async IPC.
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
            let current = state.settings.snapshot.as_ref().map(|s| s.auto_launch).unwrap_or(false);
            let _ = client.request(Request::SetAutoLaunch { enabled: !current }).await;
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
