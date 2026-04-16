use std::collections::HashMap;
use std::sync::Mutex;

const SERVICE: &str = "llm-relay";
const ENTRY_KEY: &str = "secrets";

/// All secrets are stored in a single keychain entry as JSON.
/// This means macOS only prompts for keychain access once.
static CACHE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn load_all() -> HashMap<String, String> {
    let mut cache = CACHE.lock().unwrap();
    if let Some(ref map) = *cache {
        return map.clone();
    }
    let map = match keyring::Entry::new(SERVICE, ENTRY_KEY) {
        Ok(entry) => match entry.get_password() {
            Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
            Err(_) => HashMap::new(),
        },
        Err(_) => HashMap::new(),
    };
    *cache = Some(map.clone());
    map
}

fn save_all(map: &HashMap<String, String>) {
    *CACHE.lock().unwrap() = Some(map.clone());
    let json = serde_json::to_string(map).unwrap_or_else(|_| "{}".to_string());
    if let Ok(entry) = keyring::Entry::new(SERVICE, ENTRY_KEY) {
        if let Err(e) = entry.set_password(&json) {
            log::warn!("keystore save: {e}");
        }
    }
}

/// Store a secret in the OS keychain.
pub fn set_secret(key: &str, value: &str) {
    let mut map = load_all();
    map.insert(key.to_string(), value.to_string());
    save_all(&map);
}

/// Retrieve a secret from the OS keychain.
pub fn get_secret(key: &str) -> Option<String> {
    let map = load_all();
    map.get(key).cloned()
}

/// Delete a secret from the OS keychain.
pub fn delete_secret(key: &str) {
    let mut map = load_all();
    if map.remove(key).is_some() {
        save_all(&map);
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

/// Migrate old per-key entries into the unified entry.
/// Call once at startup. Silently skips if nothing to migrate.
pub fn migrate_legacy_entries(gateway_ids: &[String]) {
    let mut map = load_all();
    let mut changed = false;

    for id in gateway_ids {
        let ak = gw_auth_key(id);
        let st = gw_session_token(id);
        for key in [&ak, &st] {
            if map.contains_key(key.as_str()) {
                continue;
            }
            // Try reading from old per-key entry
            if let Ok(entry) = keyring::Entry::new(SERVICE, key) {
                if let Ok(val) = entry.get_password() {
                    map.insert(key.clone(), val);
                    let _ = entry.delete_credential();
                    changed = true;
                }
            }
        }
    }

    // active key_value
    let akv = active_key_value();
    if !map.contains_key(&akv) {
        if let Ok(entry) = keyring::Entry::new(SERVICE, &akv) {
            if let Ok(val) = entry.get_password() {
                map.insert(akv, val);
                let _ = entry.delete_credential();
                changed = true;
            }
        }
    }

    if changed {
        save_all(&map);
    }
}
