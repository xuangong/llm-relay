use llm_relay_agent::login::{LoginRegistry, LoginOutcome};
use llm_relay_core::ipc::Event;
use std::time::Duration;
use tokio::sync::broadcast;
use uuid::Uuid;

#[tokio::test]
async fn start_login_then_cancel_emits_no_completion() {
    let (tx, mut rx) = broadcast::channel(16);
    let registry = LoginRegistry::new(tx);

    let gid = Uuid::new_v4();
    // Start a login session manually with a fake poller that never resolves.
    let handle = registry
        .start_with_poller(gid, async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            LoginOutcome::Expired
        })
        .await;
    assert!(handle.is_some(), "first start should succeed");

    // Cancel before completion
    assert!(registry.cancel(gid).await);

    // No event should arrive within a short window.
    let result = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
    assert!(result.is_err(), "no event expected after cancel");
}

#[tokio::test]
async fn start_login_twice_for_same_gateway_returns_existing() {
    let (tx, _rx) = broadcast::channel(16);
    let registry = LoginRegistry::new(tx);

    let gid = Uuid::new_v4();
    let h1 = registry
        .start_with_poller(gid, async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            LoginOutcome::Expired
        })
        .await;
    let h2 = registry
        .start_with_poller(gid, async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            LoginOutcome::Expired
        })
        .await;
    assert!(h1.is_some());
    assert!(h2.is_none(), "second start for same gateway must be rejected");
}

#[tokio::test]
async fn poller_completion_emits_login_completed_event() {
    let (tx, mut rx) = broadcast::channel(16);
    let registry = LoginRegistry::new(tx);

    let gid = Uuid::new_v4();
    registry
        .start_with_poller(gid, async move {
            LoginOutcome::Completed {
                session_token: "tok".into(),
                user_id: Some("u1".into()),
                user_name: Some("alice".into()),
            }
        })
        .await
        .unwrap();

    let evt = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await
        .expect("event arrived")
        .expect("recv ok");
    match evt {
        Event::LoginCompleted { gateway_id, session_token, user_name, .. } => {
            assert_eq!(gateway_id, gid);
            assert_eq!(session_token, "tok");
            assert_eq!(user_name.as_deref(), Some("alice"));
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[tokio::test]
async fn poller_failure_emits_login_failed_event() {
    let (tx, mut rx) = broadcast::channel(16);
    let registry = LoginRegistry::new(tx);

    let gid = Uuid::new_v4();
    registry
        .start_with_poller(gid, async move {
            LoginOutcome::Failed("access_denied".into())
        })
        .await
        .unwrap();

    let evt = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await
        .expect("event arrived")
        .expect("recv ok");
    match evt {
        Event::LoginFailed { gateway_id, message } => {
            assert_eq!(gateway_id, gid);
            assert_eq!(message, "access_denied");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}
