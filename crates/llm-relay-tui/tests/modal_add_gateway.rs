use llm_relay_tui::app::event::AppEvent;
use llm_relay_tui::app::modal::{AddGatewayForm, Modal, ModalOutcome, ModalSubmit};

#[test]
fn enter_with_blank_name_sets_error_and_does_not_submit() {
    let mut m = Modal::AddGateway(AddGatewayForm::default());
    let outcome = m.handle(&AppEvent::Enter);
    assert!(matches!(outcome, ModalOutcome::Consumed));
    if let Modal::AddGateway(f) = &m {
        assert!(f.error.is_some());
    }
}

#[test]
fn typing_into_name_then_url_then_enter_submits_with_values() {
    let mut m = Modal::AddGateway(AddGatewayForm::default());
    for c in "gw1".chars() {
        m.handle(&AppEvent::Char(c));
    }
    m.handle(&AppEvent::Down); // focus URL
    for c in "https://x".chars() {
        m.handle(&AppEvent::Char(c));
    }
    let outcome = m.handle(&AppEvent::Enter);
    match outcome {
        ModalOutcome::Submit(ModalSubmit::AddGateway { name, url }) => {
            assert_eq!(name, "gw1");
            assert_eq!(url, "https://x");
        }
        other => panic!("expected submit, got {other:?}"),
    }
}

#[test]
fn esc_closes() {
    let mut m = Modal::AddGateway(AddGatewayForm::default());
    let outcome = m.handle(&AppEvent::Esc);
    assert!(matches!(outcome, ModalOutcome::Close));
}
