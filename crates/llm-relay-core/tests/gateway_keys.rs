//! `GET /api/keys` answers differently depending on which credential asks:
//! an admin key sees every key, a user session sees only what that user owns
//! or is assigned, and a plain API key sees itself. A session belonging to a
//! user who owns nothing therefore returns `200 []` — a success that answers
//! nothing. These tests pin the fallback that turns that into a real answer.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use llm_relay_core::database::{ActiveConfig, Gateway};
use llm_relay_core::gateway::fetch_keys_with_fallback;
use llm_relay_core::service::pick_key_id;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ── a stand-in gateway ───────────────────────────────────────────────────────

/// What `/api/keys` should do for a given bearer token: return a list, or fail
/// with a status. Anything not in the map is answered 401.
type Script = HashMap<String, Result<Vec<serde_json::Value>, u16>>;

#[derive(Clone)]
struct Fake {
    script: Arc<Script>,
    /// Every token that reached the server, in order — lets a test assert that
    /// a second credential was never tried.
    seen: Arc<Mutex<Vec<String>>>,
}

fn key(id: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "key": format!("sk-test-{id}"),
        "createdAt": null,
        "lastUsedAt": null,
        "ownerId": null,
        "ownerName": null,
    })
}

async fn keys_handler(State(f): State<Fake>, headers: HeaderMap) -> Response {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
        .to_string();
    f.seen.lock().unwrap().push(token.clone());

    match f.script.get(&token) {
        Some(Ok(list)) => Json(list.clone()).into_response(),
        Some(Err(code)) => (
            StatusCode::from_u16(*code).unwrap(),
            "gateway said no".to_string(),
        )
            .into_response(),
        None => (StatusCode::UNAUTHORIZED, "unknown token".to_string()).into_response(),
    }
}

async fn serve(script: Script) -> (String, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let fake = Fake {
        script: Arc::new(script),
        seen: Arc::clone(&seen),
    };
    let app = Router::new()
        .route("/api/keys", get(keys_handler))
        .with_state(fake);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), seen)
}

// ── fetch_keys_with_fallback ─────────────────────────────────────────────────

/// The bug this whole helper exists for: the selected key is invisible to the
/// session but visible to the gateway's own key. Before the fallback, the
/// caller saw an empty list, wrote a NULL key_value, and the proxy quietly
/// forwarded under a different credential than the one that was picked.
#[tokio::test]
async fn falls_back_when_the_session_cannot_see_the_wanted_key() {
    let (url, seen) = serve(HashMap::from([
        ("sess".into(), Ok(vec![])),
        (
            "admin".into(),
            Ok(vec![key("k1", "xian"), key("k2", "liang")]),
        ),
    ]))
    .await;

    let keys = fetch_keys_with_fallback(&url, Some("sess"), "admin", Some("k2"))
        .await
        .expect("the admin key can see k2");

    assert_eq!(keys.len(), 2);
    assert!(keys.iter().any(|k| k.id == "k2"));
    assert_eq!(*seen.lock().unwrap(), vec!["sess", "admin"]);
}

/// The session stays first: it is usually the broader view (an admin session
/// sees other users' keys, that user's own API key does not), so a session
/// that answers must not be second-guessed.
#[tokio::test]
async fn stops_at_the_session_when_it_answers() {
    let (url, seen) = serve(HashMap::from([
        (
            "sess".into(),
            Ok(vec![key("k1", "xian"), key("k2", "liang")]),
        ),
        ("apikey".into(), Ok(vec![key("k2", "liang")])),
    ]))
    .await;

    let keys = fetch_keys_with_fallback(&url, Some("sess"), "apikey", Some("k2"))
        .await
        .expect("session answers");

    assert_eq!(keys.len(), 2, "the broader list should win");
    assert_eq!(
        *seen.lock().unwrap(),
        vec!["sess"],
        "the fallback should not have been tried"
    );
}

/// Feeding the key picker: nothing specific is wanted, so any non-empty list
/// is an answer and an empty one is not.
#[tokio::test]
async fn without_a_wanted_key_an_empty_list_is_not_an_answer() {
    let (url, _) = serve(HashMap::from([
        ("sess".into(), Ok(vec![])),
        ("apikey".into(), Ok(vec![key("k9", "solo")])),
    ]))
    .await;

    let keys = fetch_keys_with_fallback(&url, Some("sess"), "apikey", None)
        .await
        .unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].id, "k9");
}

