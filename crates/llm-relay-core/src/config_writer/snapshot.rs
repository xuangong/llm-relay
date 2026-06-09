//! Per-target CLI config snapshots.
//!
//! One JSON file per target under `paths::cli_config_backup_dir()`.
//! Filenames are opaque sha256-derived ids — original distro name,
//! target_type, and probed home all live inside the JSON, so restore
//! never depends on filename parsing (and distro names with spaces
//! don't collide).

use super::{ClaudeSnapshot, CodexSnapshot, GeminiSnapshot, parse_env_file, serialize_env_file};
use crate::cli_target::{CliBackend, CliTarget, SnapshotMeta, TargetType};
use crate::AppError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use toml_edit::DocumentMut;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSnapshot {
    /// "windows" | "wsl". Required.
    pub target_type: String,
    /// Original `wsl -d <name>`. None for Windows targets.
    #[serde(default)]
    pub distro_name: Option<String>,
    /// $HOME captured at probe time (WSL targets). Lets restore rebuild
    /// the WslBackend without re-probing a maybe-stopped distro.
    #[serde(default)]
    pub home: Option<String>,
    pub captured_at: String,
    #[serde(default)]
    pub claude: ClaudeSnapshot,
    #[serde(default)]
    pub codex: CodexSnapshot,
    #[serde(default)]
    pub gemini: GeminiSnapshot,
}

/// Stable opaque id derived from the distro name (or the literal "windows").
/// 16 chars of lowercase base32(sha256(...)) — collision-safe enough for
/// the at-most-handful of distros a user manages.
pub fn target_file_name(meta: &SnapshotMeta) -> String {
    match meta.target_type {
        TargetType::Windows => "windows.json".to_string(),
        TargetType::Wsl => {
            use sha2::{Digest, Sha256};
            let name = meta.distro_name.as_deref().unwrap_or("");
            let mut h = Sha256::new();
            h.update(name.as_bytes());
            let digest = h.finalize();
            let b32 = data_encoding::BASE32_NOPAD.encode(&digest);
            let id: String = b32.chars().take(16).collect::<String>().to_lowercase();
            format!("wsl-{id}.json")
        }
    }
}

pub fn snapshot_path(meta: &SnapshotMeta) -> PathBuf {
    crate::paths::cli_config_backup_dir().join(target_file_name(meta))
}

/// Capture the live state of the three CLIs as visible to `target.backend`
/// and atomically persist as a TargetSnapshot JSON.
pub fn capture(target: &CliTarget) -> Result<(), AppError> {
    let snap = TargetSnapshot {
        target_type: match target.snapshot_meta.target_type {
            TargetType::Windows => "windows".into(),
            TargetType::Wsl => "wsl".into(),
        },
        distro_name: target.snapshot_meta.distro_name.clone(),
        home: target.snapshot_meta.home.clone(),
        captured_at: chrono::Utc::now().to_rfc3339(),
        claude: capture_claude(&*target.backend)?,
        codex: capture_codex(&*target.backend)?,
        gemini: capture_gemini(&*target.backend)?,
    };
    let path = snapshot_path(&target.snapshot_meta);
    std::fs::create_dir_all(crate::paths::cli_config_backup_dir())?;
    let bytes = serde_json::to_vec_pretty(&snap)?;
    crate::cli_target::atomic_write(&path, &bytes)
}

pub fn read(meta: &SnapshotMeta) -> Result<Option<TargetSnapshot>, AppError> {
    let path = snapshot_path(meta);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path)?;
    let snap: TargetSnapshot = serde_json::from_slice(&bytes)?;
    Ok(Some(snap))
}

