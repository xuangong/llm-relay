//! Env-keyed encrypted secrets backend for headless deployments (TUI / agent
//! on a server). The master key comes from the `LLM_RELAY_MASTER_KEY`
//! environment variable (base64-encoded 32 bytes). No keychain, no prompting,
//! no Argon2 — operators are responsible for storing the key in their
//! secret-management tool of choice (systemd `EnvironmentFile=`, docker secret,
//! Kubernetes Secret, etc.).
//!
//! Secrets at rest live in `<config_dir>/secrets.env.enc` (separate from the
//! interactive `secrets.enc` used by FileBackend).

use super::Backend;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine;
use rand::RngCore;
use std::collections::HashMap;
use std::path::PathBuf;

const MAGIC: &[u8; 9] = b"LLMRELAYE";
const NONCE_LEN: usize = 12;
pub const ENV_VAR: &str = "LLM_RELAY_MASTER_KEY";
pub const FILE_NAME: &str = "secrets.env.enc";

pub struct EnvBackend {
    path: PathBuf,
    key: [u8; 32],
}

impl EnvBackend {
    /// Build a backend by reading and validating the master key from env.
    /// Returns Err if the env var is missing or not a base64-encoded 32-byte value.
    pub fn from_env(path: PathBuf) -> Result<Self, String> {
        let raw = std::env::var(ENV_VAR)
            .map_err(|_| format!("{ENV_VAR} not set"))?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(raw.trim())
            .map_err(|e| format!("{ENV_VAR} is not valid base64: {e}"))?;
        if decoded.len() != 32 {
            return Err(format!(
                "{ENV_VAR} must decode to exactly 32 bytes, got {}",
                decoded.len()
            ));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&decoded);
        Ok(Self { path, key })
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.key))
    }

    fn read_file(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        let bytes = std::fs::read(&self.path).ok()?;
        if bytes.len() < MAGIC.len() + NONCE_LEN || &bytes[..MAGIC.len()] != MAGIC {
            log::warn!("env keystore at {} has bad magic", self.path.display());
            return None;
        }
        let mut o = MAGIC.len();
        let nonce = bytes[o..o + NONCE_LEN].to_vec();
        o += NONCE_LEN;
        let ct = bytes[o..].to_vec();
        Some((nonce, ct))
    }

    fn write_file(&self, nonce: &[u8], ct: &[u8]) -> std::io::Result<()> {
        let mut buf = Vec::with_capacity(MAGIC.len() + nonce.len() + ct.len());
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(nonce);
        buf.extend_from_slice(ct);
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, buf)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

impl Backend for EnvBackend {
    fn load(&self) -> HashMap<String, String> {
        let Some((nonce, ct)) = self.read_file() else {
            return HashMap::new();
        };
        match self.cipher().decrypt(Nonce::from_slice(&nonce), ct.as_ref()) {
            Ok(plain) => serde_json::from_slice(&plain).unwrap_or_default(),
            Err(e) => {
                log::error!(
                    "env keystore decrypt failed: {e} (wrong {ENV_VAR}? regenerate to start fresh)"
                );
                HashMap::new()
            }
        }
    }

    fn save(&self, map: &HashMap<String, String>) {
        let mut nonce = vec![0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce);
        let plain = serde_json::to_vec(map).unwrap_or_else(|_| b"{}".to_vec());
        match self.cipher().encrypt(Nonce::from_slice(&nonce), plain.as_ref()) {
            Ok(ct) => {
                if let Err(e) = self.write_file(&nonce, &ct) {
                    log::error!("env keystore write {} failed: {e}", self.path.display());
                }
            }
            Err(e) => log::error!("env keystore encrypt failed: {e}"),
        }
    }
}

/// Helper for the agent to print operator-facing setup instructions when
/// `LLM_RELAY_MASTER_KEY` is missing or invalid.
pub fn setup_hint() -> String {
    format!(
        "{ENV_VAR} is required for the headless agent.\n\
         Generate a key once and store it in your secret manager:\n\n\
         \topenssl rand -base64 32\n\n\
         Then export it before starting the agent:\n\n\
         \texport {ENV_VAR}=<base64-32-bytes>\n"
    )
}
