use llm_relay_tui::app::state::{AppState, GatewayRow};
use llm_relay_tui::app::event::AppEvent;
use uuid::Uuid;

fn make_state(n: usize) -> AppState {
    let mut s = AppState::new();
    let rows = (0..n).map(|i| GatewayRow {
        id: Uuid::new_v4(),
        name: format!("gw-{i}"),
        url: format!("http://example.com/{i}"),
        ..GatewayRow::default()
    }).collect();
    s.replace_gateways(rows);
    s
}

#[test]
fn pressing_s_does_not_mutate_star_in_state_layer() {
    let mut s = make_state(3);
    assert!(!s.gateways[0].starred);
    s.handle(AppEvent::Char('s'));
    assert!(!s.gateways[0].starred);
}

#[test]
fn down_then_enter_expands_second_row() {
    let mut s = make_state(3);
    s.handle(AppEvent::Down);
    s.handle(AppEvent::Enter);
    assert!(!s.gateways[0].expanded);
    assert!(s.gateways[1].expanded);
    assert!(!s.gateways[2].expanded);
}
