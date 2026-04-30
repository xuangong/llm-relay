pub mod gateways;
pub mod usage;
pub mod errors;
pub mod settings;
pub mod modals;

use crate::app::state::{AppState, Tab};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Tabs};
use ratatui::Frame;

pub fn render(frame: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(frame.area());

    let titles: Vec<Line> = ["Gateways", "Usage", "Errors", "Settings"]
        .iter()
        .copied()
        .map(Line::from)
        .collect();
    let selected = match state.active_tab {
        Tab::Gateways => 0,
        Tab::Usage => 1,
        Tab::Errors => 2,
        Tab::Settings => 3,
    };
    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title("LLM Relay"))
        .select(selected)
        .highlight_style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan));
    frame.render_widget(tabs, chunks[0]);

    match state.active_tab {
        Tab::Gateways => gateways::render(frame, chunks[1], state),
        Tab::Usage => usage::render(frame, chunks[1], state),
        Tab::Errors => errors::render(frame, chunks[1], state),
        Tab::Settings => settings::render(frame, chunks[1], state),
    }

    if let Some(modal) = &state.modal {
        use crate::app::modal::Modal;
        let area = frame.area();
        match modal {
            Modal::AddGateway(f) => modals::add_gateway::render(frame, area, f),
            Modal::EditGateway(f) => modals::edit_gateway::render(frame, area, f),
            Modal::Login(f) => modals::login::render(frame, area, f),
            Modal::SelectKeyModel(f) => modals::select_key_model::render(frame, area, f),
        }
    }
}
