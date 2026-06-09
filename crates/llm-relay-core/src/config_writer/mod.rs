use crate::AppError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use toml_edit::DocumentMut;

pub mod snapshot;

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

// ─── Pre-apply snapshot ───
// Captured the first time apply_all_configs runs against a "clean" state so
// that disabling the relay can restore the user's original config — not just
// strip the keys the relay wrote.

fn snapshot_path() -> PathBuf {
    crate::paths::config_dir().join("cli-config-backup.json")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSnapshot {
    /// `null` ↔ the key was not present in env. Some("") is a real empty string.
    pub anthropic_base_url: Option<String>,
    pub anthropic_model: Option<String>,
    pub anthropic_small_fast_model: Option<String>,
    pub anthropic_auth_token: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSnapshot {
    pub openai_api_key: Option<String>,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    /// Serialized TOML of `[model_providers.copilot_gateway]` if it existed,
    /// so we can put the original subtable back verbatim.
    pub copilot_gateway_provider_toml: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiSnapshot {
    pub gemini_api_key: Option<String>,
    pub google_gemini_base_url: Option<String>,
    pub gemini_api_base_url: Option<String>,
    pub selected_auth_type: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliConfigSnapshot {
    pub captured_at: String,
    pub claude: ClaudeSnapshot,
    pub codex: CodexSnapshot,
    pub gemini: GeminiSnapshot,
}

fn capture_claude_snapshot() -> ClaudeSnapshot {
    let path = claude_settings_path();
    let mut snap = ClaudeSnapshot::default();
    if !path.exists() {
        return snap;
    }
    let Ok(content) = fs::read_to_string(&path) else { return snap };
    let Ok(val) = serde_json::from_str::<Value>(&content) else { return snap };
    let Some(env) = val.get("env").and_then(|v| v.as_object()) else { return snap };
    let get = |k: &str| env.get(k).and_then(|v| v.as_str()).map(String::from);
    snap.anthropic_base_url = get("ANTHROPIC_BASE_URL");
    snap.anthropic_model = get("ANTHROPIC_MODEL");
    snap.anthropic_small_fast_model = get("ANTHROPIC_SMALL_FAST_MODEL");
    snap.anthropic_auth_token = get("ANTHROPIC_AUTH_TOKEN");
    snap
}

fn capture_codex_snapshot() -> CodexSnapshot {
    let dir = codex_dir();
    let mut snap = CodexSnapshot::default();

    let auth_path = dir.join("auth.json");
    if auth_path.exists() {
        if let Ok(content) = fs::read_to_string(&auth_path) {
            if let Ok(val) = serde_json::from_str::<Value>(&content) {
                snap.openai_api_key = val
                    .get("OPENAI_API_KEY")
                    .and_then(|v| v.as_str())
                    .map(String::from);
            }
        }
    }

    let config_path = dir.join("config.toml");
    if config_path.exists() {
        if let Ok(content) = fs::read_to_string(&config_path) {
            if let Ok(doc) = content.parse::<DocumentMut>() {
                snap.model = doc.get("model").and_then(|v| v.as_str()).map(String::from);
                snap.model_provider = doc
                    .get("model_provider")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                // Serialize the entire `[model_providers.copilot_gateway]` subtable
                // if it pre-existed (so a restore preserves any user fields, not
                // just the ones the relay overwrites).
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
    }
    snap
}

fn capture_gemini_snapshot() -> GeminiSnapshot {
    let dir = gemini_dir();
    let mut snap = GeminiSnapshot::default();

    let env_path = dir.join(".env");
    if env_path.exists() {
        if let Ok(content) = fs::read_to_string(&env_path) {
            let env_map = parse_env_file(&content);
            snap.gemini_api_key = env_map.get("GEMINI_API_KEY").cloned();
            snap.google_gemini_base_url = env_map.get("GOOGLE_GEMINI_BASE_URL").cloned();
            snap.gemini_api_base_url = env_map.get("GEMINI_API_BASE_URL").cloned();
        }
    }

    let settings_path = dir.join("settings.json");
    if settings_path.exists() {
        if let Ok(content) = fs::read_to_string(&settings_path) {
            if let Ok(val) = serde_json::from_str::<Value>(&content) {
                snap.selected_auth_type = val
                    .get("security")
                    .and_then(|v| v.get("auth"))
                    .and_then(|v| v.get("selectedType"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
            }
        }
    }
    snap
}

/// Write a snapshot of the current CLI config state to disk if one doesn't
/// already exist. Called from `apply_all_configs` so that the FIRST apply
/// captures the user's pre-relay state; subsequent applies leave the snapshot
/// untouched (otherwise we'd snapshot the relay's own values).
fn capture_snapshot_if_absent() -> Result<(), AppError> {
    let path = snapshot_path();
    if path.exists() {
        return Ok(());
    }
    let snap = CliConfigSnapshot {
        captured_at: chrono::Utc::now().to_rfc3339(),
        claude: capture_claude_snapshot(),
        codex: capture_codex_snapshot(),
        gemini: capture_gemini_snapshot(),
    };
    fs::create_dir_all(crate::paths::config_dir())?;
    let json = serde_json::to_string_pretty(&snap)?;
    atomic_write(&path, json.as_bytes())
}

pub fn read_snapshot() -> Result<Option<CliConfigSnapshot>, AppError> {
    let path = snapshot_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)?;
    let snap: CliConfigSnapshot = serde_json::from_str(&content)?;
    Ok(Some(snap))
}

fn restore_claude(snap: &ClaudeSnapshot) -> Result<(), AppError> {
    let path = claude_settings_path();
    if !path.exists() {
        // Nothing to restore into. Snapshot was either empty or the user
        // deleted the file; either way we're done.
        return Ok(());
    }
    let content = fs::read_to_string(&path)?;
    let mut settings: Value =
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));
    let env = settings
        .as_object_mut()
        .unwrap()
        .entry("env")
        .or_insert_with(|| serde_json::json!({}));
    if let Some(env_obj) = env.as_object_mut() {
        let apply = |obj: &mut serde_json::Map<String, Value>, k: &str, v: &Option<String>| {
            match v {
                Some(s) => { obj.insert(k.to_string(), Value::String(s.clone())); }
                None => { obj.remove(k); }
            }
        };
        apply(env_obj, "ANTHROPIC_BASE_URL", &snap.anthropic_base_url);
        apply(env_obj, "ANTHROPIC_MODEL", &snap.anthropic_model);
        apply(env_obj, "ANTHROPIC_SMALL_FAST_MODEL", &snap.anthropic_small_fast_model);
        apply(env_obj, "ANTHROPIC_AUTH_TOKEN", &snap.anthropic_auth_token);
    }
    let json_str = serde_json::to_string_pretty(&settings)?;
    atomic_write(&path, json_str.as_bytes())
}

fn restore_codex(snap: &CodexSnapshot) -> Result<(), AppError> {
    let dir = codex_dir();

    let auth_path = dir.join("auth.json");
    if auth_path.exists() {
        let content = fs::read_to_string(&auth_path)?;
        let mut auth: Value =
            serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(obj) = auth.as_object_mut() {
            match &snap.openai_api_key {
                Some(k) => { obj.insert("OPENAI_API_KEY".to_string(), Value::String(k.clone())); }
                None => { obj.remove("OPENAI_API_KEY"); }
            }
        }
        let json_str = serde_json::to_string_pretty(&auth)?;
        atomic_write(&auth_path, json_str.as_bytes())?;
    }

    let config_path = dir.join("config.toml");
    if config_path.exists() {
        let content = fs::read_to_string(&config_path)?;
        if let Ok(mut doc) = content.parse::<DocumentMut>() {
            // model / model_provider
            match &snap.model {
                Some(m) => { doc["model"] = toml_edit::value(m.clone()); }
                None => { doc.as_table_mut().remove("model"); }
            }
            match &snap.model_provider {
                Some(p) => { doc["model_provider"] = toml_edit::value(p.clone()); }
                None => { doc.as_table_mut().remove("model_provider"); }
            }
            // copilot_gateway subtable: restore original verbatim, or remove entirely
            if let Some(providers) = doc.get_mut("model_providers").and_then(|v| v.as_table_mut()) {
                providers.remove("copilot_gateway");
            }
            if let Some(orig_toml) = &snap.copilot_gateway_provider_toml {
                if doc.get("model_providers").is_none() {
                    doc["model_providers"] = toml_edit::table();
                }
                // Parse the original subtable back in via a wrapper doc.
                let wrapper = format!("[model_providers.copilot_gateway]\n{orig_toml}");
                if let Ok(parsed) = wrapper.parse::<DocumentMut>() {
                    if let Some(orig_gw) = parsed
                        .get("model_providers")
                        .and_then(|v| v.as_table())
                        .and_then(|t| t.get("copilot_gateway"))
                        .and_then(|v| v.as_table())
                    {
                        if let Some(providers) = doc.get_mut("model_providers").and_then(|v| v.as_table_mut()) {
                            providers.insert("copilot_gateway", toml_edit::Item::Table(orig_gw.clone()));
                        }
                    }
                }
            }
            // Drop model_providers entirely if now empty, to avoid leaving an empty table behind.
            let providers_empty = doc
                .get("model_providers")
                .and_then(|v| v.as_table())
                .map(|t| t.is_empty())
                .unwrap_or(false);
            if providers_empty {
                doc.as_table_mut().remove("model_providers");
            }
            atomic_write(&config_path, doc.to_string().as_bytes())?;
        }
    }
    Ok(())
}

fn restore_gemini(snap: &GeminiSnapshot) -> Result<(), AppError> {
    let dir = gemini_dir();

    let env_path = dir.join(".env");
    if env_path.exists() {
        let content = fs::read_to_string(&env_path)?;
        let mut env_map = parse_env_file(&content);
        let apply = |m: &mut HashMap<String, String>, k: &str, v: &Option<String>| {
            match v {
                Some(s) => { m.insert(k.to_string(), s.clone()); }
                None => { m.remove(k); }
            }
        };
        apply(&mut env_map, "GEMINI_API_KEY", &snap.gemini_api_key);
        apply(&mut env_map, "GOOGLE_GEMINI_BASE_URL", &snap.google_gemini_base_url);
        apply(&mut env_map, "GEMINI_API_BASE_URL", &snap.gemini_api_base_url);
        atomic_write(&env_path, serialize_env_file(&env_map).as_bytes())?;
    }

    let settings_path = dir.join("settings.json");
    if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)?;
        let mut settings: Value =
            serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(obj) = settings.as_object_mut() {
            // We only manipulate security.auth.selectedType. If the snapshot
            // says "was unset" and the path exists, remove just that leaf.
            match &snap.selected_auth_type {
                Some(t) => {
                    let security = obj
                        .entry("security")
                        .or_insert_with(|| serde_json::json!({}));
                    if let Some(sec_obj) = security.as_object_mut() {
                        let auth = sec_obj
                            .entry("auth")
                            .or_insert_with(|| serde_json::json!({}));
                        if let Some(auth_obj) = auth.as_object_mut() {
                            auth_obj.insert("selectedType".to_string(), Value::String(t.clone()));
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
        let json_str = serde_json::to_string_pretty(&settings)?;
        atomic_write(&settings_path, json_str.as_bytes())?;
    }
    Ok(())
}

fn delete_snapshot() -> Result<(), AppError> {
    let path = snapshot_path();
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

/// Atomic write: write to a temp file then rename.
fn atomic_write(path: &PathBuf, content: &[u8]) -> Result<(), AppError> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Use nanosecond timestamp to avoid temp file name conflicts
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let file_name = path.file_name()
        .ok_or_else(|| AppError::Config("Invalid file name".to_string()))?
        .to_string_lossy();
    let mut tmp = path.parent()
        .ok_or_else(|| AppError::Config("Invalid path".to_string()))?
        .to_path_buf();
    tmp.push(format!("{}.tmp.{}", file_name, ts));

    // Write to temp file with explicit flush
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(content)?;
        f.flush()?;
    }

    // Preserve file permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            let perm = meta.permissions().mode();
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(perm));
        }
    }

    // Windows special handling: remove then rename
    #[cfg(windows)]
    {
        if path.exists() {
            let _ = fs::remove_file(path);
        }
    }

    fs::rename(&tmp, path)?;
    Ok(())
}

// ─── Claude Code ───
// ~/.claude/settings.json — merge into "env" block

fn claude_settings_path() -> PathBuf {
    home_dir().join(".claude").join("settings.json")
}

/// Split a Copilot composite Claude id into (baseId, effort, context1m).
/// Mirrors `parseCompositeModelId` in copilot-api-gateway/src/services/copilot/variants.ts:
/// strips up to two known suffixes (`-high|-xhigh` → effort, `-1m` → context1m)
/// from the tail in any order. Non-Claude ids pass through unchanged.
fn decompose_claude_id(id: &str) -> (String, Option<String>, bool) {
    if !id.starts_with("claude-") {
        return (id.to_string(), None, false);
    }
    let mut rest = id.to_string();
    let mut effort: Option<String> = None;
    let mut context1m = false;
    for _ in 0..2 {
        let Some(dash) = rest.rfind('-') else { break };
        let suffix = &rest[dash + 1..];
        if suffix == "1m" && !context1m {
            context1m = true;
            rest.truncate(dash);
        } else if (suffix == "high" || suffix == "xhigh") && effort.is_none() {
            effort = Some(suffix.to_string());
            rest.truncate(dash);
        } else {
            break;
        }
    }
    (rest, effort, context1m)
}

pub fn write_claude_config(
    base_url: &str,
    api_key: &str,
    model: Option<&str>,
    small_model: Option<&str>,
) -> Result<(), AppError> {
    let path = claude_settings_path();

    // ANTHROPIC_MODEL must be the bare base id; any -xhigh / -1m suffix is an
    // internal selection signal the relay applies per-request on the wire
    // (see proxy_server), not user-facing config. Small model is written
    // as-is (it never needs modifiers).
    let big_base: Option<String> = model.map(|m| decompose_claude_id(m).0);

    // Read existing settings or start fresh
    let mut settings: Value = if path.exists() {
        let content = fs::read_to_string(&path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Build env block
    let env = settings
        .as_object_mut()
        .unwrap()
        .entry("env")
        .or_insert_with(|| serde_json::json!({}));

    // ANTHROPIC_AUTH_TOKEN policy: the relay does NOT manage this secret. If
    // the user already set it (any value), leave it alone — Claude Code reads
    // it before invoking the proxy and the relay rewrites x-api-key on the
    // wire anyway. If it's missing, drop in a harmless placeholder so Claude
    // Code's "token must be set" preflight passes; the placeholder never
    // reaches upstream.
    let token_already_present = env
        .as_object()
        .and_then(|o| o.get("ANTHROPIC_AUTH_TOKEN"))
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let token_to_write: Option<&str> = if token_already_present {
        None
    } else {
        Some(api_key)
    };

    // Check if the current config already matches
    let needs_update = if let Some(env_obj) = env.as_object() {
        let token_match = match token_to_write {
            Some(t) => env_obj.get("ANTHROPIC_AUTH_TOKEN").and_then(|v| v.as_str()) == Some(t),
            None => true,
        };
        !(token_match
            && env_obj.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str()) == Some(base_url)
            && (big_base.is_none()
                || env_obj.get("ANTHROPIC_MODEL").and_then(|v| v.as_str()) == big_base.as_deref())
            && (small_model.is_none()
                || env_obj.get("ANTHROPIC_SMALL_FAST_MODEL").and_then(|v| v.as_str()) == small_model))
    } else {
        true
    };

    if !needs_update {
        // Config already correct, skip write
        return Ok(());
    }

    if let Some(env_obj) = env.as_object_mut() {
        if let Some(t) = token_to_write {
            env_obj.insert(
                "ANTHROPIC_AUTH_TOKEN".to_string(),
                Value::String(t.to_string()),
            );
        }
        env_obj.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            Value::String(base_url.to_string()),
        );
        if let Some(b) = big_base.as_deref() {
            env_obj.insert("ANTHROPIC_MODEL".to_string(), Value::String(b.to_string()));
        }
        if let Some(m) = small_model {
            env_obj.insert(
                "ANTHROPIC_SMALL_FAST_MODEL".to_string(),
                Value::String(m.to_string()),
            );
        }
        // ANTHROPIC_CUSTOM_HEADERS is left untouched on purpose. If the user
        // already configured it (advanced setup), the relay must not clobber
        // their override; if they didn't, we don't introduce an unfamiliar
        // var — the relay injects the equivalent headers on the wire when the
        // active model has -xhigh / -1m suffixes (see proxy_server).
    }

    let json_str = serde_json::to_string_pretty(&settings)?;
    atomic_write(&path, json_str.as_bytes())
}

pub fn read_claude_config() -> Result<Option<Value>, AppError> {
    let path = claude_settings_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)?;
    let val: Value = serde_json::from_str(&content)?;
    Ok(Some(val))
}

pub fn clear_claude_config() -> Result<(), AppError> {
    let path = claude_settings_path();
    if !path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(&path)?;
    let mut settings: Value =
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));

    if let Some(env) = settings.get_mut("env").and_then(|v| v.as_object_mut()) {
        // Only clear vars the relay actually owns. ANTHROPIC_AUTH_TOKEN and
        // ANTHROPIC_CUSTOM_HEADERS are user-managed (or only written as a
        // placeholder when missing) — leaving them lets people unhook the
        // relay without losing pre-existing config.
        env.remove("ANTHROPIC_BASE_URL");
        env.remove("ANTHROPIC_MODEL");
        env.remove("ANTHROPIC_SMALL_FAST_MODEL");
    }

    let json_str = serde_json::to_string_pretty(&settings)?;
    atomic_write(&path, json_str.as_bytes())
}

// ─── Codex CLI ───
// ~/.codex/config.toml + ~/.codex/auth.json

fn codex_dir() -> PathBuf {
    home_dir().join(".codex")
}

pub fn write_codex_config(
    base_url: &str,
    api_key: &str,
    model: Option<&str>,
) -> Result<(), AppError> {
    let dir = codex_dir();
    fs::create_dir_all(&dir)?;

    // Write auth.json only if needed
    let auth_path = dir.join("auth.json");
    let needs_auth_update = if auth_path.exists() {
        if let Ok(content) = fs::read_to_string(&auth_path) {
            if let Ok(existing) = serde_json::from_str::<Value>(&content) {
                existing.get("OPENAI_API_KEY").and_then(|v| v.as_str()) != Some(api_key)
            } else {
                true
            }
        } else {
            true
        }
    } else {
        true
    };

    if needs_auth_update {
        let auth = serde_json::json!({
            "OPENAI_API_KEY": api_key
        });
        let auth_str = serde_json::to_string_pretty(&auth)?;
        atomic_write(&auth_path, auth_str.as_bytes())?;
    }

    // Read or create config.toml, update model + model_provider + base_url
    let config_path = dir.join("config.toml");
    let mut doc: DocumentMut = if config_path.exists() {
        let content = fs::read_to_string(&config_path)?;
        content
            .parse::<DocumentMut>()
            .unwrap_or_else(|_| "".parse::<DocumentMut>().unwrap())
    } else {
        "".parse::<DocumentMut>().unwrap()
    };

    // Check if config needs update
    let url_with_slash = if base_url.ends_with('/') {
        base_url.to_string()
    } else {
        format!("{base_url}/")
    };

    let needs_config_update = doc.get("model_provider").and_then(|v| v.as_str()) != Some("copilot_gateway")
        || (model.is_some() && doc.get("model").and_then(|v| v.as_str()) != model)
        || doc.get("model_providers")
            .and_then(|mp| mp.get("copilot_gateway"))
            .and_then(|gw| gw.get("base_url"))
            .and_then(|u| u.as_str()) != Some(&url_with_slash);

    if !needs_config_update {
        // Config already correct, skip write
        return Ok(());
    }

    if let Some(m) = model {
        doc["model"] = toml_edit::value(m);
    }
    doc["model_provider"] = toml_edit::value("copilot_gateway");

    // Ensure [model_providers.copilot_gateway] section
    if doc.get("model_providers").is_none() {
        doc["model_providers"] = toml_edit::table();
    }
    if let Some(providers) = doc["model_providers"].as_table_mut() {
        if !providers.contains_key("copilot_gateway") {
            providers["copilot_gateway"] = toml_edit::table();
        }
        if let Some(gw) = providers["copilot_gateway"].as_table_mut() {
            gw["name"] = toml_edit::value("Copilot Gateway");
            gw["base_url"] = toml_edit::value(&url_with_slash);
            gw["env_key"] = toml_edit::value("OPENAI_API_KEY");
            gw["wire_api"] = toml_edit::value("responses");
        }
    }

    let toml_str = doc.to_string();
    atomic_write(&config_path, toml_str.as_bytes())
}

pub fn read_codex_config() -> Result<(Option<Value>, Option<String>), AppError> {
    let dir = codex_dir();

    let auth: Option<Value> = {
        let path = dir.join("auth.json");
        if path.exists() {
            let content = fs::read_to_string(&path)?;
            Some(serde_json::from_str(&content)?)
        } else {
            None
        }
    };

    let config: Option<String> = {
        let path = dir.join("config.toml");
        if path.exists() {
            Some(fs::read_to_string(&path)?)
        } else {
            None
        }
    };

    Ok((auth, config))
}

pub fn clear_codex_config() -> Result<(), AppError> {
    let dir = codex_dir();

    // Remove auth.json key
    let auth_path = dir.join("auth.json");
    if auth_path.exists() {
        let content = fs::read_to_string(&auth_path)?;
        let mut auth: Value =
            serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(obj) = auth.as_object_mut() {
            obj.remove("OPENAI_API_KEY");
        }
        let auth_str = serde_json::to_string_pretty(&auth)?;
        atomic_write(&auth_path, auth_str.as_bytes())?;
    }

    // Remove copilot_gateway base_url from config.toml
    let config_path = dir.join("config.toml");
    if config_path.exists() {
        let content = fs::read_to_string(&config_path)?;
        if let Ok(mut doc) = content.parse::<DocumentMut>() {
            if let Some(providers) = doc
                .get_mut("model_providers")
                .and_then(|v| v.as_table_mut())
            {
                if let Some(gw) = providers
                    .get_mut("copilot_gateway")
                    .and_then(|v| v.as_table_mut())
                {
                    gw.remove("base_url");
                }
            }
            let toml_str = doc.to_string();
            atomic_write(&config_path, toml_str.as_bytes())?;
        }
    }

    Ok(())
}

// ─── Gemini CLI ───
// ~/.gemini/.env

fn gemini_dir() -> PathBuf {
    home_dir().join(".gemini")
}

pub(crate) fn parse_env_file(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_string();
            let value = value.trim().to_string();
            if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                map.insert(key, value);
            }
        }
    }
    map
}

