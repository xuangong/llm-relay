use llm_relay_tui::app::state::AppState;
use llm_relay_core::ipc::UsageRange;

#[test]
fn cycle_range_visits_all_then_wraps() {
    let mut s = AppState::new();
    assert_eq!(s.usage.range, UsageRange::Today);
    s.cycle_usage_range(); assert_eq!(s.usage.range, UsageRange::Last7Days);
    s.cycle_usage_range(); assert_eq!(s.usage.range, UsageRange::Last30Days);
    s.cycle_usage_range(); assert_eq!(s.usage.range, UsageRange::AllTime);
    s.cycle_usage_range(); assert_eq!(s.usage.range, UsageRange::Today);
}
