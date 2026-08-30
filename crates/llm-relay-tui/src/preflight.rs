//! First-run master-key wizard.
//!
//! The headless agent refuses to start without `LLM_RELAY_MASTER_KEY` — it is
//! the only thing standing between `secrets.env.enc` and whoever copies that
//! file off the box. Without a wizard the first-run experience is the agent
//! exiting 2 before the TUI ever draws a frame, so we ask here instead: offer
//! to generate a key, show it exactly once, and hand it to the agent through
//! its environment.
//!
//! The key is never written to disk by us. Storing it next to the ciphertext
//! it protects would make the encryption decoration; where it lives (systemd
//! `EnvironmentFile=`, a password manager, a secrets mount) is the operator's
//! call. It is displayed inside the alternate screen, which the terminal
//! discards on exit rather than committing to scrollback.

use crate::app::terminal::Tui;
use crossterm::event::{self, Event as CtEvent, KeyCode, KeyEventKind};
use llm_relay_core::keystore::{self, EnvInitError, ENV_KEY_VAR};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use std::path::Path;

/// Make sure a usable master key exists, prompting if it doesn't.
///
/// Returns the extra environment to spawn the agent with: empty when the
/// caller's own environment already carries a working key (the child inherits
/// it), or a single `LLM_RELAY_MASTER_KEY` pair when the wizard just made one.
pub fn ensure_master_key(term: &mut Tui, config_dir: &Path) -> anyhow::Result<Vec<(String, String)>> {
    match keystore::probe_env(config_dir) {
        // A key is set and it opens the store — nothing to do.
        Ok(()) => Ok(Vec::new()),

        // A key exists but doesn't fit the store. Generating another one would
        // produce a second wrong key, so there is nothing to offer here.
        Err(e @ EnvInitError::UnreadableStore(_)) | Err(e @ EnvInitError::AlreadyInitialized) => {
            Err(anyhow::anyhow!("{e}"))
        }

        Err(EnvInitError::MissingKey(_)) => {
            let store = config_dir.join(keystore::ENV_STORE_FILE);
            if store.exists() {
                // Sealed secrets with no key in sight. A fresh key cannot open
                // them, and quietly starting over would throw away every signed-in
                // gateway, so say what the two real options are and stop.
                notice(
                    term,
                    " Master key missing ",
                    orphaned_store_lines(&store),
                )?;
                anyhow::bail!(
                    "{ENV_KEY_VAR} is not set and {} already exists — set the original key, \
                     or move that file aside to start over",
                    store.display()
                );
            }
            wizard(term, &store)
        }
    }
}

/// Two screens: explain, then reveal. Esc on either one aborts startup —
/// continuing without a key would just defer the failure to the agent.
fn wizard(term: &mut Tui, store: &Path) -> anyhow::Result<Vec<(String, String)>> {
    if !confirm(term, " LLM Relay — first run ", intro_lines(store), "Enter  generate a key    Esc  quit")? {
        anyhow::bail!("setup cancelled — set {ENV_KEY_VAR} yourself and start again");
    }

    let key = keystore::generate_master_key();
    let mut copied = false;
    loop {
        let hint = if copied {
            "copied to clipboard    Enter  I saved it, continue    Esc  quit"
        } else {
            "c  copy    Enter  I saved it, continue    Esc  quit"
        };
        draw(term, " Save this key now ", reveal_lines(&key), hint)?;
        match key_press()? {
            KeyCode::Enter => break,
            KeyCode::Esc => anyhow::bail!("setup cancelled — no key was saved"),
            KeyCode::Char('c') => {
                // Headless servers routinely have no clipboard. Failing here
                // would be worse than useless: the key is on screen either way.
                copied = arboard::Clipboard::new()
                    .and_then(|mut c| c.set_text(key.clone()))
                    .is_ok();
            }
            _ => {}
        }
    }

    Ok(vec![(ENV_KEY_VAR.to_string(), key)])
}

