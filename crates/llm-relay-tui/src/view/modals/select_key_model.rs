use crate::app::modal::{SelectField, SelectKeyModelForm};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect, form: &SelectKeyModelForm) {
    let dialog = centered(60, 14, area);
    frame.render_widget(Clear, dialog);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Configure {} ", form.gateway_name))
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block.clone(), dialog);
    let inner = block.inner(dialog);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // error
            Constraint::Min(1),    // fields
            Constraint::Length(1), // hint
        ])
        .split(inner);

    // Error line
    if let Some(err) = &form.error {
        let p = Paragraph::new(err.as_str()).style(Style::default().fg(Color::Red));
        frame.render_widget(p, chunks[0]);
    }

    if form.loading_models {
        let p = Paragraph::new("Fetching models...");
        frame.render_widget(p, chunks[1]);
    } else {
        let mut lines = Vec::new();

        // Key selector
        let key_label = form
            .selected_key()
            .map(|k| k.name.as_str())
            .unwrap_or("(none)");
        lines.push(field_line("Key", key_label, form.focus == SelectField::Key));

        lines.push(Line::from(""));

        if let Some(cat) = &form.catalog {
            lines.push(model_line("Claude", &cat.claude, form.claude_idx, form.focus == SelectField::Claude));
            lines.push(model_line("Claude Small", &cat.claude, form.claude_small_idx, form.focus == SelectField::ClaudeSmall));
            lines.push(model_line("Codex", &cat.codex, form.codex_idx, form.focus == SelectField::Codex));
            lines.push(model_line("Gemini", &cat.gemini, form.gemini_idx, form.focus == SelectField::Gemini));
        } else {
            lines.push(Line::from("No model catalog"));
        }

        let p = Paragraph::new(lines);
        frame.render_widget(p, chunks[1]);
    }

    let hint = Paragraph::new("←/→ change  ↑/↓ field  Enter apply  Esc cancel")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, chunks[2]);
}

fn field_line<'a>(label: &'a str, value: &'a str, focused: bool) -> Line<'a> {
    let style = if focused {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let arrow = if focused { "◄ " } else { "  " };
    let arrow_r = if focused { " ►" } else { "" };
    Line::from(vec![
        Span::styled(format!("{label:>12}: "), Style::default().fg(Color::DarkGray)),
        Span::raw(arrow),
        Span::styled(value, style),
        Span::raw(arrow_r),
    ])
}

fn model_line<'a>(label: &'a str, options: &'a [String], idx: usize, focused: bool) -> Line<'a> {
    let value = options.get(idx).map(|s| s.as_str()).unwrap_or("(none)");
    field_line(label, value, focused)
}

fn centered(w: u16, h: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect { x, y, width: w.min(area.width), height: h.min(area.height) }
}
