//! Per-target CLI config snapshots.
//!
//! One JSON file per target under `paths::cli_config_backup_dir()`.
//! Filenames are opaque sha256-derived ids — original distro name,
//! target_type, and probed home all live inside the JSON, so restore
//! never depends on filename parsing (and distro names with spaces
//! don't collide).

use super::{parse_env_file, serialize_env_file, ClaudeSnapshot, CodexSnapshot, GeminiSnapshot};
use crate::cli_target::{CliBackend, CliTarget, SnapshotMeta, TargetType};
use crate::AppError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
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

pub fn capture_extra_env_originals(
    target: &CliTarget,
    extra_env: Option<&BTreeMap<String, String>>,
) -> Result<(), AppError> {
    let Some(mut snapshot) = read(&target.snapshot_meta)? else {
        return Ok(());
    };
    let Some(extra_env) = extra_env else {
        return Ok(());
    };
    if extra_env.is_empty() {
        return Ok(());
    }
    let current = target
        .backend
        .read(&[".claude", "settings.json"])?
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .and_then(|settings| settings.get("env").and_then(Value::as_object).cloned())
        .unwrap_or_default();
    let mut changed = false;
    for key in extra_env.keys() {
        if snapshot.claude.extra_env_originals.contains_key(key) {
            continue;
        }
        snapshot.claude.extra_env_originals.insert(
            key.clone(),
            current.get(key).and_then(Value::as_str).map(String::from),
        );
        changed = true;
    }
    if !snapshot.claude.extra_env_captured {
        snapshot.claude.extra_env_captured = true;
        changed = true;
    }
    if changed {
        let path = snapshot_path(&target.snapshot_meta);
        crate::cli_target::atomic_write(&path, &serde_json::to_vec_pretty(&snapshot)?)?;
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
            target_type: if snap.target_type == "wsl" {
                TargetType::Wsl
            } else {
                TargetType::Windows
            },
            distro_name: snap.distro_name.clone(),
            home: snap.home.clone(),
        };
        let key = snap.distro_name.unwrap_or_else(|| "windows".to_string());
        map.insert(key, meta);
    }
    Ok(map)
}

/// Walk the backup directory and return every snapshot, parsed. Used by
/// the GUI Disable dialog to show "what will be restored" per target.
/// Malformed files are skipped (logged).
pub fn list_all() -> Result<Vec<TargetSnapshot>, AppError> {
    let dir = crate::paths::cli_config_backup_dir();
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
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
        match serde_json::from_slice::<TargetSnapshot>(&bytes) {
            Ok(s) => out.push(s),
            Err(e) => log::warn!("ignoring malformed snapshot {}: {e}", path.display()),
        }
    }
    // Stable order: windows first, then wsl by distro name.
    out.sort_by(
        |a, b| match (a.target_type.as_str(), b.target_type.as_str()) {
            ("windows", "windows") => std::cmp::Ordering::Equal,
            ("windows", _) => std::cmp::Ordering::Less,
            (_, "windows") => std::cmp::Ordering::Greater,
            _ => a.distro_name.cmp(&b.distro_name),
        },
    );
    Ok(out)
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
        None => {
            return Err(AppError::Config(
                "legacy snapshot is not a JSON object".into(),
            ))
        }
    };
    obj.insert("target_type".into(), json!("windows"));
    obj.entry("captured_at")
        .or_insert_with(|| json!(chrono::Utc::now().to_rfc3339()));
    std::fs::create_dir_all(&new_dir)?;
    let new_bytes = serde_json::to_vec_pretty(&v)?;
    crate::cli_target::atomic_write(&new_path, &new_bytes)?;
    std::fs::remove_file(&old_path)?;
    log::info!("migrated legacy CLI snapshot → {}", new_path.display());
    Ok(())
}

// ─── Capture helpers (per-CLI, via backend) ───