pub fn delete(meta: &SnapshotMeta) -> Result<(), AppError> {
    let path = snapshot_path(meta);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Restore one target from its snapshot via the supplied backend.
pub fn restore(snap: &TargetSnapshot, backend: &dyn CliBackend) -> Result<(), AppError> {
    restore_claude_backend(&snap.claude, backend)?;
    restore_codex_backend(&snap.codex, backend)?;
    restore_gemini_backend(&snap.gemini, backend)?;
    Ok(())
}

/// Scan the backup directory; return `distro_name → SnapshotMeta` index.
/// Windows snapshots are keyed by the literal "windows".
pub fn build_index() -> Result<std::collections::HashMap<String, SnapshotMeta>, AppError> {
    let dir = crate::paths::cli_config_backup_dir();
    let mut map = std::collections::HashMap::new();
    if !dir.exists() {
        return Ok(map);
    }
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("snapshot read failed {}: {e}", path.display());
                continue;
            }
        };
        let snap: TargetSnapshot = match serde_json::from_slice(&bytes) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("ignoring malformed snapshot {}: {e}", path.display());
                continue;
            }
        };
        let meta = SnapshotMeta {
            target_type: if snap.target_type == "wsl" { TargetType::Wsl } else { TargetType::Windows },
            distro_name: snap.distro_name.clone(),
            home: snap.home.clone(),
        };
        let key = snap.distro_name.unwrap_or_else(|| "windows".to_string());
        map.insert(key, meta);
    }
    Ok(map)
}

/// One-shot migration of pre-WSL2 single-file snapshot to per-target
/// directory layout. Old format has no `target_type` field; new clear
/// path requires one. Idempotent: skips if target file already exists.
pub fn migrate_legacy_if_needed() -> Result<(), AppError> {
    let old_path = crate::paths::legacy_cli_config_backup_file();
    let new_dir = crate::paths::cli_config_backup_dir();
    let new_path = new_dir.join("windows.json");

    if !old_path.exists() || new_path.exists() {
        return Ok(());
    }

    let bytes = match std::fs::read(&old_path) {
        Ok(b) => b,
        Err(e) => return Err(AppError::Config(format!("legacy snapshot read: {e}"))),
    };
    let mut v: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            // Move aside so we don't retry every boot.
            let aside = old_path.with_extension("corrupt");
            log::warn!(
                "legacy snapshot malformed ({e}); moving to {}",
                aside.display()
            );
            let _ = std::fs::rename(&old_path, aside);
            return Ok(());
        }
    };
    let obj = match v.as_object_mut() {
        Some(o) => o,
        None => return Err(AppError::Config("legacy snapshot is not a JSON object".into())),
    };
    obj.insert("target_type".into(), json!("windows"));
    obj.entry("captured_at")
        .or_insert_with(|| json!(chrono::Utc::now().to_rfc3339()));
    std::fs::create_dir_all(&new_dir)?;
    let new_bytes = serde_json::to_vec_pretty(&v)?;
    crate::cli_target::atomic_write(&new_path, &new_bytes)?;
    std::fs::remove_file(&old_path)?;
    log::info!(
        "migrated legacy CLI snapshot → {}",
        new_path.display()
    );
    Ok(())
}

// ─── Capture helpers (per-CLI, via backend) ───

fn capture_claude(backend: &dyn CliBackend) -> Result<ClaudeSnapshot, AppError> {
    let mut snap = ClaudeSnapshot::default();
    let Some(content) = backend.read(&[".claude", "settings.json"])? else { return Ok(snap); };
    let Ok(val) = serde_json::from_str::<Value>(&content) else { return Ok(snap); };
    let Some(env) = val.get("env").and_then(|v| v.as_object()) else { return Ok(snap); };
    let get = |k: &str| env.get(k).and_then(|v| v.as_str()).map(String::from);
    snap.anthropic_base_url = get("ANTHROPIC_BASE_URL");
    snap.anthropic_model = get("ANTHROPIC_MODEL");
    snap.anthropic_small_fast_model = get("ANTHROPIC_SMALL_FAST_MODEL");
    snap.anthropic_auth_token = get("ANTHROPIC_AUTH_TOKEN");
    Ok(snap)
}

fn capture_codex(backend: &dyn CliBackend) -> Result<CodexSnapshot, AppError> {
    let mut snap = CodexSnapshot::default();
    if let Some(content) = backend.read(&[".codex", "auth.json"])? {
        if let Ok(val) = serde_json::from_str::<Value>(&content) {
            snap.openai_api_key = val
                .get("OPENAI_API_KEY")
                .and_then(|v| v.as_str())
                .map(String::from);
        }
    }
    if let Some(content) = backend.read(&[".codex", "config.toml"])? {
        if let Ok(doc) = content.parse::<DocumentMut>() {
            snap.model = doc.get("model").and_then(|v| v.as_str()).map(String::from);
            snap.model_provider = doc
                .get("model_provider")
                .and_then(|v| v.as_str())
                .map(String::from);
            if let Some(gw) = doc
                .get("model_providers")
                .and_then(|v| v.as_table())
                .and_then(|t| t.get("copilot_gateway"))
                .and_then(|v| v.as_table())
            {
                snap.copilot_gateway_provider_toml = Some(gw.to_string());
            }
        }
    }
    Ok(snap)
}

