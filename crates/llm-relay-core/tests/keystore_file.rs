//! Tests the file backend in isolation (system backend requires a real keychain).

use llm_relay_core::keystore::{file_backend_for_test, Backend};
use std::collections::HashMap;

mod helper {
    pub fn unique_tmp(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("llm-relay-test-{}-{}", name, std::process::id()));
        p
    }
}

#[test]
fn round_trip_with_env_password() {
    std::env::set_var("LLM_RELAY_KEY", "test-pw-123");
    let path = helper::unique_tmp("rt").join("secrets.enc");
    let _ = std::fs::remove_file(&path);

    let be = file_backend_for_test(path.clone());
    let mut m = HashMap::new();
    m.insert("k1".to_string(), "v1".to_string());
    m.insert("k2".to_string(), "v2".to_string());
    be.save(&m);

    // Fresh backend reading the same file with same env password
    let be2 = file_backend_for_test(path);
    let loaded = be2.load();
    assert_eq!(loaded, m);
}

#[test]
fn wrong_password_yields_empty_map() {
    std::env::set_var("LLM_RELAY_KEY", "correct");
    let path = helper::unique_tmp("wp").join("secrets.enc");
    let _ = std::fs::remove_file(&path);

    let be = file_backend_for_test(path.clone());
    let mut m = HashMap::new();
    m.insert("k".to_string(), "v".to_string());
    be.save(&m);

    std::env::set_var("LLM_RELAY_KEY", "wrong");
    let be2 = file_backend_for_test(path);
    let loaded = be2.load();
    assert!(loaded.is_empty(), "wrong password should fail to decrypt");
}
