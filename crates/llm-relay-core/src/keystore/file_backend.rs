use super::Backend;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::{Argon2, Algorithm, Version, Params};
use rand::RngCore;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

const MAGIC: &[u8; 9] = b"LLMRELAY1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

pub struct FileBackend {
    path: PathBuf,
    master_key: Mutex<Option<[u8; 32]>>,
}

impl FileBackend {
    pub fn new(path: PathBuf) -> Self {
        Self { path, master_key: Mutex::new(None) }
    }

    fn obtain_master_password(&self) -> Result<String, String> {
        if let Ok(p) = std::env::var("LLM_RELAY_KEY") {
            return Ok(p);
        }
        rpassword::prompt_password("LLM Relay master password: ")
            .map_err(|e| format!("password prompt failed: {e}"))
    }

    fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; 32], String> {
        let params = Params::new(64 * 1024, 3, 1, Some(32))
            .map_err(|e| format!("argon2 params: {e}"))?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut out = [0u8; 32];
        argon2
            .hash_password_into(password.as_bytes(), salt, &mut out)
            .map_err(|e| format!("argon2 derive: {e}"))?;
        Ok(out)
    }

    fn cipher_for_key(key: &[u8; 32]) -> Aes256Gcm {
        Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
    }

    fn read_file(&self) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        // Returns (salt, nonce, ciphertext)
        let bytes = std::fs::read(&self.path).ok()?;
        if bytes.len() < MAGIC.len() + SALT_LEN + NONCE_LEN || &bytes[..MAGIC.len()] != MAGIC {
            log::warn!("keystore file at {} has bad magic", self.path.display());
            return None;
        }
        let mut o = MAGIC.len();
        let salt = bytes[o..o + SALT_LEN].to_vec(); o += SALT_LEN;
        let nonce = bytes[o..o + NONCE_LEN].to_vec(); o += NONCE_LEN;
        let ct = bytes[o..].to_vec();
        Some((salt, nonce, ct))
    }

    fn write_file(&self, salt: &[u8], nonce: &[u8], ct: &[u8]) -> std::io::Result<()> {
        let mut buf = Vec::with_capacity(MAGIC.len() + salt.len() + nonce.len() + ct.len());
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(salt);
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

impl Backend for FileBackend {
    fn load(&self) -> HashMap<String, String> {
        let Some((salt, nonce, ct)) = self.read_file() else {
            return HashMap::new();
        };
        let mut guard = self.master_key.lock().unwrap();
        let key = match *guard {
            Some(k) => k,
            None => {
                let pw = match self.obtain_master_password() {
                    Ok(p) => p,
                    Err(e) => { log::error!("{e}"); return HashMap::new(); }
                };
                match Self::derive_key(&pw, &salt) {
                    Ok(k) => { *guard = Some(k); k },
                    Err(e) => { log::error!("{e}"); return HashMap::new(); }
                }
            }
        };
        let cipher = Self::cipher_for_key(&key);
        match cipher.decrypt(Nonce::from_slice(&nonce), ct.as_ref()) {
            Ok(plain) => serde_json::from_slice(&plain).unwrap_or_default(),
            Err(e) => {
                log::error!("keystore decrypt failed: {e} (wrong password?)");
                *guard = None; // force re-prompt next time
                HashMap::new()
            }
        }
    }

    fn save(&self, map: &HashMap<String, String>) {
        let mut guard = self.master_key.lock().unwrap();
        let (salt, key) = match *guard {
            Some(k) => {
                // Reuse existing salt if file exists, else fresh
                let salt = self.read_file().map(|(s, _, _)| s).unwrap_or_else(|| {
                    let mut s = vec![0u8; SALT_LEN]; rand::thread_rng().fill_bytes(&mut s); s
                });
                (salt, k)
            }
            None => {
                let pw = match self.obtain_master_password() {
                    Ok(p) => p,
                    Err(e) => { log::error!("{e}"); return; }
                };
                let mut salt = vec![0u8; SALT_LEN]; rand::thread_rng().fill_bytes(&mut salt);
                match Self::derive_key(&pw, &salt) {
                    Ok(k) => { *guard = Some(k); (salt, k) }
                    Err(e) => { log::error!("{e}"); return; }
                }
            }
        };
        let mut nonce = vec![0u8; NONCE_LEN]; rand::thread_rng().fill_bytes(&mut nonce);
        let cipher = Self::cipher_for_key(&key);
        let plain = serde_json::to_vec(map).unwrap_or_else(|_| b"{}".to_vec());
        match cipher.encrypt(Nonce::from_slice(&nonce), plain.as_ref()) {
            Ok(ct) => {
                if let Err(e) = self.write_file(&salt, &nonce, &ct) {
                    log::error!("keystore write {} failed: {e}", self.path.display());
                }
            }
            Err(e) => log::error!("keystore encrypt failed: {e}"),
        }
    }
}
