use llm_relay_tui::app::event::AppEvent;
use llm_relay_tui::app::modal::{LoginForm, LoginUiState, Modal, ModalOutcome};
use uuid::Uuid;

fn make_login_modal(state: LoginUiState) -> Modal {
    Modal::Login(LoginForm {
        gateway_id: Uuid::new_v4(),
        gateway_name: "test-gw".into(),
        state,
    })
}

#[test]
fn login_esc_closes_in_initiating() {
    let mut m = make_login_modal(LoginUiState::Initiating);
    let outcome = m.handle(&AppEvent::Esc);
    assert!(matches!(outcome, ModalOutcome::Close));
}

#[test]
fn login_esc_closes_in_waiting() {
    let mut m = make_login_modal(LoginUiState::WaitingForUser {
        user_code: "ABC-123".into(),
        verification_uri: "https://example.com/login".into(),
        expires_in_secs: 300,
    });
    let outcome = m.handle(&AppEvent::Esc);
    assert!(matches!(outcome, ModalOutcome::Close));
}

#[test]
fn login_c_consumed_in_waiting() {
    let mut m = make_login_modal(LoginUiState::WaitingForUser {
        user_code: "XYZ-789".into(),
        verification_uri: "https://example.com/login".into(),
        expires_in_secs: 120,
    });
    let outcome = m.handle(&AppEvent::Char('c'));
    assert!(matches!(outcome, ModalOutcome::Consumed));
}

#[test]
fn login_esc_closes_in_completed() {
    let mut m = make_login_modal(LoginUiState::Completed);
    let outcome = m.handle(&AppEvent::Esc);
    assert!(matches!(outcome, ModalOutcome::Close));
}

#[test]
fn login_esc_closes_in_failed() {
    let mut m = make_login_modal(LoginUiState::Failed("network error".into()));
    let outcome = m.handle(&AppEvent::Esc);
    assert!(matches!(outcome, ModalOutcome::Close));
}

#[test]
fn login_esc_closes_in_expired() {
    let mut m = make_login_modal(LoginUiState::Expired);
    let outcome = m.handle(&AppEvent::Esc);
    assert!(matches!(outcome, ModalOutcome::Close));
}

#[test]
fn login_state_transitions_via_apply_ipc() {
    use llm_relay_core::ipc::Event as IpcEvent;
    use llm_relay_tui::app::event::AppEvent;
    use llm_relay_tui::app::state::AppState;

    let mut state = AppState::new();
    let gid = Uuid::new_v4();
    state.modal = Some(Modal::Login(LoginForm {
        gateway_id: gid,
        gateway_name: "gw".into(),
        state: LoginUiState::WaitingForUser {
            user_code: "CODE".into(),
            verification_uri: "https://x".into(),
            expires_in_secs: 60,
        },
    }));

    // Simulate LoginCompleted event.
    state.handle(AppEvent::Ipc(IpcEvent::LoginCompleted {
        gateway_id: gid,
        session_token: "tok".into(),
        user_id: None,
        user_name: None,
    }));

    if let Some(Modal::Login(f)) = &state.modal {
        assert!(matches!(f.state, LoginUiState::Completed));
    } else {
        panic!("modal should still be Some");
    }
}
