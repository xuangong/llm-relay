use crate::app::state::AppState;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);

    // Surface fetch errors (e.g. NotImplemented) instead of an empty table.
    if let Some(msg) = &state.errors.error {
        let banner = Paragraph::new(format!("尚未实现 / Not yet implemented ({msg})"))
            .style(Style::default().fg(Color::Yellow))
            .block(Block::default().borders(Borders::ALL).title("Recent Errors"));
        frame.render_widget(banner, chunks[0]);
        let hint = Paragraph::new("r refresh  Tab next  q quit")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(hint, chunks[1]);
        return;
    }

    let header = Row::new(vec!["Time", "Gateway", "Kind", "Message"])
        .style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan));
    let rows = state.errors.rows.iter().map(|r| {
        Row::new(vec![
            Cell::from(r.timestamp_iso.clone()),
            Cell::from(r.gateway_name.clone()),
            Cell::from(r.kind.clone()).style(match r.kind.as_str() {
                "auth" => Style::default().fg(Color::Yellow),
                "proxy" => Style::default().fg(Color::Red),
                _ => Style::default().fg(Color::Magenta),
            }),
            Cell::from(r.message.clone()),
        ])
    });
    let widths = [
        Constraint::Length(20),
        Constraint::Percentage(20),
        Constraint::Length(8),
        Constraint::Min(10),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("Recent Errors"))
        .row_highlight_style(Style::default().bg(Color::DarkGray));
    let mut ts = TableState::default();
    ts.select(Some(state.errors.selected));
    frame.render_stateful_widget(table, chunks[0], &mut ts);

    let hint = Paragraph::new("r refresh  Tab next  q quit")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, chunks[1]);
}
