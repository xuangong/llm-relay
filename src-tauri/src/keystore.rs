const SERVICE: &str = "llm-relay";

/// Store a secret in the OS keychain.
pub fn set_secret(key: &str, value: &str) {
    if let Ok(entry) = keyring::Entry::new(SERVICE, key) {
        if let Err(e) = entry.set_password(value) {
            log::warn!("keystore set_secret({key}): {e}");
        }
    }
}

/// Retrieve a secret from the OS keychain.
pub fn get_secret(key: &str) -> Option<String> {
    let entry = keyring::Entry::new(SERVICE, key).ok()?;
    entry.get_password().ok()
}

/// Delete a secret from the OS keychain.
pub fn delete_secret(key: &str) {
    if let Ok(entry) = keyring::Entry::new(SERVICE, key) {
        let _ = entry.delete_credential();
    }
}

// Key naming helpers

pub fn gw_auth_key(gateway_id: &str) -> String {
    format!("gw:{gateway_id}:auth_key")
}

pub fn gw_session_token(gateway_id: &str) -> String {
    format!("gw:{gateway_id}:session_token")
}

pub fn active_key_value() -> String {
    "active:key_value".to_string()
}