fn capture_claude(backend: &dyn CliBackend) -> Result<ClaudeSnapshot, AppError> {
    let mut snap = ClaudeSnapshot {
        extended_fields_captured: true,
        ..ClaudeSnapshot::default()
    };
    let Some(content) = backend.read(&[".claude", "settings.json"])? else {
        return Ok(snap);
    };
    let val = serde_json::from_str::<Value>(&content)?;
    let settings = val.as_object().ok_or_else(|| {
        AppError::Config(
            "cannot snapshot ~/.claude/settings.json: root must be a JSON object".into(),
        )
    })?;
    let Some(env) = settings.get("env") else {
        return Ok(snap);
    };
    let env = env.as_object().ok_or_else(|| {
        AppError::Config(
            "cannot snapshot ~/.claude/settings.json: env must be a JSON object".into(),
        )
    })?;
    let get = |k: &str| env.get(k).and_then(|v| v.as_str()).map(String::from);
    snap.anthropic_base_url = get("ANTHROPIC_BASE_URL");
    snap.anthropic_model = get("ANTHROPIC_MODEL");
    snap.anthropic_small_fast_model = get("ANTHROPIC_SMALL_FAST_MODEL");
    snap.anthropic_auth_token = get("ANTHROPIC_AUTH_TOKEN");
    snap.claude_code_subagent_model = get("CLAUDE_CODE_SUBAGENT_MODEL");
    snap.anthropic_default_haiku_model = get("ANTHROPIC_DEFAULT_HAIKU_MODEL");
    snap.anthropic_custom_headers = get("ANTHROPIC_CUSTOM_HEADERS");
    Ok(snap)
}

fn backfill_extended_claude_fields(
    snap: &mut ClaudeSnapshot,
    backend: &dyn CliBackend,
) -> Result<bool, AppError> {
    if snap.extended_fields_captured {
        return Ok(false);
    }
    let current = capture_claude(backend)?;
    snap.claude_code_subagent_model = current.claude_code_subagent_model;
    snap.anthropic_default_haiku_model = current.anthropic_default_haiku_model;
    snap.anthropic_custom_headers = current.anthropic_custom_headers;
    snap.extended_fields_captured = true;
    Ok(true)
}

fn backfill_special_codex_fields(
    snap: &mut CodexSnapshot,
    backend: &dyn CliBackend,
) -> Result<bool, AppError> {
    let need_main_effort = !snap.special_fields_captured;
    let need_subagent_defaults = !snap.codex_subagent_defaults_captured;
    if !need_main_effort && !need_subagent_defaults {
        return Ok(false);
    }
    let current = capture_codex(backend)?;
    if need_main_effort {
        snap.model_reasoning_effort = current.model_reasoning_effort;
        snap.special_fields_captured = true;
    }
    if need_subagent_defaults {
        snap.default_subagent_model = current.default_subagent_model;
        snap.default_subagent_reasoning_effort = current.default_subagent_reasoning_effort;
        snap.codex_subagent_defaults_captured = true;
    }
    Ok(true)
}

/// Upgrade an older snapshot before an apply overwrites newly-managed fields.
pub fn backfill_extended_snapshot(target: &CliTarget) -> Result<(), AppError> {
    let Some(mut snap) = read(&target.snapshot_meta)? else {
        return Ok(());
    };
    let claude_changed = backfill_extended_claude_fields(&mut snap.claude, &*target.backend)?;
    let codex_changed = backfill_special_codex_fields(&mut snap.codex, &*target.backend)?;
    if !claude_changed && !codex_changed {
        return Ok(());
    }
    let path = snapshot_path(&target.snapshot_meta);
    let bytes = serde_json::to_vec_pretty(&snap)?;
    crate::cli_target::atomic_write(&path, &bytes)
}

