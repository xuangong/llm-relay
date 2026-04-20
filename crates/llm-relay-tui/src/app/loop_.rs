//! Main loop: drains crossterm key events and IPC events, applies them to
//! `AppState`, and re-renders.

use crate::app::{event::AppEvent, state::AppState, terminal::Tui};
use crate::ipc_client::IpcClient;
use crossterm::event::{self, Event as CtEvent, KeyCode, KeyEventKind};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

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

    // Initial render
    term.draw(|f| crate::view::render(f, &state))?;

    while let Some(evt) = rx.recv().await {
        state.handle(evt);
        term.draw(|f| crate::view::render(f, &state))?;
        if state.should_quit {
            break;
        }
    }
    Ok(())
}