fn capture_gemini(backend: &dyn CliBackend) -> Result<GeminiSnapshot, AppError> {
    let mut snap = GeminiSnapshot::default();
    if let Some(content) = backend.read(&[".gemini", ".env"])? {
        let env_map = parse_env_file(&content);
        snap.gemini_api_key = env_map.get("GEMINI_API_KEY").cloned();
        snap.google_gemini_base_url = env_map.get("GOOGLE_GEMINI_BASE_URL").cloned();
        snap.gemini_api_base_url = env_map.get("GEMINI_API_BASE_URL").cloned();
    }
    if let Some(content) = backend.read(&[".gemini", "settings.json"])? {
        if let Ok(val) = serde_json::from_str::<Value>(&content) {
            snap.selected_auth_type = val
                .get("security")
                .and_then(|v| v.get("auth"))
                .and_then(|v| v.get("selectedType"))
                .and_then(|v| v.as_str())
                .map(String::from);
        }
    }
    Ok(snap)
}

// ─── Restore helpers (per-CLI, via backend) ───

fn restore_claude_backend(snap: &ClaudeSnapshot, backend: &dyn CliBackend) -> Result<(), AppError> {
    let rel: &[&str] = &[".claude", "settings.json"];
    let Some(content) = backend.read(rel)? else { return Ok(()); };
    let mut settings: Value = serde_json::from_str(&content).unwrap_or_else(|_| json!({}));
    let env = settings
        .as_object_mut()
        .unwrap()
        .entry("env")
        .or_insert_with(|| json!({}));
    if let Some(env_obj) = env.as_object_mut() {
        let apply = |obj: &mut serde_json::Map<String, Value>, k: &str, v: &Option<String>| match v {
            Some(s) => {
                obj.insert(k.into(), Value::String(s.clone()));
            }
            None => {
                obj.remove(k);
            }
        };
        apply(env_obj, "ANTHROPIC_BASE_URL", &snap.anthropic_base_url);
        apply(env_obj, "ANTHROPIC_MODEL", &snap.anthropic_model);
        apply(env_obj, "ANTHROPIC_SMALL_FAST_MODEL", &snap.anthropic_small_fast_model);
        apply(env_obj, "ANTHROPIC_AUTH_TOKEN", &snap.anthropic_auth_token);
    }
    backend.write_atomic(rel, serde_json::to_string_pretty(&settings)?.as_bytes())
}

fn restore_codex_backend(snap: &CodexSnapshot, backend: &dyn CliBackend) -> Result<(), AppError> {
    if backend.exists(&[".codex", "auth.json"])? {
        let content = backend.read(&[".codex", "auth.json"])?.unwrap_or_default();
        let mut auth: Value = serde_json::from_str(&content).unwrap_or_else(|_| json!({}));
        if let Some(obj) = auth.as_object_mut() {
            match &snap.openai_api_key {
                Some(k) => {
                    obj.insert("OPENAI_API_KEY".into(), Value::String(k.clone()));
                }
                None => {
                    obj.remove("OPENAI_API_KEY");
                }
            }
        }
        backend.write_atomic(
            &[".codex", "auth.json"],
            serde_json::to_string_pretty(&auth)?.as_bytes(),
        )?;
    }
    if backend.exists(&[".codex", "config.toml"])? {
        let content = backend.read(&[".codex", "config.toml"])?.unwrap_or_default();
        if let Ok(mut doc) = content.parse::<DocumentMut>() {
            match &snap.model {
                Some(m) => {
                    doc["model"] = toml_edit::value(m.clone());
                }
                None => {
                    doc.as_table_mut().remove("model");
                }
            }
            match &snap.model_provider {
                Some(p) => {
                    doc["model_provider"] = toml_edit::value(p.clone());
                }
                None => {
                    doc.as_table_mut().remove("model_provider");
                }
            }
            if let Some(providers) = doc.get_mut("model_providers").and_then(|v| v.as_table_mut()) {
                providers.remove("copilot_gateway");
            }
            if let Some(orig_toml) = &snap.copilot_gateway_provider_toml {
                if doc.get("model_providers").is_none() {
                    doc["model_providers"] = toml_edit::table();
                }
                let wrapper = format!("[model_providers.copilot_gateway]\n{orig_toml}");
                if let Ok(parsed) = wrapper.parse::<DocumentMut>() {
                    if let Some(orig_gw) = parsed
                        .get("model_providers")
                        .and_then(|v| v.as_table())
                        .and_then(|t| t.get("copilot_gateway"))
                        .and_then(|v| v.as_table())
                    {
                        if let Some(providers) =
                            doc.get_mut("model_providers").and_then(|v| v.as_table_mut())
                        {
                            providers.insert("copilot_gateway", toml_edit::Item::Table(orig_gw.clone()));
                        }
                    }
                }
            }
            let empty = doc
                .get("model_providers")
                .and_then(|v| v.as_table())
                .map(|t| t.is_empty())
                .unwrap_or(false);
            if empty {
                doc.as_table_mut().remove("model_providers");
            }
            backend.write_atomic(&[".codex", "config.toml"], doc.to_string().as_bytes())?;
        }
    }
    Ok(())
}

