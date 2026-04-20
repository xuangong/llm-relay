use llm_relay_tui::app::state::{AppState, Tab};
use llm_relay_tui::app::event::AppEvent;

#[test]
fn tab_navigation_cycles_through_all_tabs() {
    let mut s = AppState::new();
    assert_eq!(s.active_tab, Tab::Gateways);
    s.handle(AppEvent::NextTab);
    assert_eq!(s.active_tab, Tab::Usage);
    s.handle(AppEvent::NextTab);
    assert_eq!(s.active_tab, Tab::Errors);
    s.handle(AppEvent::NextTab);
    assert_eq!(s.active_tab, Tab::Settings);
    s.handle(AppEvent::NextTab);
    assert_eq!(s.active_tab, Tab::Gateways);
}

#[test]
fn quit_event_sets_should_quit_flag() {
    let mut s = AppState::new();
    assert!(!s.should_quit);
    s.handle(AppEvent::Quit);
    assert!(s.should_quit);
}