fn capture_codex(backend: &dyn CliBackend) -> Result<CodexSnapshot, AppError> {
    let mut snap = CodexSnapshot {
        special_fields_captured: true,
        codex_subagent_defaults_captured: true,
        ..CodexSnapshot::default()
    };
    if let Some(content) = backend.read(&[".codex", "auth.json"])? {
        if let Ok(val) = serde_json::from_str::<Value>(&content) {
            snap.openai_api_key = val
                .get("OPENAI_API_KEY")
                .and_then(|v| v.as_str())
                .map(String::from);
        }
    }
    if let Some(content) = backend.read(&[".codex", "config.toml"])? {
        let doc = content
            .parse::<DocumentMut>()
            .map_err(|e| AppError::Config(format!("cannot snapshot ~/.codex/config.toml: {e}")))?;
        snap.model = doc.get("model").and_then(|v| v.as_str()).map(String::from);
        snap.model_provider = doc
            .get("model_provider")
            .and_then(|v| v.as_str())
            .map(String::from);
        snap.model_reasoning_effort = doc
            .get("model_reasoning_effort")
            .and_then(|v| v.as_str())
            .map(String::from);
        snap.default_subagent_model = doc
            .get("default_subagent_model")
            .and_then(|v| v.as_str())
            .map(String::from);
        snap.default_subagent_reasoning_effort = doc
            .get("default_subagent_reasoning_effort")
            .and_then(|v| v.as_str())
            .map(String::from);
        snap.copilot_gateway_provider_toml = doc
            .get("model_providers")
            .and_then(|v| v.as_table_like())
            .and_then(|t| t.get("copilot_gateway"))
            .and_then(super::table_item_to_table)
            .map(|t| t.to_string());
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
    let Some(content) = backend.read(rel)? else {
        return Ok(());
    };
    let mut settings: Value = serde_json::from_str(&content).unwrap_or_else(|_| json!({}));
    let env = settings
        .as_object_mut()
        .unwrap()
        .entry("env")
        .or_insert_with(|| json!({}));
    if let Some(env_obj) = env.as_object_mut() {
        let apply = |obj: &mut serde_json::Map<String, Value>, k: &str, v: &Option<String>| match v
        {
            Some(s) => {
                obj.insert(k.into(), Value::String(s.clone()));
            }
            None => {
                obj.remove(k);
            }
        };
        apply(env_obj, "ANTHROPIC_BASE_URL", &snap.anthropic_base_url);
        apply(env_obj, "ANTHROPIC_MODEL", &snap.anthropic_model);
        apply(
            env_obj,
            "ANTHROPIC_SMALL_FAST_MODEL",
            &snap.anthropic_small_fast_model,
        );
        apply(env_obj, "ANTHROPIC_AUTH_TOKEN", &snap.anthropic_auth_token);
        if snap.extended_fields_captured {
            apply(
                env_obj,
                "CLAUDE_CODE_SUBAGENT_MODEL",
                &snap.claude_code_subagent_model,
            );
            apply(
                env_obj,
                "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                &snap.anthropic_default_haiku_model,
            );
            apply(
                env_obj,
                "ANTHROPIC_CUSTOM_HEADERS",
                &snap.anthropic_custom_headers,
            );
        }
        if snap.extra_env_captured {
            for (key, original) in &snap.extra_env_originals {
                apply(env_obj, key, original);
            }
        }
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
        let content = backend
            .read(&[".codex", "config.toml"])?
            .unwrap_or_default();
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
            if snap.special_fields_captured {
                match &snap.model_reasoning_effort {
                    Some(effort) => {
                        doc["model_reasoning_effort"] = toml_edit::value(effort.clone());
                    }
                    None => {
                        doc.as_table_mut().remove("model_reasoning_effort");
                    }
                }
            }
            if snap.codex_subagent_defaults_captured {
                match &snap.default_subagent_model {
                    Some(model) => {
                        doc["default_subagent_model"] = toml_edit::value(model.clone());
                    }
                    None => {
                        doc.as_table_mut().remove("default_subagent_model");
                    }
                }
                match &snap.default_subagent_reasoning_effort {
                    Some(effort) => {
                        doc["default_subagent_reasoning_effort"] = toml_edit::value(effort.clone());
                    }
                    None => {
                        doc.as_table_mut()
                            .remove("default_subagent_reasoning_effort");
                    }
                }
            }
            if let Some(providers) = doc
                .get_mut("model_providers")
                .and_then(|v| v.as_table_like_mut())
            {
                providers.remove("copilot_gateway");
            }
            if let Some(orig_toml) = &snap.copilot_gateway_provider_toml {
                let wrapper = format!("[model_providers.copilot_gateway]\n{orig_toml}");
                if let Ok(parsed) = wrapper.parse::<DocumentMut>() {
                    if let Some(orig_gw) = parsed
                        .get("model_providers")
                        .and_then(|v| v.as_table_like())
                        .and_then(|t| t.get("copilot_gateway"))
                        .and_then(super::table_item_to_table)
                    {
                        let providers = super::ensure_table_item(&mut doc["model_providers"]);
                        providers.insert("copilot_gateway", toml_edit::Item::Table(orig_gw));
                    }
                }
            }
            let empty = doc
                .get("model_providers")
                .and_then(|v| v.as_table_like())
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
        let apply = |m: &mut std::collections::HashMap<String, String>,
                     k: &str,
                     v: &Option<String>| {
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
        apply(
            &mut env_map,
            "GOOGLE_GEMINI_BASE_URL",
            &snap.google_gemini_base_url,
        );
        apply(
            &mut env_map,
            "GEMINI_API_BASE_URL",
            &snap.gemini_api_base_url,
        );
        backend.write_atomic(
            &[".gemini", ".env"],
            serialize_env_file(&env_map).as_bytes(),
        )?;
    }
    if backend.exists(&[".gemini", "settings.json"])? {
        let content = backend
            .read(&[".gemini", "settings.json"])?
            .unwrap_or_default();
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
    use crate::cli_target::WindowsFsBackend;
    use tempfile::TempDir;

    fn fake_backend(tmp: &TempDir) -> WindowsFsBackend {
        WindowsFsBackend {
            home: tmp.path().to_path_buf(),
        }
    }

    #[test]
    fn invalid_claude_config_cannot_produce_an_authoritative_snapshot() {
        let tmp = TempDir::new().unwrap();
        let backend = fake_backend(&tmp);
        backend
            .write_atomic(&[".claude", "settings.json"], br#"{"env":null}"#)
            .unwrap();

        let err = capture_claude(&backend).unwrap_err();
        assert!(err.to_string().contains("env must be a JSON object"));
    }

    #[test]
    fn malformed_codex_config_cannot_produce_an_authoritative_snapshot() {
        let tmp = TempDir::new().unwrap();
        let backend = fake_backend(&tmp);
        backend
            .write_atomic(&[".codex", "config.toml"], b"[agents\ninvalid")
            .unwrap();

        let err = capture_codex(&backend).unwrap_err();
        assert!(err
            .to_string()
            .contains("cannot snapshot ~/.codex/config.toml"));
    }

    #[test]
    fn captures_and_restores_extended_claude_fields() {
        let tmp = TempDir::new().unwrap();
        let backend = fake_backend(&tmp);
        backend
            .write_atomic(
                &[".claude", "settings.json"],
                br#"{"env":{"CLAUDE_CODE_SUBAGENT_MODEL":"before-sub","ANTHROPIC_DEFAULT_HAIKU_MODEL":"before-haiku","ANTHROPIC_CUSTOM_HEADERS":"before-header","ANTHROPIC_SMALL_FAST_MODEL":"before-small"}}"#,
            )
            .unwrap();
        let snap = capture_claude(&backend).unwrap();
        assert!(snap.extended_fields_captured);

        backend
            .write_atomic(
                &[".claude", "settings.json"],
                br#"{"env":{"CLAUDE_CODE_SUBAGENT_MODEL":"after","ANTHROPIC_DEFAULT_HAIKU_MODEL":"after"}}"#,
            )
            .unwrap();
        restore_claude_backend(&snap, &backend).unwrap();
        let value: Value = serde_json::from_str(
            &backend
                .read(&[".claude", "settings.json"])
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let env = value["env"].as_object().unwrap();
        assert_eq!(env["CLAUDE_CODE_SUBAGENT_MODEL"], "before-sub");
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "before-haiku");
        assert_eq!(env["ANTHROPIC_CUSTOM_HEADERS"], "before-header");
        assert_eq!(env["ANTHROPIC_SMALL_FAST_MODEL"], "before-small");
    }

    #[test]
    fn legacy_snapshot_restore_does_not_touch_uncaptured_fields() {
        let tmp = TempDir::new().unwrap();
        let backend = fake_backend(&tmp);
        backend
            .write_atomic(
                &[".claude", "settings.json"],
                br#"{"env":{"ANTHROPIC_CUSTOM_HEADERS":"current","CLAUDE_CODE_SUBAGENT_MODEL":"current-sub"}}"#,
            )
            .unwrap();
        let legacy: ClaudeSnapshot = serde_json::from_str("{}").unwrap();
        assert!(!legacy.extended_fields_captured);
        restore_claude_backend(&legacy, &backend).unwrap();
        let value: Value = serde_json::from_str(
            &backend
                .read(&[".claude", "settings.json"])
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(value["env"]["ANTHROPIC_CUSTOM_HEADERS"], "current");
        assert_eq!(value["env"]["CLAUDE_CODE_SUBAGENT_MODEL"], "current-sub");
    }

    #[test]
    fn backfill_records_current_extended_fields() {
        let tmp = TempDir::new().unwrap();
        let backend = fake_backend(&tmp);
        backend
            .write_atomic(
                &[".claude", "settings.json"],
                br#"{"env":{"ANTHROPIC_CUSTOM_HEADERS":"original","ANTHROPIC_DEFAULT_HAIKU_MODEL":"original-haiku"}}"#,
            )
            .unwrap();
        let mut legacy = ClaudeSnapshot::default();
        assert!(backfill_extended_claude_fields(&mut legacy, &backend).unwrap());
        assert!(legacy.extended_fields_captured);
        assert_eq!(legacy.anthropic_custom_headers.as_deref(), Some("original"));
        assert_eq!(
            legacy.anthropic_default_haiku_model.as_deref(),
            Some("original-haiku")
        );
        assert!(!backfill_extended_claude_fields(&mut legacy, &backend).unwrap());
    }

    #[test]
    fn captures_and_restores_codex_top_level_defaults() {
        let tmp = TempDir::new().unwrap();
        let backend = fake_backend(&tmp);
        backend
            .write_atomic(
                &[".codex", "config.toml"],
                br#"model_reasoning_effort = "medium"
default_subagent_model = "before-sub"
default_subagent_reasoning_effort = "low"
"#,
            )
            .unwrap();
        let snap = capture_codex(&backend).unwrap();
        assert!(snap.special_fields_captured);
        assert!(snap.codex_subagent_defaults_captured);
        assert_eq!(snap.model_reasoning_effort.as_deref(), Some("medium"));
        assert_eq!(snap.default_subagent_model.as_deref(), Some("before-sub"));
        assert_eq!(
            snap.default_subagent_reasoning_effort.as_deref(),
            Some("low")
        );

        backend
            .write_atomic(
                &[".codex", "config.toml"],
                br#"model_reasoning_effort = "high"
default_subagent_model = "after-sub"
default_subagent_reasoning_effort = "high"
"#,
            )
            .unwrap();
        restore_codex_backend(&snap, &backend).unwrap();
        let content = backend.read(&[".codex", "config.toml"]).unwrap().unwrap();
        let doc = content.parse::<DocumentMut>().unwrap();
        assert_eq!(doc["model_reasoning_effort"].as_str(), Some("medium"));
        assert_eq!(doc["default_subagent_model"].as_str(), Some("before-sub"));
        assert_eq!(
            doc["default_subagent_reasoning_effort"].as_str(),
            Some("low")
        );
    }

    #[test]
    fn captures_inline_codex_provider_table() {
        let tmp = TempDir::new().unwrap();
        let backend = fake_backend(&tmp);
        backend
            .write_atomic(
                &[".codex", "config.toml"],
                br#"model_providers = { copilot_gateway = { name = "Mine", custom = "keep" } }
"#,
            )
            .unwrap();

        let snap = capture_codex(&backend).unwrap();
        assert!(snap
            .copilot_gateway_provider_toml
            .as_deref()
            .is_some_and(|value| value.contains("custom = \"keep\"")));
    }

    #[test]
    fn legacy_codex_snapshot_does_not_touch_uncaptured_defaults() {
        let tmp = TempDir::new().unwrap();
        let backend = fake_backend(&tmp);
        backend
            .write_atomic(
                &[".codex", "config.toml"],
                br#"model_reasoning_effort = "high"
default_subagent_model = "current-sub"
default_subagent_reasoning_effort = "high"
"#,
            )
            .unwrap();
        let legacy: CodexSnapshot = serde_json::from_str("{}").unwrap();
        assert!(!legacy.special_fields_captured);
        assert!(!legacy.codex_subagent_defaults_captured);
        restore_codex_backend(&legacy, &backend).unwrap();
        let content = backend.read(&[".codex", "config.toml"]).unwrap().unwrap();
        assert!(content.contains("model_reasoning_effort = \"high\""));
        assert!(content.contains("default_subagent_model = \"current-sub\""));
    }

    #[test]
    fn backfill_records_current_codex_defaults() {
        let tmp = TempDir::new().unwrap();
        let backend = fake_backend(&tmp);
        backend
            .write_atomic(
                &[".codex", "config.toml"],
                br#"model_reasoning_effort = "low"
default_subagent_model = "original-sub"
default_subagent_reasoning_effort = "medium"
"#,
            )
            .unwrap();
        let mut legacy = CodexSnapshot::default();
        assert!(backfill_special_codex_fields(&mut legacy, &backend).unwrap());
        assert!(legacy.special_fields_captured);
        assert!(legacy.codex_subagent_defaults_captured);
        assert_eq!(legacy.model_reasoning_effort.as_deref(), Some("low"));
        assert_eq!(
            legacy.default_subagent_model.as_deref(),
            Some("original-sub")
        );
        assert_eq!(
            legacy.default_subagent_reasoning_effort.as_deref(),
            Some("medium")
        );
        assert!(!backfill_special_codex_fields(&mut legacy, &backend).unwrap());
    }

    #[test]
    fn opaque_id_stable_and_distinct_for_distinct_names() {
        let a = SnapshotMeta {
            target_type: TargetType::Wsl,
            distro_name: Some("Ubuntu 22.04".into()),
            home: None,
        };
        let b = SnapshotMeta {
            target_type: TargetType::Wsl,
            distro_name: Some("Ubuntu_22.04".into()),
            home: None,
        };
        let n1 = target_file_name(&a);
        let n2 = target_file_name(&a);
        let n3 = target_file_name(&b);
        assert_eq!(n1, n2);
        assert_ne!(n1, n3);
        assert!(n1.starts_with("wsl-") && n1.ends_with(".json"));
    }

    #[test]
    fn windows_target_uses_fixed_filename() {
        let m = SnapshotMeta {
            target_type: TargetType::Windows,
            distro_name: None,
            home: None,
        };
        assert_eq!(target_file_name(&m), "windows.json");
    }
}