pub(crate) fn serialize_env_file(map: &HashMap<String, String>) -> String {
    let mut keys: Vec<_> = map.keys().collect();
    keys.sort();
    keys.iter()
        .filter_map(|k| map.get(*k).map(|v| format!("{k}={v}")))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn write_gemini_config(base_url: &str, api_key: &str) -> Result<(), AppError> {
    let dir = gemini_dir();
    fs::create_dir_all(&dir)?;

    let env_path = dir.join(".env");

    // Read existing or start fresh
    let mut env_map = if env_path.exists() {
        let content = fs::read_to_string(&env_path)?;
        parse_env_file(&content)
    } else {
        HashMap::new()
    };

    // Check if update needed. Write both the legacy (GEMINI_API_BASE_URL) and
    // the new (GOOGLE_GEMINI_BASE_URL) variable names so older and newer Gemini
    // CLI versions both pick up the relay base URL.
    let needs_update = env_map.get("GEMINI_API_KEY") != Some(&api_key.to_string())
        || env_map.get("GOOGLE_GEMINI_BASE_URL") != Some(&base_url.to_string())
        || env_map.get("GEMINI_API_BASE_URL") != Some(&base_url.to_string());

    if !needs_update {
        // Config already correct, skip write
        return Ok(());
    }

    env_map.insert("GEMINI_API_KEY".to_string(), api_key.to_string());
    env_map.insert("GOOGLE_GEMINI_BASE_URL".to_string(), base_url.to_string());
    env_map.insert("GEMINI_API_BASE_URL".to_string(), base_url.to_string());

    let content = serialize_env_file(&env_map);
    atomic_write(&env_path, content.as_bytes())?;

    // Also write settings.json to set auth type to gemini-api-key
    let settings_path = dir.join("settings.json");
    let mut settings: Value = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if let Some(obj) = settings.as_object_mut() {
        let security = obj
            .entry("security")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(sec_obj) = security.as_object_mut() {
            let auth = sec_obj
                .entry("auth")
                .or_insert_with(|| serde_json::json!({}));
            if let Some(auth_obj) = auth.as_object_mut() {
                auth_obj.insert(
                    "selectedType".to_string(),
                    Value::String("gemini-api-key".to_string()),
                );
            }
        }
    }

    let settings_str = serde_json::to_string_pretty(&settings)?;
    atomic_write(&settings_path, settings_str.as_bytes())
}

pub fn read_gemini_config() -> Result<Option<HashMap<String, String>>, AppError> {
    let env_path = gemini_dir().join(".env");
    if !env_path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&env_path)?;
    Ok(Some(parse_env_file(&content)))
}