fn restore_gemini_backend(snap: &GeminiSnapshot, backend: &dyn CliBackend) -> Result<(), AppError> {
    if backend.exists(&[".gemini", ".env"])? {
        let content = backend.read(&[".gemini", ".env"])?.unwrap_or_default();
        let mut env_map = parse_env_file(&content);
        let apply = |m: &mut std::collections::HashMap<String, String>, k: &str, v: &Option<String>| {
            match v {
                Some(s) => {
                    m.insert(k.into(), s.clone());
                }
                None => {
                    m.remove(k);
                }
            }
        };
        apply(&mut env_map, "GEMINI_API_KEY", &snap.gemini_api_key);
        apply(&mut env_map, "GOOGLE_GEMINI_BASE_URL", &snap.google_gemini_base_url);
        apply(&mut env_map, "GEMINI_API_BASE_URL", &snap.gemini_api_base_url);
        backend.write_atomic(&[".gemini", ".env"], serialize_env_file(&env_map).as_bytes())?;
    }
    if backend.exists(&[".gemini", "settings.json"])? {
        let content = backend.read(&[".gemini", "settings.json"])?.unwrap_or_default();
        let mut settings: Value = serde_json::from_str(&content).unwrap_or_else(|_| json!({}));
        if let Some(obj) = settings.as_object_mut() {
            match &snap.selected_auth_type {
                Some(t) => {
                    let security = obj.entry("security").or_insert_with(|| json!({}));
                    if let Some(sec_obj) = security.as_object_mut() {
                        let auth = sec_obj.entry("auth").or_insert_with(|| json!({}));
                        if let Some(auth_obj) = auth.as_object_mut() {
                            auth_obj.insert("selectedType".into(), Value::String(t.clone()));
                        }
                    }
                }
                None => {
                    if let Some(auth_obj) = obj
                        .get_mut("security")
                        .and_then(|v| v.as_object_mut())
                        .and_then(|s| s.get_mut("auth"))
                        .and_then(|v| v.as_object_mut())
                    {
                        auth_obj.remove("selectedType");
                    }
                }
            }
        }
        backend.write_atomic(
            &[".gemini", "settings.json"],
            serde_json::to_string_pretty(&settings)?.as_bytes(),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_id_stable_and_distinct_for_distinct_names() {
        let a = SnapshotMeta { target_type: TargetType::Wsl, distro_name: Some("Ubuntu 22.04".into()), home: None };
        let b = SnapshotMeta { target_type: TargetType::Wsl, distro_name: Some("Ubuntu_22.04".into()), home: None };
        let n1 = target_file_name(&a);
        let n2 = target_file_name(&a);
        let n3 = target_file_name(&b);
        assert_eq!(n1, n2);
        assert_ne!(n1, n3);
        assert!(n1.starts_with("wsl-") && n1.ends_with(".json"));
    }

    #[test]
    fn windows_target_uses_fixed_filename() {
        let m = SnapshotMeta { target_type: TargetType::Windows, distro_name: None, home: None };
        assert_eq!(target_file_name(&m), "windows.json");
    }
}
