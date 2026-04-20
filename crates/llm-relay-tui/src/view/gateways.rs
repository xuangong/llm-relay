use crate::app::state::{AppState, GatewayRow};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
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

    let hint = Paragraph::new(
        "↑/↓ select  Enter expand  s star  a add  e edit  l login  d delete  r refresh  Tab next  q quit",
    )
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, chunks[1]);
}

fn row_to_item(_i: usize, row: &GatewayRow, _selected: bool) -> ListItem<'_> {
    let icon = match row.healthy {
        Some(true) => Span::styled("●", Style::default().fg(Color::Green)),
        Some(false) => Span::styled("●", Style::default().fg(Color::Red)),
        None => Span::styled("●", Style::default().fg(Color::DarkGray)),
    };
    let star = if row.starred { "★ " } else { "  " };
    let latency = row
        .latency_ms
        .map(|ms| format!(" {ms}ms"))
        .unwrap_or_default();
    let header = Line::from(vec![
        Span::raw(star),
        icon,
        Span::raw("  "),
        Span::styled(&row.name, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(&row.url, Style::default().fg(Color::DarkGray)),
        Span::raw(latency),
    ]);
    if row.expanded {
        let detail = Line::from(vec![Span::styled(
            format!("    id={}", row.id),
            Style::default().fg(Color::DarkGray),
        )]);
        ListItem::new(vec![header, detail])
    } else {
        ListItem::new(vec![header])
    }
}