pub fn clear_gemini_config() -> Result<(), AppError> {
    let env_path = gemini_dir().join(".env");
    if !env_path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(&env_path)?;
    let mut env_map = parse_env_file(&content);
    env_map.remove("GEMINI_API_KEY");
    env_map.remove("GOOGLE_GEMINI_BASE_URL");
    env_map.remove("GEMINI_API_BASE_URL");

    let content = serialize_env_file(&env_map);
    atomic_write(&env_path, content.as_bytes())
}

/// Write all three CLI configs pointing to the provided base_url/api_key.
pub fn apply_all_configs(
    base_url: &str,
    api_key: &str,
    claude_model: Option<&str>,
    claude_small_model: Option<&str>,
    codex_model: Option<&str>,
    _gemini_model: Option<&str>,
) -> Result<(), AppError> {
    // Capture user's pre-relay state on the very first apply so that
    // clear_all_configs can restore the original values. Snapshot file
    // existing == "relay is active"; we never overwrite an existing one
    // (otherwise the second apply would snapshot the relay's own values).
    capture_snapshot_if_absent()?;

    write_claude_config(base_url, api_key, claude_model, claude_small_model)?;
    write_codex_config(base_url, api_key, codex_model)?;
    write_gemini_config(base_url, api_key)?;
    ensure_openai_api_key_in_shell_rc()?;
    Ok(())
}