/// Logging in with a plain API key stores it as both the session token and the
/// auth key. Asking twice would double every picker's latency for nothing.
#[tokio::test]
async fn identical_credentials_are_only_tried_once() {
    let (url, seen) = serve(HashMap::from([("same".into(), Ok(vec![]))])).await;

    let keys = fetch_keys_with_fallback(&url, Some("same"), "same", Some("k1"))
        .await
        .expect("an empty list is still a successful response");

    assert!(keys.is_empty());
    assert_eq!(*seen.lock().unwrap(), vec!["same"]);
}

/// An expired session is the common real case: 401 from the session, a good
/// list from the gateway key. Erroring out here would break every picker the
/// moment a session aged out.
#[tokio::test]
async fn a_failing_session_falls_through_to_the_auth_key() {
    let (url, _) = serve(HashMap::from([
        ("stale".into(), Err(401)),
        ("apikey".into(), Ok(vec![key("k1", "xian")])),
    ]))
    .await;

    let keys = fetch_keys_with_fallback(&url, Some("stale"), "apikey", Some("k1"))
        .await
        .unwrap();
    assert_eq!(keys[0].id, "k1");
}

/// Nobody can see the key. The caller still gets the list it can show — this
/// is not the layer that decides whether an empty picker is fatal; `set_active`
/// is, and it errors.
#[tokio::test]
async fn an_unsatisfying_success_beats_an_error() {
    let (url, _) = serve(HashMap::from([
        ("sess".into(), Ok(vec![key("other", "someone else")])),
        ("apikey".into(), Err(500)),
    ]))
    .await;

    let keys = fetch_keys_with_fallback(&url, Some("sess"), "apikey", Some("missing"))
        .await
        .expect("a partial answer is better than no answer");
    assert_eq!(keys[0].id, "other");
}

#[tokio::test]
async fn every_credential_failing_is_an_error() {
    let (url, _) = serve(HashMap::from([
        ("sess".into(), Err(401)),
        ("apikey".into(), Err(500)),
    ]))
    .await;

    let err = fetch_keys_with_fallback(&url, Some("sess"), "apikey", None)
        .await
        .expect_err("nothing worked");
    assert!(err.to_string().contains("500"), "{err}");
}

#[tokio::test]
async fn no_credentials_at_all_is_an_error_without_a_request() {
    let (url, seen) = serve(HashMap::new()).await;

    let err = fetch_keys_with_fallback(&url, Some(""), "", None)
        .await
        .expect_err("nothing to authenticate with");
    assert!(err.to_string().contains("log in"), "{err}");
    assert!(seen.lock().unwrap().is_empty());
}

// ── pick_key_id ──────────────────────────────────────────────────────────────

fn gateway(id: &str, preferred: Option<&str>) -> Gateway {
    Gateway {
        id: id.into(),
        name: format!("gw-{id}"),
        url: "http://example.invalid".into(),
        auth_key: "k".into(),
        is_admin: false,
        session_token: None,
        user_id: None,
        user_name: None,
        sort_order: 0,
        created_at: "2026-01-01T00:00:00Z".into(),
        claude_model: None,
        claude_subagent_model: None,
        claude_small_model: None,
        codex_model: None,
        codex_subagent_model: None,
        gemini_model: None,
        preferred_key_id: preferred.map(str::to_string),
        claude_extra_config_id: None,
    }
}

fn active(gateway_id: &str, key_id: &str) -> ActiveConfig {
    ActiveConfig {
        gateway_id: Some(gateway_id.into()),
        key_id: Some(key_id.into()),
        key_name: None,
        key_value: None,
        claude_model: None,
        claude_subagent_model: None,
        claude_small_model: None,
        codex_model: None,
        codex_subagent_model: None,
        gemini_model: None,
        claude_extra_config_id: None,
        auto_switch: false,
        applied_at: None,
        last_switched_at: None,
    }
}

#[test]
fn the_gateways_own_key_wins_over_whatever_is_active() {
    let gw = gateway("A", Some("key-A"));
    assert_eq!(
        pick_key_id(&gw, Some(&active("B", "key-B"))),
        Some("key-A".into())
    );
}

/// The tray-switch bug: clicking gateway B while A is active used to hand B
/// the key id A issued, which B has never heard of.
#[test]
fn another_gateways_active_key_is_not_borrowed() {
    let gw = gateway("B", None);
    assert_eq!(pick_key_id(&gw, Some(&active("A", "key-A"))), None);
}

#[test]
fn re_applying_the_same_gateway_reuses_its_active_key() {
    let gw = gateway("A", None);
    assert_eq!(
        pick_key_id(&gw, Some(&active("A", "key-A"))),
        Some("key-A".into())
    );
}

#[test]
fn a_never_configured_gateway_has_no_key() {
    assert_eq!(pick_key_id(&gateway("A", None), None), None);
}
