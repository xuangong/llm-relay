use crate::app::state::AppState;
use llm_relay_core::ipc::UsageRange;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(3), Constraint::Length(2)])
        .split(area);

    let range_label = match state.usage.range {
        UsageRange::Today => "Today",
        UsageRange::Last7Days => "Last 7 days",
        UsageRange::Last30Days => "Last 30 days",
        UsageRange::AllTime => "All time",
    };
    let header_p = Paragraph::new(format!("Range: {range_label}  (press 'p' to cycle)"))
        .style(Style::default().add_modifier(Modifier::BOLD));
    frame.render_widget(header_p, chunks[0]);

    let header = Row::new(vec!["Gateway", "Model", "Reqs", "In", "Out", "Cost ($)"])
        .style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan));
    let rows = state.usage.rows.iter().map(|r| {
        Row::new(vec![
            Cell::from(r.gateway_name.clone()),
            Cell::from(r.model.clone()),
            Cell::from(r.requests.to_string()),
            Cell::from(r.input_tokens.to_string()),
            Cell::from(r.output_tokens.to_string()),
            Cell::from(format!("{:.4}", r.cost_usd)),
        ])
    });
    let widths = [
        Constraint::Percentage(22),
        Constraint::Percentage(28),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("Usage"))
        .row_highlight_style(Style::default().bg(Color::DarkGray));
    let mut ts = TableState::default();
    ts.select(Some(state.usage.selected));
    frame.render_stateful_widget(table, chunks[1], &mut ts);

    let hint = Paragraph::new("p cycle range  r refresh  Tab next  q quit")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, chunks[2]);
}