/// Clear all three CLI configs.
///
/// If a pre-apply snapshot exists, restore each captured field to its original
/// value (re-inserting when the relay overwrote it, deleting when the relay
/// introduced it) and remove the snapshot. Otherwise fall back to the legacy
/// behavior of stripping only relay-written keys.
pub fn clear_all_configs() -> Result<(), AppError> {
    if let Some(snap) = read_snapshot()? {
        restore_claude(&snap.claude)?;
        restore_codex(&snap.codex)?;
        restore_gemini(&snap.gemini)?;
        delete_snapshot()?;
    } else {
        clear_claude_config()?;
        clear_codex_config()?;
        clear_gemini_config()?;
    }
    Ok(())
}

/// Ensure OPENAI_API_KEY is set in the user's environment.
/// Codex CLI requires this env var to exist; we set a dummy placeholder
/// since the local proxy injects the real key.
fn ensure_openai_api_key_in_shell_rc() -> Result<(), AppError> {
    // Set in current process so child processes inherit immediately
    if std::env::var("OPENAI_API_KEY").is_err() {
        std::env::set_var("OPENAI_API_KEY", "dummy");
    }

    #[cfg(target_os = "macos")]
    {
        // launchctl setenv makes it available to GUI apps (Finder, Spotlight launches)
        let _ = std::process::Command::new("launchctl")
            .args(["setenv", "OPENAI_API_KEY", "dummy"])
            .output();

        // Also write to shell rc for terminal sessions
        let home = home_dir();
        let rc_path = if home.join(".zshrc").exists() {
            home.join(".zshrc")
        } else {
            home.join(".bashrc")
        };

        let marker = "export OPENAI_API_KEY=";
        let content = if rc_path.exists() {
            fs::read_to_string(&rc_path)?
        } else {
            String::new()
        };

        if !content.lines().any(|line| {
            let trimmed = line.trim();
            trimmed.starts_with(marker) && !trimmed.starts_with('#')
        }) {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&rc_path)?;
            writeln!(f)?;
            writeln!(f, "# Added by LLM Relay for Codex CLI compatibility")?;
            writeln!(f, "export OPENAI_API_KEY=dummy")?;
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let home = home_dir();
        let rc_path = if home.join(".zshrc").exists() {
            home.join(".zshrc")
        } else {
            home.join(".bashrc")
        };

        let marker = "export OPENAI_API_KEY=";
        let content = if rc_path.exists() {
            fs::read_to_string(&rc_path)?
        } else {
            String::new()
        };

        if !content.lines().any(|line| {
            let trimmed = line.trim();
            trimmed.starts_with(marker) && !trimmed.starts_with('#')
        }) {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&rc_path)?;
            writeln!(f)?;
            writeln!(f, "# Added by LLM Relay for Codex CLI compatibility")?;
            writeln!(f, "export OPENAI_API_KEY=dummy")?;
        }
    }

    #[cfg(windows)]
    {
        // On Windows, set a persistent user environment variable via the registry
        use std::process::Command;
        // Check if already set in user env via `reg query`
        let check = Command::new("reg")
            .args(["query", "HKCU\\Environment", "/v", "OPENAI_API_KEY"])
            .output();
        if let Ok(output) = check {
            if output.status.success() {
                // Already set, skip
                return Ok(());
            }
        }
        // Set via setx (persists across reboots, user-level, writes to HKCU\Environment)
        let result = Command::new("setx")
            .args(["OPENAI_API_KEY", "dummy"])
            .output()
            .map_err(|e| AppError::Config(format!("Failed to run setx: {e}")))?;
        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            log::warn!("setx OPENAI_API_KEY failed: {stderr}");
        } else {
            // Also set in current process so child processes inherit it immediately
            std::env::set_var("OPENAI_API_KEY", "dummy");
            // Broadcast WM_SETTINGCHANGE so Explorer and other apps pick up the change
            broadcast_env_change();
        }
    }

    Ok(())
}

