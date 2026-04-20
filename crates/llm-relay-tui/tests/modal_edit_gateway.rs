use llm_relay_tui::app::event::AppEvent;
use llm_relay_tui::app::modal::{AddField, EditGatewayForm, Modal, ModalOutcome, ModalSubmit};
use uuid::Uuid;

#[test]
fn edit_enter_with_blank_name_sets_error() {
    let mut m = Modal::EditGateway(EditGatewayForm {
        id: Uuid::new_v4(),
        name: String::new(),
        url: "https://example.com".into(),
        focus: AddField::Name,
        error: None,
    });
    let outcome = m.handle(&AppEvent::Enter);
    assert!(matches!(outcome, ModalOutcome::Consumed));
    if let Modal::EditGateway(f) = &m {
        assert!(f.error.is_some());
    }
}

#[test]
fn edit_enter_with_bad_url_sets_error() {
    let mut m = Modal::EditGateway(EditGatewayForm {
        id: Uuid::new_v4(),
        name: "mygw".into(),
        url: "not-a-url".into(),
        focus: AddField::Name,
        error: None,
    });
    let outcome = m.handle(&AppEvent::Enter);
    assert!(matches!(outcome, ModalOutcome::Consumed));
    if let Modal::EditGateway(f) = &m {
        assert!(f.error.is_some());
    }
}

#[test]
fn edit_valid_enter_submits() {
    let id = Uuid::new_v4();
    let mut m = Modal::EditGateway(EditGatewayForm {
        id,
        name: "updated".into(),
        url: "https://new-url.com".into(),
        focus: AddField::Name,
        error: None,
    });
    let outcome = m.handle(&AppEvent::Enter);
    match outcome {
        ModalOutcome::Submit(ModalSubmit::EditGateway { id: eid, name, url }) => {
            assert_eq!(eid, id);
            assert_eq!(name, "updated");
            assert_eq!(url, "https://new-url.com");
        }
        other => panic!("expected Submit(EditGateway), got {other:?}"),
    }
}

#[test]
fn edit_esc_closes() {
    let mut m = Modal::EditGateway(EditGatewayForm::default());
    let outcome = m.handle(&AppEvent::Esc);
    assert!(matches!(outcome, ModalOutcome::Close));
}

#[test]
fn edit_typing_appends_to_focused_field() {
    let mut m = Modal::EditGateway(EditGatewayForm {
        id: Uuid::nil(),
        name: "base".into(),
        url: String::new(),
        focus: AddField::Name,
        error: None,
    });
    m.handle(&AppEvent::Char('X'));
    if let Modal::EditGateway(f) = &m {
        assert_eq!(f.name, "baseX");
    }
    m.handle(&AppEvent::Down); // switch to URL
    m.handle(&AppEvent::Char('h'));
    if let Modal::EditGateway(f) = &m {
        assert_eq!(f.url, "h");
        assert_eq!(f.focus, AddField::Url);
    }
}
