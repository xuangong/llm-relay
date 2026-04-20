use super::{Backend, KEYSTORE_ENTRY, KEYSTORE_SERVICE};
use std::collections::HashMap;

pub struct SystemBackend;

impl SystemBackend {
    /// Probe by attempting a get/set on a sentinel entry. Any error means
    /// the OS keychain is not usable in this environment.
    pub fn probe() -> Result<Self, String> {
        let entry = keyring::Entry::new(KEYSTORE_SERVICE, "__probe__")
            .map_err(|e| format!("entry: {e}"))?;
        match entry.set_password("ok") {
            Ok(()) => {
                let _ = entry.delete_credential();
                Ok(Self)
            }
            Err(e) => Err(format!("write: {e}")),
        }
    }
}

impl Backend for SystemBackend {
    fn load(&self) -> HashMap<String, String> {
        match keyring::Entry::new(KEYSTORE_SERVICE, KEYSTORE_ENTRY) {
            Ok(entry) => match entry.get_password() {
                Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
                Err(_) => HashMap::new(),
            },
            Err(_) => HashMap::new(),
        }
    }

    fn save(&self, map: &HashMap<String, String>) {
        let json = serde_json::to_string(map).unwrap_or_else(|_| "{}".into());
        if let Ok(entry) = keyring::Entry::new(KEYSTORE_SERVICE, KEYSTORE_ENTRY) {
            if let Err(e) = entry.set_password(&json) {
                log::warn!("keystore save (system): {e}");
            }
        }
    }
}
