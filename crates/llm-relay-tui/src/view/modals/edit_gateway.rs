use crate::app::modal::{AddField, EditGatewayForm};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect, form: &EditGatewayForm) {
    let dialog = centered(60, 11, area);
    frame.render_widget(Clear, dialog);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Edit Gateway ")
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block.clone(), dialog);

    let inner = block.inner(dialog);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(field_label("Name", form.focus == AddField::Name), chunks[0]);
    frame.render_widget(field_value(&form.name, form.focus == AddField::Name), chunks[1]);
    frame.render_widget(field_label("URL", form.focus == AddField::Url), chunks[2]);
    frame.render_widget(field_value(&form.url, form.focus == AddField::Url), chunks[3]);

    if let Some(err) = &form.error {
        let p = Paragraph::new(err.as_str()).style(Style::default().fg(Color::Red));
        frame.render_widget(p, chunks[4]);
    }

    let hint = Paragraph::new("↑/↓ field  Enter submit  Esc cancel")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, chunks[5]);
}

fn field_label(label: &str, focused: bool) -> Paragraph<'_> {
    let style = if focused {
        Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    };
    Paragraph::new(label).style(style)
}

fn field_value<'a>(value: &'a str, focused: bool) -> Paragraph<'a> {
    let display = if focused { format!("> {value}_") } else { format!("  {value}") };
    Paragraph::new(display).block(Block::default().borders(Borders::BOTTOM))
}

fn centered(w: u16, h: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect { x, y, width: w.min(area.width), height: h.min(area.height) }
}
