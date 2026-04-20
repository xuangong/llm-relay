use crate::app::state::AppState;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);

    let lines: Vec<Line> = match state.settings.snapshot.as_ref() {
        None => vec![Line::from("Loading settings...")],
        Some(s) => vec![
            kv_owned("Keystore", s.keystore_kind.clone(), kind_color(&s.keystore_kind)),
            kv_owned("Agent PID", s.agent_pid.to_string(), Color::White),
            kv_owned("Socket", s.socket_path.clone(), Color::White),
            kv_owned("Proxy port", s.proxy_port.to_string(), Color::White),
            kv_owned("Log path", s.log_path.clone(), Color::White),
            kv_owned(
                "Auto-launch on boot",
                if s.auto_launch { "ON".to_string() } else { "OFF".to_string() },
                if s.auto_launch { Color::Green } else { Color::DarkGray },
            ),
        ],
    };
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Settings"));
    frame.render_widget(p, chunks[0]);

    let hint = Paragraph::new("a toggle auto-launch  r refresh  Tab next  q quit")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, chunks[1]);
}

fn kv_owned(k: &str, v: String, c: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {k:<20}"), Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(v, Style::default().fg(c)),
    ])
}

fn kind_color(kind: &str) -> Color {
    match kind {
        "system" => Color::Green,
        "encrypted-file" => Color::Yellow,
        _ => Color::Red,
    }
}
