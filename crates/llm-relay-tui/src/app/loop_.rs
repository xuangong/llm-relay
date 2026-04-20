//! Main loop: drains crossterm key events and IPC events, applies them to
//! `AppState`, and re-renders.

use crate::app::{event::AppEvent, state::{AppState, GatewayRow}, terminal::Tui};
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

    // Initial gateway load.
    if let Ok(resp) = client.request(Request::ListGateways).await {
        if let Some(rows) = gw_rows_from_response(resp) {
            state.replace_gateways(rows);
        }
    }

    // Initial render.
    term.draw(|f| crate::view::render(f, &state))?;

    while let Some(evt) = rx.recv().await {
        // Refresh short-circuits before state.handle so we can do async IPC.
        if matches!(evt, AppEvent::Refresh) {
            if let Ok(resp) = client.request(Request::ListGateways).await {
                if let Some(rows) = gw_rows_from_response(resp) {
                    state.replace_gateways(rows);
                }
            }
        } else {
            state.handle(evt);
        }
        term.draw(|f| crate::view::render(f, &state))?;
        if state.should_quit {
            break;
        }
    }
    Ok(())
}