/// Broadcast WM_SETTINGCHANGE after modifying environment variables,
/// so Explorer and other running apps pick up the change without restart.
#[cfg(windows)]
fn broadcast_env_change() {
    use std::ffi::CString;
    use std::ptr;

    // HWND_BROADCAST = 0xFFFF, WM_SETTINGCHANGE = 0x001A
    extern "system" {
        fn SendMessageTimeoutA(
            hwnd: *mut std::ffi::c_void,
            msg: u32,
            wparam: usize,
            lparam: *const i8,
            flags: u32,
            timeout: u32,
            result: *mut usize,
        ) -> isize;
    }

    let env = CString::new("Environment").unwrap();
    unsafe {
        // SMTO_ABORTIFHUNG = 0x0002, timeout 5000ms
        SendMessageTimeoutA(
            0xFFFF as *mut _,
            0x001A,
            0,
            env.as_ptr(),
            0x0002,
            5000,
            ptr::null_mut(),
        );
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentCliConfig {
    pub claude: Option<serde_json::Value>,
    pub codex_auth: Option<serde_json::Value>,
    pub codex_config: Option<String>,
    pub gemini: Option<HashMap<String, String>>,
}

pub fn read_all_configs() -> Result<CurrentCliConfig, AppError> {
    let claude = read_claude_config()?;
    let (codex_auth, codex_config) = read_codex_config()?;
    let gemini = read_gemini_config()?;

    Ok(CurrentCliConfig {
        claude,
        codex_auth,
        codex_config,
        gemini,
    })
}

// ─── Backend-typed variants ───
//
// New CliBackend-aware writers/clearers used by `apply_to_targets`. They
// accept any `&dyn CliBackend` and an explicit `base_url`, so a single
// codepath serves Windows + every selected WSL distro. The legacy
// path-based functions above still exist as direct callers used during
// the rollout; once `apply_to_targets` replaces `apply_all_configs`,
// they can be removed.
//
// The logic is a deliberate copy-and-adapt of the path-based versions
// — same JSON/TOML manipulation, no behavioral drift. Diffing against
// the legacy fns is the right way to spot any unintentional change.

use crate::cli_target::CliBackend;

pub fn write_claude_config_with(
    backend: &dyn CliBackend,
    base_url: &str,
    api_key: &str,
    model: Option<&str>,
    small_model: Option<&str>,
) -> Result<(), AppError> {
    let rel: &[&str] = &[".claude", "settings.json"];
    let big_base: Option<String> = model.map(|m| decompose_claude_id(m).0);

    let existing = backend.read(rel)?;
    let mut settings: Value = match existing.as_deref() {
        Some(s) => serde_json::from_str(s).unwrap_or_else(|_| serde_json::json!({})),
        None => serde_json::json!({}),
    };

    let env = settings
        .as_object_mut()
        .unwrap()
        .entry("env")
        .or_insert_with(|| serde_json::json!({}));

    let token_already_present = env
        .as_object()
        .and_then(|o| o.get("ANTHROPIC_AUTH_TOKEN"))
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let token_to_write: Option<&str> = if token_already_present { None } else { Some(api_key) };

    let needs_update = if let Some(env_obj) = env.as_object() {
        let token_match = match token_to_write {
            Some(t) => env_obj.get("ANTHROPIC_AUTH_TOKEN").and_then(|v| v.as_str()) == Some(t),
            None => true,
        };
        !(token_match
            && env_obj.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str()) == Some(base_url)
            && (big_base.is_none()
                || env_obj.get("ANTHROPIC_MODEL").and_then(|v| v.as_str()) == big_base.as_deref())
            && (small_model.is_none()
                || env_obj.get("ANTHROPIC_SMALL_FAST_MODEL").and_then(|v| v.as_str()) == small_model))
    } else {
        true
    };

    if !needs_update {
        return Ok(());
    }

    if let Some(env_obj) = env.as_object_mut() {
        if let Some(t) = token_to_write {
            env_obj.insert("ANTHROPIC_AUTH_TOKEN".to_string(), Value::String(t.to_string()));
        }
        env_obj.insert("ANTHROPIC_BASE_URL".to_string(), Value::String(base_url.to_string()));
        if let Some(b) = big_base.as_deref() {
            env_obj.insert("ANTHROPIC_MODEL".to_string(), Value::String(b.to_string()));
        }
        if let Some(m) = small_model {
            env_obj.insert("ANTHROPIC_SMALL_FAST_MODEL".to_string(), Value::String(m.to_string()));
        }
    }

    let json_str = serde_json::to_string_pretty(&settings)?;
    backend.write_atomic(rel, json_str.as_bytes())
}

pub fn clear_claude_config_with(backend: &dyn CliBackend) -> Result<(), AppError> {
    let rel: &[&str] = &[".claude", "settings.json"];
    let Some(content) = backend.read(rel)? else { return Ok(()); };
    let mut settings: Value = serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(env) = settings.get_mut("env").and_then(|v| v.as_object_mut()) {
        env.remove("ANTHROPIC_BASE_URL");
        env.remove("ANTHROPIC_MODEL");
        env.remove("ANTHROPIC_SMALL_FAST_MODEL");
    }
    backend.write_atomic(rel, serde_json::to_string_pretty(&settings)?.as_bytes())
}

pub fn write_codex_config_with(
    backend: &dyn CliBackend,
    base_url: &str,
    api_key: &str,
    model: Option<&str>,
) -> Result<(), AppError> {
    let auth_rel: &[&str] = &[".codex", "auth.json"];
    let cfg_rel: &[&str] = &[".codex", "config.toml"];

    // auth.json
    let existing_auth = backend.read(auth_rel)?;
    let needs_auth_update = match existing_auth.as_deref() {
        Some(s) => match serde_json::from_str::<Value>(s) {
            Ok(v) => v.get("OPENAI_API_KEY").and_then(|x| x.as_str()) != Some(api_key),
            Err(_) => true,
        },
        None => true,
    };
    if needs_auth_update {
        let auth = serde_json::json!({ "OPENAI_API_KEY": api_key });
        backend.write_atomic(auth_rel, serde_json::to_string_pretty(&auth)?.as_bytes())?;
    }

    // config.toml
    let url_with_slash = if base_url.ends_with('/') {
        base_url.to_string()
    } else {
        format!("{base_url}/")
    };
    let existing_cfg = backend.read(cfg_rel)?;
    let mut doc: DocumentMut = match existing_cfg.as_deref() {
        Some(s) => s.parse::<DocumentMut>().unwrap_or_else(|_| "".parse().unwrap()),
        None => "".parse().unwrap(),
    };

    let needs_cfg_update = doc.get("model_provider").and_then(|v| v.as_str()) != Some("copilot_gateway")
        || (model.is_some() && doc.get("model").and_then(|v| v.as_str()) != model)
        || doc
            .get("model_providers")
            .and_then(|mp| mp.get("copilot_gateway"))
            .and_then(|gw| gw.get("base_url"))
            .and_then(|u| u.as_str())
            != Some(&url_with_slash);

    if !needs_cfg_update {
        return Ok(());
    }

    if let Some(m) = model {
        doc["model"] = toml_edit::value(m);
    }
    doc["model_provider"] = toml_edit::value("copilot_gateway");
    if doc.get("model_providers").is_none() {
        doc["model_providers"] = toml_edit::table();
    }
    if let Some(providers) = doc["model_providers"].as_table_mut() {
        if !providers.contains_key("copilot_gateway") {
            providers["copilot_gateway"] = toml_edit::table();
        }
        if let Some(gw) = providers["copilot_gateway"].as_table_mut() {
            gw["name"] = toml_edit::value("Copilot Gateway");
            gw["base_url"] = toml_edit::value(&url_with_slash);
            gw["env_key"] = toml_edit::value("OPENAI_API_KEY");
            gw["wire_api"] = toml_edit::value("responses");
        }
    }
    backend.write_atomic(cfg_rel, doc.to_string().as_bytes())
}

pub fn clear_codex_config_with(backend: &dyn CliBackend) -> Result<(), AppError> {
    let auth_rel: &[&str] = &[".codex", "auth.json"];
    let cfg_rel: &[&str] = &[".codex", "config.toml"];

    if let Some(content) = backend.read(auth_rel)? {
        let mut auth: Value = serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(obj) = auth.as_object_mut() {
            obj.remove("OPENAI_API_KEY");
        }
        backend.write_atomic(auth_rel, serde_json::to_string_pretty(&auth)?.as_bytes())?;
    }
    if let Some(content) = backend.read(cfg_rel)? {
        if let Ok(mut doc) = content.parse::<DocumentMut>() {
            if let Some(providers) = doc.get_mut("model_providers").and_then(|v| v.as_table_mut()) {
                if let Some(gw) = providers.get_mut("copilot_gateway").and_then(|v| v.as_table_mut()) {
                    gw.remove("base_url");
                }
            }
            backend.write_atomic(cfg_rel, doc.to_string().as_bytes())?;
        }
    }
    Ok(())
}

pub fn write_gemini_config_with(
    backend: &dyn CliBackend,
    base_url: &str,
    api_key: &str,
) -> Result<(), AppError> {
    let env_rel: &[&str] = &[".gemini", ".env"];
    let settings_rel: &[&str] = &[".gemini", "settings.json"];

    let mut env_map = match backend.read(env_rel)? {
        Some(s) => parse_env_file(&s),
        None => HashMap::new(),
    };
    let needs_update = env_map.get("GEMINI_API_KEY") != Some(&api_key.to_string())
        || env_map.get("GOOGLE_GEMINI_BASE_URL") != Some(&base_url.to_string())
        || env_map.get("GEMINI_API_BASE_URL") != Some(&base_url.to_string());
    if needs_update {
        env_map.insert("GEMINI_API_KEY".into(), api_key.into());
        env_map.insert("GOOGLE_GEMINI_BASE_URL".into(), base_url.into());
        env_map.insert("GEMINI_API_BASE_URL".into(), base_url.into());
        backend.write_atomic(env_rel, serialize_env_file(&env_map).as_bytes())?;
    }

    let mut settings: Value = match backend.read(settings_rel)? {
        Some(s) => serde_json::from_str(&s).unwrap_or_else(|_| serde_json::json!({})),
        None => serde_json::json!({}),
    };
    if let Some(obj) = settings.as_object_mut() {
        let security = obj.entry("security").or_insert_with(|| serde_json::json!({}));
        if let Some(sec_obj) = security.as_object_mut() {
            let auth = sec_obj.entry("auth").or_insert_with(|| serde_json::json!({}));
            if let Some(auth_obj) = auth.as_object_mut() {
                auth_obj.insert("selectedType".into(), Value::String("gemini-api-key".into()));
            }
        }
    }
    backend.write_atomic(settings_rel, serde_json::to_string_pretty(&settings)?.as_bytes())
}

pub fn clear_gemini_config_with(backend: &dyn CliBackend) -> Result<(), AppError> {
    let env_rel: &[&str] = &[".gemini", ".env"];
    let Some(content) = backend.read(env_rel)? else { return Ok(()); };
    let mut env_map = parse_env_file(&content);
    env_map.remove("GEMINI_API_KEY");
    env_map.remove("GOOGLE_GEMINI_BASE_URL");
    env_map.remove("GEMINI_API_BASE_URL");
    backend.write_atomic(env_rel, serialize_env_file(&env_map).as_bytes())
}

#[cfg(test)]
mod backend_tests {
    use super::*;
    use crate::cli_target::WindowsFsBackend;
    use tempfile::TempDir;

    fn fake_home(tmp: &TempDir) -> WindowsFsBackend {
        WindowsFsBackend { home: tmp.path().to_path_buf() }
    }

    #[test]
    fn claude_round_trip_via_backend() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        write_claude_config_with(&b, "http://127.0.0.1:18080", "kk", Some("claude-sonnet-4-6"), None).unwrap();
        let content = b.read(&[".claude", "settings.json"]).unwrap().unwrap();
        assert!(content.contains("ANTHROPIC_BASE_URL"));
        assert!(content.contains("http://127.0.0.1:18080"));
        clear_claude_config_with(&b).unwrap();
        let content = b.read(&[".claude", "settings.json"]).unwrap().unwrap();
        assert!(!content.contains("ANTHROPIC_BASE_URL"));
    }

    #[test]
    fn gemini_round_trip_via_backend() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        write_gemini_config_with(&b, "http://host.docker.internal:18080", "kk").unwrap();
        let env = b.read(&[".gemini", ".env"]).unwrap().unwrap();
        assert!(env.contains("GEMINI_API_BASE_URL=http://host.docker.internal:18080"));
        clear_gemini_config_with(&b).unwrap();
        let env = b.read(&[".gemini", ".env"]).unwrap().unwrap();
        assert!(!env.contains("GEMINI_API_BASE_URL"));
    }
}
