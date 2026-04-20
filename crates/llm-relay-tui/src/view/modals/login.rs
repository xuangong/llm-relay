use crate::app::modal::{LoginForm, LoginUiState};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect, form: &LoginForm) {
    let dialog = centered(70, 13, area);
    frame.render_widget(Clear, dialog);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Sign in to {} ", form.gateway_name))
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block.clone(), dialog);
    let inner = block.inner(dialog);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let (body, hint) = match &form.state {
        LoginUiState::Initiating => (
            vec![Line::from("Requesting device code...")],
            "Esc cancel",
        ),
        LoginUiState::WaitingForUser { user_code, verification_uri, expires_in_secs } => {
            let lines = vec![
                Line::from(vec![
                    Span::styled(
                        "Open this URL in any browser:",
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(Span::styled(
                    verification_uri.clone(),
                    Style::default().fg(Color::Cyan),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        "Enter the code:",
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(Span::styled(
                    user_code.clone(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(format!("Expires in {expires_in_secs}s")),
            ];
            (lines, "c copy code  Esc cancel")
        }
        LoginUiState::Completed => (
            vec![Line::from(Span::styled(
                "Signed in successfully",
                Style::default().fg(Color::Green),
            ))],
            "Esc close",
        ),
        LoginUiState::Failed(msg) => (
            vec![Line::from(Span::styled(
                format!("Login failed: {msg}"),
                Style::default().fg(Color::Red),
            ))],
            "Esc close",
        ),
        LoginUiState::Expired => (
            vec![Line::from(Span::styled(
                "Code expired — please try again",
                Style::default().fg(Color::Yellow),
            ))],
            "Esc close",
        ),
    };

    let p = Paragraph::new(body);
    frame.render_widget(p, chunks[1]);
    let h = Paragraph::new(hint).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(h, chunks[2]);
}

fn centered(w: u16, h: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect { x, y, width: w.min(area.width), height: h.min(area.height) }
}
