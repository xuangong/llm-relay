//! Tests the env-keyed keystore backend used by the headless agent.

use llm_relay_core::keystore::{env_backend_for_test, Backend, ENV_KEY_VAR};
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

    // Rotate the key — old ciphertext is unreadable.
    std::env::set_var(ENV_KEY_VAR, random_key_b64());
    let be2 = env_backend_for_test(path).expect("ok2");
    assert!(be2.load().is_empty(), "wrong key should fail to decrypt");
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