fn intro_lines(store: &Path) -> Vec<Line<'static>> {
    vec![
        Line::from("Gateway tokens are kept encrypted at:"),
        Line::from(Span::styled(
            store.display().to_string(),
            Style::default().fg(Color::Cyan),
        )),
        Line::from(""),
        Line::from(format!(
            "The key that decrypts them is read from ${ENV_KEY_VAR} and is never \
             written to disk by this program — that is what makes the file safe to \
             back up. You have not set it, so there is nothing to start the agent with."
        )),
        Line::from(""),
        Line::from("Generating one takes a second. It will be shown once, and only once."),
    ]
}

fn reveal_lines(key: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "This is the only time it will be displayed.",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("{ENV_KEY_VAR}={key}"),
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Store it somewhere durable before continuing:",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  · a password manager, or"),
        Line::from("  · /etc/llm-relay/master.env  (chmod 600), referenced from your unit as"),
        Line::from(Span::styled(
            "    EnvironmentFile=/etc/llm-relay/master.env",
            Style::default().fg(Color::Cyan),
        )),
        Line::from(""),
        Line::from("Lose it and the stored tokens are unrecoverable — you would sign in again."),
    ]
}

fn orphaned_store_lines(store: &Path) -> Vec<Line<'static>> {
    vec![
        Line::from("An encrypted keystore already exists:"),
        Line::from(Span::styled(
            store.display().to_string(),
            Style::default().fg(Color::Cyan),
        )),
        Line::from(""),
        Line::from(format!(
            "but ${ENV_KEY_VAR} is not set. A new key cannot open it, so this wizard \
             has nothing to offer. Either:"
        )),
        Line::from(""),
        Line::from("  · export the original key and start the TUI again, or"),
        Line::from("  · move that file aside and sign in to your gateways again."),
    ]
}

// ── rendering ───────────────────────────────────────────────────────────────

fn draw(term: &mut Tui, title: &str, body: Vec<Line<'static>>, hint: &str) -> anyhow::Result<()> {
    term.draw(|frame| {
        let area = frame.area();
        let w = 80.min(area.width);
        // Size the box to the *wrapped* body, not the line count. Several of
        // these lines are prose that reflows, and a box sized to the unwrapped
        // count silently clips the last thing the operator needs to read.
        let inner_w = w.saturating_sub(2).max(1) as usize;
        let rows: usize = body
            .iter()
            .map(|l| l.width().max(1).div_ceil(inner_w))
            .sum();
        // + borders, the blank spacer, and the hint line.
        let dialog = centered(w, (rows as u16).saturating_add(4), area);

        frame.render_widget(Clear, dialog);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::Cyan));
        let inner = block.inner(dialog);
        frame.render_widget(block, dialog);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)])
            .split(inner);
        frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), chunks[0]);
        frame.render_widget(
            Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
            chunks[2],
        );
    })?;
    Ok(())
}

/// Draw and wait for Enter (true) or Esc (false).
fn confirm(term: &mut Tui, title: &str, body: Vec<Line<'static>>, hint: &str) -> anyhow::Result<bool> {
    draw(term, title, body, hint)?;
    loop {
        match key_press()? {
            KeyCode::Enter => return Ok(true),
            KeyCode::Esc => return Ok(false),
            _ => {}
        }
    }
}

/// Draw and wait for any key. Used for dead ends, where the message matters
/// more than the choice — the caller errors out afterwards regardless.
fn notice(term: &mut Tui, title: &str, body: Vec<Line<'static>>) -> anyhow::Result<()> {
    draw(term, title, body, "press any key to quit")?;
    key_press()?;
    Ok(())
}

/// Block until a key goes down. Key *releases* are separate events on Windows
/// and would otherwise dismiss a screen the instant the previous one was
/// acknowledged.
fn key_press() -> anyhow::Result<KeyCode> {
    loop {
        if let CtEvent::Key(k) = event::read()? {
            if k.kind == KeyEventKind::Press {
                return Ok(k.code);
            }
        }
    }
}

fn centered(w: u16, h: u16, area: Rect) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}
