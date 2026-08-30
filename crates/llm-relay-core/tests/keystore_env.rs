//! Tests the env-keyed keystore backend used by the headless agent.

use llm_relay_core::keystore::{
    env_backend_for_test, env_verify_for_test, generate_master_key, probe_env, Backend,
    EnvInitError, ENV_KEY_VAR, ENV_STORE_FILE,
};
use base64::Engine;
use std::collections::HashMap;
use std::sync::Mutex;

fn unique_tmp(name: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("llm-relay-env-test-{}-{}", name, std::process::id()));
    p
}

fn random_key_b64() -> String {
    use rand::RngCore;
    let mut k = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut k);
    base64::engine::general_purpose::STANDARD.encode(k)
}

// Tests share process-wide env state — serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn round_trip_with_env_master_key() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ENV_KEY_VAR, random_key_b64());
    let path = unique_tmp("rt").join("secrets.env.enc");
    let _ = std::fs::remove_file(&path);

    let be = env_backend_for_test(path.clone()).expect("env backend ok");
    let mut m = HashMap::new();
    m.insert("k1".into(), "v1".into());
    m.insert("gw:abc:auth_key".into(), "tok-123".into());
    be.save(&m);

    let be2 = env_backend_for_test(path).expect("env backend ok 2");
    let loaded = be2.load();
    assert_eq!(loaded, m);
}

#[test]
fn wrong_master_key_yields_empty_map() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ENV_KEY_VAR, random_key_b64());
    let path = unique_tmp("wk").join("secrets.env.enc");
    let _ = std::fs::remove_file(&path);

    let be = env_backend_for_test(path.clone()).expect("ok");
    let mut m = HashMap::new();
    m.insert("k".into(), "v".into());
    be.save(&m);

    // Rotate the key — old ciphertext is unreadable. `load()` has no error
    // channel so it degrades to empty; `verify()` (below) is what stops the
    // agent from ever reaching this state.
    std::env::set_var(ENV_KEY_VAR, random_key_b64());
    let be2 = env_backend_for_test(path).expect("ok2");
    assert!(be2.load().is_empty(), "wrong key should fail to decrypt");
}

#[test]
fn verify_accepts_a_missing_store() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ENV_KEY_VAR, random_key_b64());
    let path = unique_tmp("v-fresh").join("secrets.env.enc");
    let _ = std::fs::remove_file(&path);

    // First run on a clean server: nothing sealed yet, nothing to check.
    env_verify_for_test(path).expect("missing store is a legitimate first run");
}

#[test]
fn verify_accepts_the_key_that_sealed_the_store() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ENV_KEY_VAR, random_key_b64());
    let path = unique_tmp("v-ok").join("secrets.env.enc");
    let _ = std::fs::remove_file(&path);

    let be = env_backend_for_test(path.clone()).expect("ok");
    let mut m = HashMap::new();
    m.insert("gw:abc:auth_key".into(), "tok-123".into());
    be.save(&m);

    env_verify_for_test(path).expect("same key must verify");
}

#[test]
fn verify_rejects_a_rotated_key() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ENV_KEY_VAR, random_key_b64());
    let path = unique_tmp("v-rot").join("secrets.env.enc");
    let _ = std::fs::remove_file(&path);

    let be = env_backend_for_test(path.clone()).expect("ok");
    let mut m = HashMap::new();
    m.insert("gw:abc:auth_key".into(), "tok-123".into());
    be.save(&m);

    std::env::set_var(ENV_KEY_VAR, random_key_b64());
    let err = env_verify_for_test(path).expect_err("rotated key must be fatal");
    assert!(err.contains(ENV_KEY_VAR), "{err}");
}

#[test]
fn verify_rejects_a_file_that_is_not_ours() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ENV_KEY_VAR, random_key_b64());
    let path = unique_tmp("v-bad").join("secrets.env.enc");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Long enough to pass the length check, wrong magic. Overwriting this
    // would destroy whatever it actually is.
    std::fs::write(&path, b"NOTOURFILE-padding-padding-padding").unwrap();

    let err = env_verify_for_test(path).expect_err("foreign file must be fatal");
    assert!(err.contains("bad header"), "{err}");
}

/// The wizard's contract: whatever `generate_master_key` hands the operator
/// has to be accepted verbatim as `LLM_RELAY_MASTER_KEY`.
#[test]
fn a_generated_key_is_usable_as_a_master_key() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ENV_KEY_VAR, generate_master_key());
    let path = unique_tmp("gen").join(ENV_STORE_FILE);
    let _ = std::fs::remove_file(&path);

    let be = env_backend_for_test(path.clone()).expect("generated key must parse");
    let mut m = HashMap::new();
    m.insert("gw:abc:auth_key".into(), "tok-123".into());
    be.save(&m);

    env_verify_for_test(path).expect("generated key must open its own store");
}

#[test]
fn probe_env_reports_a_missing_key() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var(ENV_KEY_VAR);
    let dir = unique_tmp("probe-none");
    let _ = std::fs::remove_file(dir.join(ENV_STORE_FILE));

    // The TUI branches on this variant to decide whether to offer the wizard,
    // so the distinction from UnreadableStore has to survive.
    match probe_env(&dir) {
        Err(EnvInitError::MissingKey(_)) => {}
        other => panic!("expected MissingKey, got {other:?}"),
    }
}

#[test]
fn probe_env_accepts_a_fresh_dir_with_a_valid_key() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ENV_KEY_VAR, random_key_b64());
    let dir = unique_tmp("probe-fresh");
    let _ = std::fs::remove_file(dir.join(ENV_STORE_FILE));

    probe_env(&dir).expect("nothing sealed yet — any valid key is fine");
}

#[test]
fn probe_env_reports_a_store_the_key_cannot_open() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ENV_KEY_VAR, random_key_b64());
    let dir = unique_tmp("probe-rot");
    let path = dir.join(ENV_STORE_FILE);
    let _ = std::fs::remove_file(&path);

    let be = env_backend_for_test(path).expect("ok");
    let mut m = HashMap::new();
    m.insert("k".into(), "v".into());
    be.save(&m);

    std::env::set_var(ENV_KEY_VAR, random_key_b64());
    match probe_env(&dir) {
        Err(EnvInitError::UnreadableStore(_)) => {}
        other => panic!("expected UnreadableStore, got {other:?}"),
    }
}

fn expect_err(r: Result<impl Backend, String>) -> String {
    match r {
        Ok(_) => panic!("expected Err"),
        Err(e) => e,
    }
}

#[test]
fn missing_env_var_returns_error() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var(ENV_KEY_VAR);
    let path = unique_tmp("missing").join("secrets.env.enc");
    let err = expect_err(env_backend_for_test(path));
    assert!(err.contains(ENV_KEY_VAR), "{err}");
}

#[test]
fn invalid_base64_returns_error() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ENV_KEY_VAR, "not-base64!!!");
    let path = unique_tmp("badb64").join("secrets.env.enc");
    let err = expect_err(env_backend_for_test(path));
    assert!(err.contains("base64"), "{err}");
}

#[test]
fn wrong_length_returns_error() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // base64 of 16 bytes — decodes fine but wrong length.
    std::env::set_var(
        ENV_KEY_VAR,
        base64::engine::general_purpose::STANDARD.encode([0u8; 16]),
    );
    let path = unique_tmp("badlen").join("secrets.env.enc");
    let err = expect_err(env_backend_for_test(path));
    assert!(err.contains("32 bytes"), "{err}");
}
