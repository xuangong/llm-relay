use crate::app::state::{AppState, GatewayRow};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect, state: &AppState) {
    let has_status = state.status_message.is_some();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if has_status {
            vec![Constraint::Min(3), Constraint::Length(1), Constraint::Length(2)]
        } else {
            vec![Constraint::Min(3), Constraint::Length(0), Constraint::Length(2)]
        })
        .split(area);

    let items: Vec<ListItem> = state
        .gateways
        .iter()
        .enumerate()
        .map(|(i, row)| row_to_item(i, row, i == state.selected_row))
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Gateways"))
        .highlight_style(Style::default().bg(Color::DarkGray));
    let mut list_state = ListState::default();
    list_state.select(Some(state.selected_row));
    frame.render_stateful_widget(list, chunks[0], &mut list_state);

    if let Some(msg) = &state.status_message {
        let p = Paragraph::new(msg.as_str()).style(Style::default().fg(Color::Yellow));
        frame.render_widget(p, chunks[1]);
    }

    let hint = Paragraph::new(
        "↑/↓ select  U/D reorder  Enter expand  s activate  a add  e edit  l login  k config  d delete  r refresh  Tab next  q quit",
    )
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, chunks[2]);
}

fn row_to_item(_i: usize, row: &GatewayRow, _selected: bool) -> ListItem<'_> {
    let icon = match row.healthy {
        Some(true) => Span::styled("●", Style::default().fg(Color::Green)),
        Some(false) => Span::styled("●", Style::default().fg(Color::Red)),
        None => Span::styled("●", Style::default().fg(Color::DarkGray)),
    };
    let active_marker = if row.starred { Span::styled("★ ", Style::default().fg(Color::Yellow)) } else { Span::raw("  ") };
    let latency = row
        .latency_ms
        .map(|ms| format!(" {ms}ms"))
        .unwrap_or_default();
    // 🔒 indicates the gateway has no auth_key on file — proxy traffic will 401
    // until the user runs the device-code flow.
    let lock_hint: Span = if row.needs_login {
        Span::styled(" [!login]", Style::default().fg(Color::Yellow))
    } else {
        Span::raw("")
    };
    let header = Line::from(vec![
        active_marker,
        icon,
        Span::raw("  "),
        Span::styled(&row.name, Style::default().add_modifier(Modifier::BOLD)),
        lock_hint,
        Span::raw("  "),
        Span::styled(&row.url, Style::default().fg(Color::Gray)),
        Span::raw(latency),
    ]);
    if row.expanded {
        let dim = Style::default().fg(Color::Gray);
        let mut lines = vec![header];

        // Session / user
        let session = match &row.user_name {
            Some(u) => format!("    session: {u}"),
            None if row.needs_login => "    session: not logged in".to_string(),
            None => "    session: anonymous".to_string(),
        };
        lines.push(Line::from(Span::styled(session, dim)));

        // Active key
        if let Some(key) = &row.active_key_name {
            lines.push(Line::from(Span::styled(format!("    key: {key}"), dim)));
        }

        // Models
        let models: Vec<String> = [
            row.claude_model.as_deref().map(|m| format!("claude={m}")),
            row.claude_small_model.as_deref().map(|m| format!("claude_small={m}")),
            row.codex_model.as_deref().map(|m| format!("codex={m}")),
            row.gemini_model.as_deref().map(|m| format!("gemini={m}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !models.is_empty() {
            lines.push(Line::from(Span::styled(format!("    models: {}", models.join(", ")), dim)));
        }

        ListItem::new(lines)
    } else {
        ListItem::new(vec![header])
    }
}
