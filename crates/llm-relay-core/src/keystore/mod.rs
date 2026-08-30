//! Unified secrets store. Tries the OS keychain at first use; falls back
//! to an AES-256-GCM encrypted file when the keychain is unavailable
//! (e.g. Linux servers without DBus / Secret Service).

mod env_backend;
mod file_backend;
mod system_backend;

pub use env_backend::{
    generate_master_key, setup_hint as env_setup_hint, ENV_VAR as ENV_KEY_VAR,
    FILE_NAME as ENV_STORE_FILE,
};

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const SERVICE: &str = "llm-relay";
const ENTRY_KEY: &str = "secrets";

pub trait Backend: Send + Sync {
    fn load(&self) -> HashMap<String, String>;
    fn save(&self, map: &HashMap<String, String>);
}

static BACKEND: OnceLock<Box<dyn Backend>> = OnceLock::new();
static CACHE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

use crate::ipc::protocol::KeystoreKind;
static CURRENT_KIND: OnceLock<KeystoreKind> = OnceLock::new();

pub fn current_kind() -> KeystoreKind {
    *CURRENT_KIND.get().unwrap_or(&KeystoreKind::System)
}

/// Initialize the keystore. Call once at startup before any get/set.
/// Tries the OS keychain first by performing a real read+write probe;
/// on failure, falls back to encrypted file.
pub fn init(config_dir: &std::path::Path) {
    let (backend, kind): (Box<dyn Backend>, KeystoreKind) = match system_backend::SystemBackend::probe() {
        Ok(b) => (Box::new(b), KeystoreKind::System),
        Err(e) => {
            log::warn!("system keychain unavailable: {e}; using encrypted file at {}", config_dir.display());
            (Box::new(file_backend::FileBackend::new(config_dir.join("secrets.enc"))), KeystoreKind::EncryptedFile)
        }
    };
    let _ = CURRENT_KIND.set(kind);
    let _ = BACKEND.set(backend);
}

/// Why `init_env` refused to start the process.
#[derive(Debug)]
pub enum EnvInitError {
    /// No usable master key: env var missing, not base64, or wrong length.
    /// The operator needs the "generate one" instructions.
    MissingKey(String),
    /// The key is well-formed but doesn't open `secrets.env.enc`. Telling
    /// this operator to generate a key would be actively wrong — they have
    /// one, it just isn't the one that sealed this file.
    UnreadableStore(String),
    /// `init()` or `init_env()` already ran in this process.
    AlreadyInitialized,
}

impl std::fmt::Display for EnvInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingKey(m) | Self::UnreadableStore(m) => f.write_str(m),
            Self::AlreadyInitialized => f.write_str("keystore already initialized"),
        }
    }
}

impl std::error::Error for EnvInitError {}

/// Check the env master key against the store **without** installing it as
/// the process backend. Lets a caller decide what to do about a missing key
/// before committing to it — the TUI uses this to run its first-run wizard
/// instead of spawning an agent that would immediately `exit(2)`.
pub fn probe_env(config_dir: &std::path::Path) -> Result<(), EnvInitError> {
    let path = config_dir.join(env_backend::FILE_NAME);
    env_backend::EnvBackend::from_env(path)
        .map_err(EnvInitError::MissingKey)?
        .verify()
        .map_err(EnvInitError::UnreadableStore)
}

/// Initialize the keystore in env-only mode (headless agent / TUI server).
/// Reads the master key from `LLM_RELAY_MASTER_KEY` (base64 32 bytes) and
/// stores ciphertext at `<config_dir>/secrets.env.enc`. NEVER falls back to
/// the OS keychain or interactive file backend — server deployments must
/// supply the env var explicitly.
///
/// Fails fast when the key doesn't match an existing store, so a rotated or
/// mistyped key surfaces as a startup error instead of a silently empty
/// secret set that looks like every gateway went down at once.
pub fn init_env(config_dir: &std::path::Path) -> Result<(), EnvInitError> {
    probe_env(config_dir)?;
    let path = config_dir.join(env_backend::FILE_NAME);
    let backend = env_backend::EnvBackend::from_env(path).map_err(EnvInitError::MissingKey)?;
    if BACKEND.set(Box::new(backend)).is_err() {
        return Err(EnvInitError::AlreadyInitialized);
    }
    let _ = CURRENT_KIND.set(KeystoreKind::Env);
    Ok(())
}

fn backend() -> &'static dyn Backend {
    BACKEND
        .get()
        .expect("keystore::init() must be called before use")
        .as_ref()
}

fn load_all() -> HashMap<String, String> {
    let mut cache = CACHE.lock().unwrap();
    if let Some(ref map) = *cache {
        return map.clone();
    }
    let map = backend().load();
    *cache = Some(map.clone());
    map
}

fn save_all(map: &HashMap<String, String>) {
    *CACHE.lock().unwrap() = Some(map.clone());
    backend().save(map);
}

pub fn set_secret(key: &str, value: &str) {
    let mut map = load_all();
    if map.get(key).map(String::as_str) == Some(value) {
        return;
    }
    map.insert(key.to_string(), value.to_string());
    save_all(&map);
}

pub fn get_secret(key: &str) -> Option<String> {
    load_all().get(key).cloned()
}

pub fn delete_secret(key: &str) {
    let mut map = load_all();
    if map.remove(key).is_some() {
        save_all(&map);
    }
}

pub fn gw_auth_key(gateway_id: &str) -> String { format!("gw:{gateway_id}:auth_key") }
pub fn gw_session_token(gateway_id: &str) -> String { format!("gw:{gateway_id}:session_token") }
pub fn active_key_value() -> String { "active:key_value".to_string() }

pub fn migrate_legacy_entries(gateway_ids: &[String]) {
    let mut map = load_all();
    let mut changed = false;
    for id in gateway_ids {
        for key in [gw_auth_key(id), gw_session_token(id)] {
            if map.contains_key(&key) { continue; }
            if let Ok(entry) = keyring::Entry::new(SERVICE, &key) {
                if let Ok(val) = entry.get_password() {
                    map.insert(key.clone(), val);
                    let _ = entry.delete_credential();
                    changed = true;
                }
            }
        }
    }
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
    if changed { save_all(&map); }
}

pub(super) const KEYSTORE_SERVICE: &str = SERVICE;
pub(super) const KEYSTORE_ENTRY: &str = ENTRY_KEY;

/// Test-only constructor exposing the file backend without going through `init()`.
#[doc(hidden)]
pub fn file_backend_for_test(path: std::path::PathBuf) -> impl Backend {
    file_backend::FileBackend::new(path)
}

/// Test-only constructor exposing the env backend. Reads `LLM_RELAY_MASTER_KEY`
/// at call time; returns Err if missing/invalid.
#[doc(hidden)]
pub fn env_backend_for_test(path: std::path::PathBuf) -> Result<impl Backend, String> {
    env_backend::EnvBackend::from_env(path)
}

/// Test-only hook for `EnvBackend::verify` — the startup guard that makes a
/// wrong master key fatal instead of silently empty.
#[doc(hidden)]
pub fn env_verify_for_test(path: std::path::PathBuf) -> Result<(), String> {
    env_backend::EnvBackend::from_env(path)?.verify()
}
