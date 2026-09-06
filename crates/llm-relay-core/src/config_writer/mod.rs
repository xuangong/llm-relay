use crate::AppError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use toml_edit::DocumentMut;

pub mod lifecycle;
pub mod snapshot;

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

// ─── Per-CLI snapshot structs ───
// Used by the per-target snapshot machinery in `snapshot.rs`. The legacy
// single-file capture/restore helpers were removed in followup #5; the
// only remaining writer/reader of these structs is `snapshot::*`.

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSnapshot {
    /// `null` ↔ the key was not present in env. Some("") is a real empty string.
    pub anthropic_base_url: Option<String>,
    pub anthropic_model: Option<String>,
    pub anthropic_small_fast_model: Option<String>,
    pub anthropic_auth_token: Option<String>,
    #[serde(default)]
    pub claude_code_subagent_model: Option<String>,
    #[serde(default)]
    pub anthropic_default_haiku_model: Option<String>,
    #[serde(default)]
    pub anthropic_custom_headers: Option<String>,
    /// False identifies snapshots written before the three fields above were captured.
    #[serde(default)]
    pub extended_fields_captured: bool,
    /// Original values for every environment key ever managed by an Extra config.
    /// `None` means the key did not exist before relay management began.
    #[serde(default)]
    pub extra_env_originals: BTreeMap<String, Option<String>>,
    #[serde(default)]
    pub extra_env_captured: bool,
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
    #[serde(default)]
    pub model_reasoning_effort: Option<String>,
    /// False identifies snapshots written before main-model effort was captured.
    #[serde(default)]
    pub special_fields_captured: bool,
    #[serde(default)]
    pub default_subagent_model: Option<String>,
    #[serde(default)]
    pub default_subagent_reasoning_effort: Option<String>,
    /// Independent marker because older development snapshots captured only main effort.
    #[serde(default)]
    pub codex_subagent_defaults_captured: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiSnapshot {
    pub gemini_api_key: Option<String>,
    pub google_gemini_base_url: Option<String>,
    pub gemini_api_base_url: Option<String>,
    pub selected_auth_type: Option<String>,
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
    let file_name = path
        .file_name()
        .ok_or_else(|| AppError::Config("Invalid file name".to_string()))?
        .to_string_lossy();
    let mut tmp = path
        .parent()
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
    if !id
        .get(.."claude-".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("claude-"))
    {
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

fn is_gpt_model(id: &str) -> bool {
    crate::model_id::without_context_suffix(id)
        .rsplit('/')
        .next()
        .unwrap_or(id)
        .to_ascii_lowercase()
        .starts_with("gpt")
}

/// Translate a saved gateway model id into the value Claude Code reads.
/// GPT models advertise their extended context through a model-name suffix.
fn claude_settings_model_id(id: &str, is_main: bool) -> String {
    if is_main {
        crate::model_id::normalize_claude_main_model(id)
    } else {
        id.to_string()
    }
}

/// Build the three role values written to Claude Code. Normally Claude composite
/// ids are reduced to their bare base and the relay supplies their modifiers on
/// the wire. When two roles select different variants of the same base, however,
/// the bare ids would be indistinguishable in the request body. Keep those
/// composite ids so the proxy can resolve each role exactly.
fn claude_settings_role_ids(roles: [Option<&str>; 3]) -> [Option<String>; 3] {
    let conflicting = |index: usize, id: &str| {
        let (id, bracket_context) = crate::model_id::split_context_suffix(id);
        if !id
            .get(.."claude-".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("claude-"))
        {
            return false;
        }
        let (base, effort, context1m) = decompose_claude_id(id);
        roles.iter().enumerate().any(|(other_index, other)| {
            if other_index == index {
                return false;
            }
            let Some(other) = other else { return false };
            let (other, other_bracket_context) = crate::model_id::split_context_suffix(other);
            if !other
                .get(.."claude-".len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("claude-"))
            {
                return false;
            }
            let (other_base, other_effort, other_composite_context) = decompose_claude_id(other);
            let context1m = context1m || bracket_context;
            let other_context1m = other_composite_context || other_bracket_context;
            base.eq_ignore_ascii_case(&other_base)
                && (effort != other_effort || context1m != other_context1m)
        })
    };

    std::array::from_fn(|index| {
        roles[index].map(|id| {
            let (id, _) = crate::model_id::split_context_suffix(id);
            let normalized = if conflicting(index, id) {
                id.to_string()
            } else if id
                .get(.."claude-".len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("claude-"))
            {
                decompose_claude_id(id).0
            } else {
                id.to_string()
            };
            if index == 0 {
                claude_settings_model_id(&normalized, true)
            } else {
                normalized
            }
        })
    })
}

fn write_claude_env(
    settings: &mut Value,
    base_url: &str,
    api_key: &str,
    model: Option<&str>,
    subagent_model: Option<&str>,
    haiku_model: Option<&str>,
    extra_env: Option<&BTreeMap<String, String>>,
    snapshot: Option<&ClaudeSnapshot>,
) -> Result<bool, AppError> {
    let original = settings.clone();
    let [model, subagent_model, haiku_model] =
        claude_settings_role_ids([model, subagent_model, haiku_model]);
    let remove_custom_headers = [
        model.as_deref(),
        subagent_model.as_deref(),
        haiku_model.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(is_gpt_model);

    let settings_obj = settings
        .as_object_mut()
        .ok_or_else(|| AppError::Config("~/.claude/settings.json must be a JSON object".into()))?;
    let env = settings_obj
        .entry("env")
        .or_insert_with(|| serde_json::json!({}));
    let env_obj = env.as_object_mut().ok_or_else(|| {
        AppError::Config("~/.claude/settings.json env must be a JSON object".into())
    })?;

    if let Some(snapshot) = snapshot.filter(|snapshot| snapshot.extra_env_captured) {
        for (key, original) in &snapshot.extra_env_originals {
            if extra_env.is_some_and(|extra| extra.contains_key(key)) {
                continue;
            }
            match original {
                Some(value) => {
                    env_obj.insert(key.clone(), Value::String(value.clone()));
                }
                None => {
                    env_obj.remove(key);
                }
            }
        }
    }
    if let Some(extra_env) = extra_env {
        for (key, value) in extra_env {
            env_obj.insert(key.clone(), Value::String(value.clone()));
        }
    }

    let token_already_present = env_obj
        .get("ANTHROPIC_AUTH_TOKEN")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());
    let token_to_write = (!token_already_present).then_some(api_key);
    let matches = |key: &str, desired: Option<&str>| match desired {
        Some(value) => env_obj.get(key).and_then(|v| v.as_str()) == Some(value),
        None => !env_obj.contains_key(key),
    };
    let core_matches = token_to_write.is_none_or(|token| {
        env_obj.get("ANTHROPIC_AUTH_TOKEN").and_then(|v| v.as_str()) == Some(token)
    }) && env_obj.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str())
        == Some(base_url)
        && matches("ANTHROPIC_MODEL", model.as_deref())
        && matches("CLAUDE_CODE_SUBAGENT_MODEL", subagent_model.as_deref())
        && matches("ANTHROPIC_DEFAULT_HAIKU_MODEL", haiku_model.as_deref())
        && (haiku_model.is_none() || !env_obj.contains_key("ANTHROPIC_SMALL_FAST_MODEL"))
        && (!remove_custom_headers || !env_obj.contains_key("ANTHROPIC_CUSTOM_HEADERS"));

    if let Some(token) = token_to_write {
        env_obj.insert("ANTHROPIC_AUTH_TOKEN".into(), Value::String(token.into()));
    }
    env_obj.insert("ANTHROPIC_BASE_URL".into(), Value::String(base_url.into()));
    let mut apply = |key: &str, value: Option<String>| match value {
        Some(value) => {
            env_obj.insert(key.into(), Value::String(value));
        }
        None => {
            env_obj.remove(key);
        }
    };
    apply("ANTHROPIC_MODEL", model);
    apply("CLAUDE_CODE_SUBAGENT_MODEL", subagent_model);
    let replace_legacy_small_model = haiku_model.is_some();
    apply("ANTHROPIC_DEFAULT_HAIKU_MODEL", haiku_model);
    if replace_legacy_small_model {
        env_obj.remove("ANTHROPIC_SMALL_FAST_MODEL");
    }
    if remove_custom_headers {
        env_obj.remove("ANTHROPIC_CUSTOM_HEADERS");
    }
    Ok(!core_matches || *settings != original)
}

pub fn write_claude_config(
    base_url: &str,
    api_key: &str,
    model: Option<&str>,
    subagent_model: Option<&str>,
    haiku_model: Option<&str>,
) -> Result<(), AppError> {
    let path = claude_settings_path();

    // Read existing settings or start fresh
    let mut settings: Value = if path.exists() {
        let content = fs::read_to_string(&path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if !write_claude_env(
        &mut settings,
        base_url,
        api_key,
        model,
        subagent_model,
        haiku_model,
        None,
        None,
    )? {
        return Ok(());
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
        env.remove("CLAUDE_CODE_SUBAGENT_MODEL");
        env.remove("ANTHROPIC_DEFAULT_HAIKU_MODEL");
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

/// Return the base model that Codex should rewrite to a selected fast variant.
/// Matching is deliberately case-sensitive: model ids are opaque, and only the
/// gateway's canonical `gpt-…-fast` shape opts into this behavior.
fn codex_fast_base(model: &str) -> Option<&str> {
    model
        .starts_with("gpt-")
        .then(|| model.strip_suffix("-fast"))
        .flatten()
        .filter(|base| base.len() > "gpt-".len())
}

fn table_item_to_table(item: &toml_edit::Item) -> Option<toml_edit::Table> {
    item.clone().into_table().ok()
}

/// Normalize an expanded or inline table to an editable expanded table without
/// dropping any existing entries. Non-table values are replaced, matching the
/// writer's historical recovery behavior for malformed config.
fn ensure_table_item(item: &mut toml_edit::Item) -> &mut toml_edit::Table {
    if item.as_table().is_none() {
        let existing = std::mem::take(item);
        *item = match existing.into_table() {
            Ok(table) => toml_edit::Item::Table(table),
            Err(_) => toml_edit::table(),
        };
    }
    item.as_table_mut().expect("item was normalized to a table")
}

/// Restore the main-model effort displaced by Relay's fast-model default.
fn restore_codex_main_effort(doc: &mut DocumentMut, snapshot: Option<&CodexSnapshot>) {
    if doc
        .get("model_reasoning_effort")
        .and_then(|value| value.as_str())
        != Some("high")
    {
        return;
    }
    match snapshot.and_then(|snap| snap.model_reasoning_effort.as_deref()) {
        Some(effort) => doc["model_reasoning_effort"] = toml_edit::value(effort),
        None => {
            doc.as_table_mut().remove("model_reasoning_effort");
        }
    }
}

/// Apply all relay-owned Codex TOML values and report whether the document changed.
fn update_codex_document(
    doc: &mut DocumentMut,
    base_url: &str,
    model: Option<&str>,
    subagent_model: Option<&str>,
    snapshot: Option<&CodexSnapshot>,
) -> bool {
    let before = doc.to_string();
    restore_codex_main_effort(doc, snapshot);

    match model {
        Some(model) => {
            doc["model"] = toml_edit::value(model);
            if codex_fast_base(model).is_some() {
                doc["model_reasoning_effort"] = toml_edit::value("high");
            }
        }
        None => {
            doc.as_table_mut().remove("model");
        }
    }

    match subagent_model {
        Some(model) => {
            doc["default_subagent_model"] = toml_edit::value(model);
            doc["default_subagent_reasoning_effort"] = toml_edit::value("high");
        }
        None => {
            doc.as_table_mut().remove("default_subagent_model");
            doc.as_table_mut()
                .remove("default_subagent_reasoning_effort");
        }
    }

    doc["model_provider"] = toml_edit::value("copilot_gateway");
    let providers = ensure_table_item(&mut doc["model_providers"]);
    let gateway = ensure_table_item(&mut providers["copilot_gateway"]);
    let url_with_slash = if base_url.ends_with('/') {
        base_url.to_string()
    } else {
        format!("{base_url}/")
    };
    gateway["name"] = toml_edit::value("Copilot Gateway");
    gateway["base_url"] = toml_edit::value(url_with_slash);
    gateway["env_key"] = toml_edit::value("OPENAI_API_KEY");
    gateway["wire_api"] = toml_edit::value("responses");

    before != doc.to_string()
}

fn clear_codex_document(doc: &mut DocumentMut) {
    doc.as_table_mut().remove("model_reasoning_effort");
    doc.as_table_mut().remove("default_subagent_model");
    doc.as_table_mut()
        .remove("default_subagent_reasoning_effort");
    if let Some(gateway) = doc
        .get_mut("model_providers")
        .and_then(|item| item.as_table_like_mut())
        .and_then(|providers| providers.get_mut("copilot_gateway"))
        .and_then(|item| item.as_table_like_mut())
    {
        gateway.remove("base_url");
    }
}

pub fn write_codex_config(
    base_url: &str,
    api_key: &str,
    model: Option<&str>,
    subagent_model: Option<&str>,
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

    if !update_codex_document(&mut doc, base_url, model, subagent_model, None) {
        return Ok(());
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
            clear_codex_document(&mut doc);
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

/// The value written into `OPENAI_API_KEY`.
///
/// Codex CLI refuses to start without the variable set, but the real key never
/// comes from here — the local proxy swaps it on the wire. The value is named
/// rather than left as `dummy` so anyone who finds it in their shell profile can
/// tell where it came from and that it is meant to be disregarded.
pub const OPENAI_API_KEY_PLACEHOLDER: &str = "llm-relay-ignore";

/// Which login shell the user actually runs, which decides both the file to
/// write and the syntax to write in. Guessing from whichever rc file happens to
/// exist gets this wrong for anyone who has both `.zshrc` and `.bashrc`.
///
/// Not `cfg(unix)`-gated: a Windows host still writes rc files inside its WSL
/// distros, where the shell is whatever that distro's passwd entry says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellKind {
    Zsh,
    Bash,
    Fish,
    /// Unrecognised (ksh, dash, a login shell we've never heard of). `.profile`
    /// with `export` is the portable common denominator.
    Posix,
}

fn detect_shell(shell_path: Option<&str>) -> ShellKind {
    // Match on the file name so /bin/zsh, /usr/bin/zsh and a homebrew zsh all
    // land in the same bucket.
    let name = shell_path
        .and_then(|p| p.rsplit('/').next())
        .unwrap_or_default();
    match name {
        "zsh" => ShellKind::Zsh,
        "bash" => ShellKind::Bash,
        "fish" => ShellKind::Fish,
        _ => ShellKind::Posix,
    }
}

impl ShellKind {
    fn rc_relative_path(self) -> Vec<&'static str> {
        match self {
            ShellKind::Zsh => vec![".zshrc"],
            ShellKind::Fish => vec![".config", "fish", "config.fish"],
            // Terminal.app and iTerm start bash as a *login* shell, which reads
            // .bash_profile and never .bashrc; most Linux terminals do the
            // opposite. Writing to the wrong one leaves the var unset. Keyed off
            // the build target rather than the write target, which is fine
            // because WSL only ever exists on a Windows host.
            #[cfg(target_os = "macos")]
            ShellKind::Bash => vec![".bash_profile"],
            #[cfg(not(target_os = "macos"))]
            ShellKind::Bash => vec![".bashrc"],
            ShellKind::Posix => vec![".profile"],
        }
    }

    fn export_line(self, value: &str) -> String {
        match self {
            // fish is not POSIX: `export FOO=bar` is a syntax error there.
            ShellKind::Fish => format!("set -gx OPENAI_API_KEY {value}"),
            _ => format!("export OPENAI_API_KEY={value}"),
        }
    }

    fn assignment_prefixes(self) -> &'static [&'static str] {
        match self {
            ShellKind::Fish => &["set -gx OPENAI_API_KEY", "set -x OPENAI_API_KEY"],
            _ => &["export OPENAI_API_KEY=", "OPENAI_API_KEY="],
        }
    }
}

/// Values this module considers its own, and is therefore free to overwrite.
/// `dummy` is what releases up to v0.4.0 wrote; without it in this list every
/// existing install would be stranded on the old value forever.
const RELAY_OWNED_OPENAI_VALUES: &[&str] = &["dummy", OPENAI_API_KEY_PLACEHOLDER];

/// Whether `OPENAI_API_KEY=<value>` is something we wrote, as opposed to a real
/// OpenAI key the user set themselves. Quotes are stripped first, since both
/// `export FOO="x"` and `export FOO=x` are common.
fn is_relay_owned_openai_value(value: &str) -> bool {
    let v = value.trim().trim_matches(['"', '\'']);
    RELAY_OWNED_OPENAI_VALUES.contains(&v)
}

/// The value assigned by an `OPENAI_API_KEY` line, as written.
fn assigned_openai_value(line: &str, shell: ShellKind) -> Option<&str> {
    let trimmed = line.trim();
    match shell {
        // `set -gx OPENAI_API_KEY <value>`
        ShellKind::Fish => trimmed.split_whitespace().nth(3),
        // `export OPENAI_API_KEY=<value>` / `OPENAI_API_KEY=<value>`
        _ => trimmed.split_once('=').map(|(_, v)| v),
    }
}

/// What `rc_with_openai_key` decided to do.
enum RcOutcome {
    /// The file already assigns the value we want.
    AlreadySet,
    /// The file assigns a value we did not write — a real key. Left untouched:
    /// Codex works either way, and clobbering the user's key to install a
    /// placeholder would be a bad trade.
    ForeignValue,
    Rewrite(String),
}

/// Rewrite an rc file so `OPENAI_API_KEY` ends up at `value`.
///
/// An existing *relay-written* assignment is replaced in place rather than
/// shadowed by a new line at the bottom. Appending would work for a fresh
/// install but not for an upgrade, where the old line still assigns the previous
/// placeholder and a later line silently wins — leaving the file
/// self-contradictory to read.
fn rc_with_openai_key(existing: &str, shell: ShellKind, value: &str) -> RcOutcome {
    let desired = shell.export_line(value);
    let prefixes = shell.assignment_prefixes();

    let mut out: Vec<String> = Vec::new();
    let mut outcome: Option<RcOutcome> = None;

    for line in existing.lines() {
        let trimmed = line.trim();
        let is_assignment =
            !trimmed.starts_with('#') && prefixes.iter().any(|p| trimmed.starts_with(p));
        if is_assignment && outcome.is_none() {
            if trimmed == desired {
                outcome = Some(RcOutcome::AlreadySet);
                out.push(line.to_string());
            } else if assigned_openai_value(trimmed, shell).is_some_and(is_relay_owned_openai_value)
            {
                outcome = Some(RcOutcome::Rewrite(String::new())); // placeholder, filled below
                out.push(desired.clone());
            } else {
                return RcOutcome::ForeignValue;
            }
        } else {
            out.push(line.to_string());
        }
    }

    match outcome {
        Some(RcOutcome::AlreadySet) => return RcOutcome::AlreadySet,
        Some(_) => {}
        None => {
            if !out.is_empty() {
                out.push(String::new());
            }
            out.push("# Added by LLM Relay for Codex CLI compatibility".to_string());
            out.push(desired);
        }
    }

    let mut text = out.join("\n");
    text.push('\n');
    RcOutcome::Rewrite(text)
}

/// Put the `OPENAI_API_KEY` line into the rc file of `shell`, under whatever
/// home directory `backend` points at — the local host's, or a WSL distro's.
///
/// Skips the write when the file already carries the right assignment: both
/// backends rewrite the whole file, and the user's shell may be reading it.
fn ensure_openai_api_key_in_rc(backend: &dyn CliBackend, shell: ShellKind) -> Result<(), AppError> {
    let rel = shell.rc_relative_path();
    let existing = backend.read(&rel)?.unwrap_or_default();
    match rc_with_openai_key(&existing, shell, OPENAI_API_KEY_PLACEHOLDER) {
        RcOutcome::AlreadySet => Ok(()),
        RcOutcome::ForeignValue => {
            log::info!(
                "~/{} already sets OPENAI_API_KEY to a value we did not write — leaving it alone",
                rel.join("/")
            );
            Ok(())
        }
        RcOutcome::Rewrite(updated) => {
            // Both backends mkdir the parent, which fish's ~/.config/fish needs.
            backend.write_atomic(&rel, updated.as_bytes())?;
            log::info!("wrote OPENAI_API_KEY to ~/{}", rel.join("/"));
            Ok(())
        }
    }
}

/// Ensure `OPENAI_API_KEY` is set in the user's environment, because Codex CLI
/// will not start without it.
///
/// Windows gets a user-level variable (`HKCU\Environment` via `setx`); Unix gets
/// a line in the rc file belonging to the shell the user actually logs into.
///
/// Host-only. WSL distros are handled per-target in `write_one_target`, since
/// each has its own home directory and its own login shell.
pub(crate) fn ensure_openai_api_key_env() -> Result<(), AppError> {
    // Set in this process too, so anything we spawn inherits it without waiting
    // for a new login session — unless the user already has a real key in the
    // environment, in which case theirs wins here as well.
    let inherited = std::env::var("OPENAI_API_KEY").ok();
    let ours = inherited.as_deref().is_none_or(is_relay_owned_openai_value);
    if ours {
        std::env::set_var("OPENAI_API_KEY", OPENAI_API_KEY_PLACEHOLDER);
    }

    #[cfg(target_os = "macos")]
    if ours {
        // launchctl covers GUI-launched processes, which never read a shell rc.
        let _ = std::process::Command::new("launchctl")
            .args(["setenv", "OPENAI_API_KEY", OPENAI_API_KEY_PLACEHOLDER])
            .output();
    }

    #[cfg(unix)]
    {
        let shell = detect_shell(std::env::var("SHELL").ok().as_deref());
        ensure_openai_api_key_in_rc(&crate::cli_target::WindowsFsBackend::new(), shell)?;
    }

    #[cfg(windows)]
    {
        use std::process::Command;
        // `reg query` prints "    OPENAI_API_KEY    REG_SZ    <value>".
        let current = Command::new("reg")
            .args(["query", "HKCU\\Environment", "/v", "OPENAI_API_KEY"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        let stored: Option<String> = current
            .lines()
            .find(|l| l.contains("OPENAI_API_KEY"))
            .and_then(|l| l.split_whitespace().nth(2))
            .map(|v| v.to_string());

        match stored.as_deref() {
            // Already ours and already right.
            Some(v) if v == OPENAI_API_KEY_PLACEHOLDER => {}
            // A real key the user set. Codex works with it, so don't clobber it.
            Some(v) if !is_relay_owned_openai_value(v) => {
                log::info!(
                    "HKCU\\Environment already sets OPENAI_API_KEY to a value we did not write — leaving it alone"
                );
            }
            // Absent, or an older release's `dummy`.
            _ => {
                let result = Command::new("setx")
                    .args(["OPENAI_API_KEY", OPENAI_API_KEY_PLACEHOLDER])
                    .output()
                    .map_err(|e| AppError::Config(format!("Failed to run setx: {e}")))?;
                if !result.status.success() {
                    let stderr = String::from_utf8_lossy(&result.stderr);
                    log::warn!("setx OPENAI_API_KEY failed: {stderr}");
                } else {
                    // Tell Explorer and friends, so new terminals see it without
                    // a logout.
                    broadcast_env_change();
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod openai_env_tests {
    use super::*;

    const V: &str = OPENAI_API_KEY_PLACEHOLDER;

    /// The rewritten file, or a panic naming what happened instead.
    fn rewritten(existing: &str, shell: ShellKind) -> String {
        match rc_with_openai_key(existing, shell, V) {
            RcOutcome::Rewrite(s) => s,
            RcOutcome::AlreadySet => panic!("expected a rewrite, got AlreadySet"),
            RcOutcome::ForeignValue => panic!("expected a rewrite, got ForeignValue"),
        }
    }

    #[test]
    fn shell_is_read_from_the_binary_name_not_the_path() {
        assert_eq!(detect_shell(Some("/bin/zsh")), ShellKind::Zsh);
        assert_eq!(detect_shell(Some("/opt/homebrew/bin/zsh")), ShellKind::Zsh);
        assert_eq!(detect_shell(Some("/bin/bash")), ShellKind::Bash);
        assert_eq!(detect_shell(Some("/usr/bin/fish")), ShellKind::Fish);
        // Anything we don't know gets the portable treatment rather than a guess.
        assert_eq!(detect_shell(Some("/bin/ksh")), ShellKind::Posix);
        assert_eq!(detect_shell(None), ShellKind::Posix);
    }

    #[test]
    fn each_shell_gets_its_own_rc_file() {
        assert_eq!(ShellKind::Zsh.rc_relative_path(), vec![".zshrc"]);
        assert_eq!(ShellKind::Posix.rc_relative_path(), vec![".profile"]);
        assert_eq!(
            ShellKind::Fish.rc_relative_path(),
            vec![".config", "fish", "config.fish"]
        );
        #[cfg(target_os = "macos")]
        assert_eq!(ShellKind::Bash.rc_relative_path(), vec![".bash_profile"]);
        #[cfg(not(target_os = "macos"))]
        assert_eq!(ShellKind::Bash.rc_relative_path(), vec![".bashrc"]);
    }

    #[test]
    fn a_fresh_file_gets_the_export_appended_with_a_marker() {
        let out = rewritten("", ShellKind::Zsh);
        assert_eq!(
            out,
            "# Added by LLM Relay for Codex CLI compatibility\nexport OPENAI_API_KEY=llm-relay-ignore\n"
        );
    }

    #[test]
    fn existing_content_is_kept_and_separated_by_a_blank_line() {
        let out = rewritten("export PATH=/x:$PATH", ShellKind::Zsh);
        assert_eq!(
            out,
            "export PATH=/x:$PATH\n\n# Added by LLM Relay for Codex CLI compatibility\nexport OPENAI_API_KEY=llm-relay-ignore\n"
        );
    }

    #[test]
    fn an_older_value_is_replaced_in_place_not_shadowed() {
        // The upgrade case: v0.4.0 and earlier wrote `dummy`. Appending a second
        // line would leave the file saying two different things.
        let rc = "export OPENAI_API_KEY=dummy\nexport EDITOR=vim\n";
        let out = rewritten(rc, ShellKind::Zsh);
        assert_eq!(
            out,
            "export OPENAI_API_KEY=llm-relay-ignore\nexport EDITOR=vim\n"
        );
        assert!(!out.contains("dummy"));
    }

    #[test]
    fn a_real_key_the_user_set_is_never_overwritten() {
        // Codex runs fine with the user's own key — the proxy replaces it on the
        // wire either way — so there is nothing to gain by destroying it.
        for rc in [
            "export OPENAI_API_KEY=sk-proj-realkey\n",
            "export OPENAI_API_KEY=\"sk-proj-realkey\"\n",
            "OPENAI_API_KEY=sk-proj-realkey\n",
        ] {
            assert!(
                matches!(
                    rc_with_openai_key(rc, ShellKind::Zsh, V),
                    RcOutcome::ForeignValue
                ),
                "should have left {rc:?} alone"
            );
        }
    }

    #[test]
    fn quoted_relay_values_are_still_recognised_as_ours() {
        let out = rewritten("export OPENAI_API_KEY=\"dummy\"\n", ShellKind::Zsh);
        assert_eq!(out, "export OPENAI_API_KEY=llm-relay-ignore\n");
    }

    #[test]
    fn a_file_that_already_says_the_right_thing_is_not_rewritten() {
        let rc = "export OPENAI_API_KEY=llm-relay-ignore\n";
        assert!(matches!(
            rc_with_openai_key(rc, ShellKind::Zsh, V),
            RcOutcome::AlreadySet
        ));
    }

    #[test]
    fn commented_out_assignments_are_not_mistaken_for_real_ones() {
        let rc = "# export OPENAI_API_KEY=old\n";
        let out = rewritten(rc, ShellKind::Zsh);
        assert!(out.contains("# export OPENAI_API_KEY=old"));
        assert!(out.contains("\nexport OPENAI_API_KEY=llm-relay-ignore\n"));
    }

    #[test]
    fn fish_gets_fish_syntax_because_export_is_a_syntax_error_there() {
        let out = rewritten("", ShellKind::Fish);
        assert!(out.contains("set -gx OPENAI_API_KEY llm-relay-ignore"));
        assert!(!out.contains("export"));
        // …and it recognises its own prior line rather than appending a second.
        assert!(matches!(
            rc_with_openai_key(&out, ShellKind::Fish, V),
            RcOutcome::AlreadySet
        ));
    }

    #[test]
    fn fish_also_leaves_a_real_key_alone() {
        assert!(matches!(
            rc_with_openai_key("set -gx OPENAI_API_KEY sk-real\n", ShellKind::Fish, V),
            RcOutcome::ForeignValue
        ));
    }

    #[test]
    fn writes_land_in_the_backends_home() {
        use crate::cli_target::WindowsFsBackend;
        let tmp = tempfile::TempDir::new().unwrap();
        let b = WindowsFsBackend {
            home: tmp.path().to_path_buf(),
        };
        ensure_openai_api_key_in_rc(&b, ShellKind::Fish).unwrap();
        // fish's config dir does not exist beforehand; the backend must create it.
        let written = b
            .read(&[".config", "fish", "config.fish"])
            .unwrap()
            .unwrap();
        assert!(written.contains("set -gx OPENAI_API_KEY llm-relay-ignore"));
    }
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
    subagent_model: Option<&str>,
    haiku_model: Option<&str>,
) -> Result<(), AppError> {
    write_claude_config_with_extra(
        backend,
        base_url,
        api_key,
        model,
        subagent_model,
        haiku_model,
        None,
        None,
    )
}

fn write_claude_config_with_extra(
    backend: &dyn CliBackend,
    base_url: &str,
    api_key: &str,
    model: Option<&str>,
    subagent_model: Option<&str>,
    haiku_model: Option<&str>,
    extra_env: Option<&BTreeMap<String, String>>,
    snapshot: Option<&ClaudeSnapshot>,
) -> Result<(), AppError> {
    let rel: &[&str] = &[".claude", "settings.json"];
    let existing = backend.read(rel)?;
    let mut settings: Value = match existing.as_deref() {
        Some(s) => serde_json::from_str(s).unwrap_or_else(|_| serde_json::json!({})),
        None => serde_json::json!({}),
    };

    if !write_claude_env(
        &mut settings,
        base_url,
        api_key,
        model,
        subagent_model,
        haiku_model,
        extra_env,
        snapshot,
    )? {
        return Ok(());
    }

    let json_str = serde_json::to_string_pretty(&settings)?;
    backend.write_atomic(rel, json_str.as_bytes())
}

/// Ensure `~/.claude.json` carries `"hasCompletedOnboarding": true`.
///
/// Separate file from `~/.claude/settings.json`: this one is Claude Code's own
/// state (onboarding, projects, MCP servers), and without the flag it opens the
/// first-run wizard instead of honouring the relay's just-written endpoint.
///
/// Two deliberate asymmetries with the rest of this module:
///
/// * A malformed file is left alone rather than replaced with `{}` — everywhere
///   else that fallback costs a few env vars we are about to rewrite anyway,
///   whereas here it would discard the user's Claude Code state. Better to skip
///   the flag and say so in the log.
/// * Nothing here is snapshotted or undone on disable. The flag only records
///   that onboarding happened, which stays true afterwards; restoring it to
///   absent would push the user back through the wizard for no reason.
///
/// Writes only when the flag is actually missing or false. Claude Code rewrites
/// this file constantly, so a no-op write is a chance to clobber whatever it
/// stored between our read and our write.
pub fn ensure_claude_onboarded_with(backend: &dyn CliBackend) -> Result<(), AppError> {
    let rel: &[&str] = &[".claude.json"];

    let mut settings: Value = match backend.read(rel)? {
        Some(content) => match serde_json::from_str(&content) {
            Ok(v @ Value::Object(_)) => v,
            Ok(other) => {
                log::warn!(
                    "~/.claude.json is {}, not an object — leaving it alone",
                    match other {
                        Value::Array(_) => "an array",
                        Value::Null => "null",
                        _ => "a scalar",
                    }
                );
                return Ok(());
            }
            Err(e) => {
                log::warn!("~/.claude.json is not valid JSON ({e}) — leaving it alone");
                return Ok(());
            }
        },
        None => serde_json::json!({}),
    };

    if settings.get("hasCompletedOnboarding") == Some(&Value::Bool(true)) {
        return Ok(());
    }

    settings
        .as_object_mut()
        .expect("checked to be an object above")
        .insert("hasCompletedOnboarding".to_string(), Value::Bool(true));

    backend.write_atomic(rel, serde_json::to_string_pretty(&settings)?.as_bytes())
}

pub fn clear_claude_config_with(backend: &dyn CliBackend) -> Result<(), AppError> {
    let rel: &[&str] = &[".claude", "settings.json"];
    let Some(content) = backend.read(rel)? else {
        return Ok(());
    };
    let mut settings: Value =
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(env) = settings.get_mut("env").and_then(|v| v.as_object_mut()) {
        env.remove("ANTHROPIC_BASE_URL");
        env.remove("ANTHROPIC_MODEL");
        env.remove("CLAUDE_CODE_SUBAGENT_MODEL");
        env.remove("ANTHROPIC_DEFAULT_HAIKU_MODEL");
        env.remove("ANTHROPIC_SMALL_FAST_MODEL");
    }
    backend.write_atomic(rel, serde_json::to_string_pretty(&settings)?.as_bytes())
}

pub fn write_codex_config_with(
    backend: &dyn CliBackend,
    base_url: &str,
    api_key: &str,
    model: Option<&str>,
    subagent_model: Option<&str>,
) -> Result<(), AppError> {
    write_codex_config_with_snapshot(backend, base_url, api_key, model, subagent_model, None)
}

fn write_codex_config_with_snapshot(
    backend: &dyn CliBackend,
    base_url: &str,
    api_key: &str,
    model: Option<&str>,
    subagent_model: Option<&str>,
    snapshot: Option<&CodexSnapshot>,
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
    let existing_cfg = backend.read(cfg_rel)?;
    let mut doc: DocumentMut = match existing_cfg.as_deref() {
        Some(s) => s
            .parse::<DocumentMut>()
            .unwrap_or_else(|_| "".parse().unwrap()),
        None => "".parse().unwrap(),
    };

    if !update_codex_document(&mut doc, base_url, model, subagent_model, snapshot) {
        return Ok(());
    }
    backend.write_atomic(cfg_rel, doc.to_string().as_bytes())
}

pub fn clear_codex_config_with(backend: &dyn CliBackend) -> Result<(), AppError> {
    let auth_rel: &[&str] = &[".codex", "auth.json"];
    let cfg_rel: &[&str] = &[".codex", "config.toml"];

    if let Some(content) = backend.read(auth_rel)? {
        let mut auth: Value =
            serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(obj) = auth.as_object_mut() {
            obj.remove("OPENAI_API_KEY");
        }
        backend.write_atomic(auth_rel, serde_json::to_string_pretty(&auth)?.as_bytes())?;
    }
    if let Some(content) = backend.read(cfg_rel)? {
        if let Ok(mut doc) = content.parse::<DocumentMut>() {
            clear_codex_document(&mut doc);
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
        let security = obj
            .entry("security")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(sec_obj) = security.as_object_mut() {
            let auth = sec_obj
                .entry("auth")
                .or_insert_with(|| serde_json::json!({}));
            if let Some(auth_obj) = auth.as_object_mut() {
                auth_obj.insert(
                    "selectedType".into(),
                    Value::String("gemini-api-key".into()),
                );
            }
        }
    }
    backend.write_atomic(
        settings_rel,
        serde_json::to_string_pretty(&settings)?.as_bytes(),
    )
}

pub fn clear_gemini_config_with(backend: &dyn CliBackend) -> Result<(), AppError> {
    let env_rel: &[&str] = &[".gemini", ".env"];
    let Some(content) = backend.read(env_rel)? else {
        return Ok(());
    };
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
        WindowsFsBackend {
            home: tmp.path().to_path_buf(),
        }
    }

    #[test]
    fn claude_round_trip_via_backend() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        write_claude_config_with(
            &b,
            "http://127.0.0.1:18080",
            "kk",
            Some("claude-sonnet-4-6"),
            None,
            None,
        )
        .unwrap();
        let content = b.read(&[".claude", "settings.json"]).unwrap().unwrap();
        assert!(content.contains("ANTHROPIC_BASE_URL"));
        assert!(content.contains("http://127.0.0.1:18080"));
        clear_claude_config_with(&b).unwrap();
        let content = b.read(&[".claude", "settings.json"]).unwrap().unwrap();
        assert!(!content.contains("ANTHROPIC_BASE_URL"));
    }

    fn claude_env(b: &WindowsFsBackend) -> serde_json::Map<String, Value> {
        let content = b.read(&[".claude", "settings.json"]).unwrap().unwrap();
        serde_json::from_str::<Value>(&content).unwrap()["env"]
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn writes_all_three_claude_models_and_normalizes_gpt_context() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        write_claude_config_with(
            &b,
            "http://relay",
            "kk",
            Some("GPT-5.6-sol-fast"),
            Some("gpt-5.6-agent[1m]"),
            Some("claude-haiku-4-5"),
        )
        .unwrap();

        let env = claude_env(&b);
        assert_eq!(env["ANTHROPIC_MODEL"], "GPT-5.6-sol-fast[1m]");
        assert_eq!(env["CLAUDE_CODE_SUBAGENT_MODEL"], "gpt-5.6-agent");
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "claude-haiku-4-5");

        write_claude_config_with(
            &b,
            "http://relay",
            "kk",
            Some("vendor/gpt-5.6-sol-fast"),
            None,
            None,
        )
        .unwrap();
        let env = claude_env(&b);
        assert_eq!(env["ANTHROPIC_MODEL"], "vendor/gpt-5.6-sol-fast[1m]");

        write_claude_config_with(
            &b,
            "http://relay",
            "kk",
            Some("claude-opus-4.7-xhigh-1m"),
            Some("claude-sonnet-4.7-high-1m"),
            Some("claude-haiku-4.5-1m"),
        )
        .unwrap();
        let env = claude_env(&b);
        assert_eq!(env["ANTHROPIC_MODEL"], "claude-opus-4.7[1m]");
        assert_eq!(env["CLAUDE_CODE_SUBAGENT_MODEL"], "claude-sonnet-4.7");
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "claude-haiku-4.5");
    }

    #[test]
    fn same_base_different_claude_variants_remain_role_distinguishable() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        write_claude_config_with(
            &b,
            "http://relay",
            "kk",
            Some("claude-opus-4.7-xhigh"),
            Some("claude-opus-4.7-high"),
            Some("claude-opus-4.7-1m"),
        )
        .unwrap();

        let env = claude_env(&b);
        assert_eq!(env["ANTHROPIC_MODEL"], "claude-opus-4.7-xhigh[1m]");
        assert_eq!(env["CLAUDE_CODE_SUBAGENT_MODEL"], "claude-opus-4.7-high");
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "claude-opus-4.7-1m");
    }

    #[test]
    fn extra_env_switch_restores_original_values_and_keeps_unrelated_keys() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        b.write_atomic(
            &[".claude", "settings.json"],
            br#"{"env":{"OLD":"original","OTHER":"safe"}}"#,
        )
        .unwrap();
        let snapshot = ClaudeSnapshot {
            extra_env_originals: BTreeMap::from([
                ("OLD".to_string(), Some("original".to_string())),
                ("NEW".to_string(), None),
            ]),
            extra_env_captured: true,
            ..Default::default()
        };
        let first = BTreeMap::from([
            ("OLD".to_string(), "managed".to_string()),
            ("NEW".to_string(), "1".to_string()),
        ]);
        write_claude_config_with_extra(
            &b,
            "http://relay",
            "kk",
            Some("gpt-5.6-sol"),
            Some("gpt-5.6-terra"),
            Some("gpt-5.6-luna"),
            Some(&first),
            Some(&snapshot),
        )
        .unwrap();
        let env = claude_env(&b);
        assert_eq!(env["OLD"], "managed");
        assert_eq!(env["NEW"], "1");

        write_claude_config_with_extra(
            &b,
            "http://relay",
            "kk",
            Some("gpt-5.6-sol"),
            Some("gpt-5.6-terra"),
            Some("gpt-5.6-luna"),
            None,
            Some(&snapshot),
        )
        .unwrap();
        let env = claude_env(&b);
        assert_eq!(env["OLD"], "original");
        assert!(!env.contains_key("NEW"));
        assert_eq!(env["OTHER"], "safe");
        assert_eq!(env["ANTHROPIC_MODEL"], "gpt-5.6-sol[1m]");
        assert_eq!(env["CLAUDE_CODE_SUBAGENT_MODEL"], "gpt-5.6-terra");
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "gpt-5.6-luna");
    }

    #[test]
    fn gpt_apply_removes_stale_keys_and_preserves_unrelated_env() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        b.write_atomic(
            &[".claude", "settings.json"],
            br#"{"env":{"ANTHROPIC_BASE_URL":"http://relay","ANTHROPIC_AUTH_TOKEN":"keep","ANTHROPIC_MODEL":"gpt-5.6[1m]","CLAUDE_CODE_SUBAGENT_MODEL":"claude-sonnet","ANTHROPIC_DEFAULT_HAIKU_MODEL":"claude-haiku","ANTHROPIC_SMALL_FAST_MODEL":"old","ANTHROPIC_CUSTOM_HEADERS":"stale","OTHER":"safe"}}"#,
        )
        .unwrap();

        write_claude_config_with(
            &b,
            "http://relay",
            "ignored",
            Some("gpt-5.6"),
            Some("claude-sonnet"),
            Some("claude-haiku"),
        )
        .unwrap();

        let env = claude_env(&b);
        assert!(!env.contains_key("ANTHROPIC_SMALL_FAST_MODEL"));
        assert!(!env.contains_key("ANTHROPIC_CUSTOM_HEADERS"));
        assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], "keep");
        assert_eq!(env["OTHER"], "safe");
    }

    #[test]
    fn claude_only_apply_preserves_custom_headers_and_decomposes_main_model() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        b.write_atomic(
            &[".claude", "settings.json"],
            br#"{"env":{"ANTHROPIC_CUSTOM_HEADERS":"user=value"}}"#,
        )
        .unwrap();

        write_claude_config_with(
            &b,
            "http://relay",
            "kk",
            Some("claude-opus-4.7-xhigh-1m"),
            Some("claude-sonnet-4.7"),
            Some("claude-haiku-4.5"),
        )
        .unwrap();

        let env = claude_env(&b);
        assert_eq!(env["ANTHROPIC_MODEL"], "claude-opus-4.7[1m]");
        assert_eq!(env["ANTHROPIC_CUSTOM_HEADERS"], "user=value");

        write_claude_config_with(
            &b,
            "http://relay",
            "kk",
            Some("CLAUDE-opus-4.7-xhigh-1m"),
            Some("claude-sonnet-4.7"),
            Some("claude-haiku-4.5"),
        )
        .unwrap();
        let env = claude_env(&b);
        assert_eq!(env["ANTHROPIC_MODEL"], "CLAUDE-opus-4.7[1m]");
    }

    #[test]
    fn missing_roles_clear_stale_keys_but_keep_legacy_small_without_replacement() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        b.write_atomic(
            &[".claude", "settings.json"],
            br#"{"env":{"ANTHROPIC_MODEL":"old-main","CLAUDE_CODE_SUBAGENT_MODEL":"old-subagent","ANTHROPIC_DEFAULT_HAIKU_MODEL":"old-haiku","ANTHROPIC_SMALL_FAST_MODEL":"legacy-small"}}"#,
        )
        .unwrap();

        write_claude_config_with(&b, "http://relay", "kk", None, None, None).unwrap();

        let env = claude_env(&b);
        assert!(!env.contains_key("ANTHROPIC_MODEL"));
        assert!(!env.contains_key("CLAUDE_CODE_SUBAGENT_MODEL"));
        assert!(!env.contains_key("ANTHROPIC_DEFAULT_HAIKU_MODEL"));
        assert_eq!(env["ANTHROPIC_SMALL_FAST_MODEL"], "legacy-small");
    }

    #[test]
    fn non_object_claude_env_returns_config_error_without_rewriting() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        let original = r#"{"env":null,"other":"keep"}"#;
        b.write_atomic(&[".claude", "settings.json"], original.as_bytes())
            .unwrap();

        let err = write_claude_config_with(
            &b,
            "http://relay",
            "kk",
            Some("claude-sonnet-4.7"),
            None,
            None,
        )
        .unwrap_err();

        assert!(err.to_string().contains("env must be a JSON object"));
        assert_eq!(
            b.read(&[".claude", "settings.json"]).unwrap().unwrap(),
            original
        );
    }

    fn codex_doc(b: &WindowsFsBackend) -> DocumentMut {
        b.read(&[".codex", "config.toml"])
            .unwrap()
            .unwrap()
            .parse()
            .unwrap()
    }

    #[test]
    fn codex_writes_main_and_subagent_defaults_as_top_level_keys() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);

        write_codex_config_with(
            &b,
            "http://relay",
            "kk",
            Some("gpt-5.6-sol-fast"),
            Some("gpt-5.6-mini-fast"),
        )
        .unwrap();
        let doc = codex_doc(&b);
        assert_eq!(doc["model"].as_str(), Some("gpt-5.6-sol-fast"));
        assert_eq!(doc["model_reasoning_effort"].as_str(), Some("high"));
        assert_eq!(
            doc["default_subagent_model"].as_str(),
            Some("gpt-5.6-mini-fast")
        );
        assert_eq!(
            doc["default_subagent_reasoning_effort"].as_str(),
            Some("high")
        );
        assert!(doc.get("agents").is_none());
    }

    #[test]
    fn codex_missing_models_clear_stale_top_level_values() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        b.write_atomic(
            &[".codex", "config.toml"],
            br#"model = "gpt-5.6-sol-fast"
model_reasoning_effort = "high"
default_subagent_model = "gpt-5.6-mini-fast"
default_subagent_reasoning_effort = "high"
"#,
        )
        .unwrap();

        write_codex_config_with(&b, "http://relay", "kk", None, None).unwrap();
        let doc = codex_doc(&b);
        assert!(doc.get("model").is_none());
        assert!(doc.get("model_reasoning_effort").is_none());
        assert!(doc.get("default_subagent_model").is_none());
        assert!(doc.get("default_subagent_reasoning_effort").is_none());
    }

    #[test]
    fn codex_non_fast_main_restores_snapshot_effort_and_preserves_inline_providers() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        let snapshot = CodexSnapshot {
            model_reasoning_effort: Some("xhigh".into()),
            special_fields_captured: true,
            ..CodexSnapshot::default()
        };
        b.write_atomic(
            &[".codex", "config.toml"],
            br#"model = "gpt-5.6-sol-fast"
model_reasoning_effort = "high"
model_providers = { other = { name = "Other", base_url = "https://other.example" }, copilot_gateway = { custom = "keep" } }
"#,
        )
        .unwrap();

        write_codex_config_with_snapshot(
            &b,
            "http://relay",
            "kk",
            Some("gpt-5.6-sol"),
            None,
            Some(&snapshot),
        )
        .unwrap();
        let doc = codex_doc(&b);
        assert_eq!(doc["model_reasoning_effort"].as_str(), Some("xhigh"));
        let providers = doc["model_providers"].as_table_like().unwrap();
        assert_eq!(
            providers
                .get("other")
                .and_then(|item| item.as_table_like())
                .and_then(|other| other.get("base_url"))
                .and_then(|value| value.as_str()),
            Some("https://other.example")
        );
        assert_eq!(
            providers
                .get("copilot_gateway")
                .and_then(|item| item.as_table_like())
                .and_then(|gateway| gateway.get("custom"))
                .and_then(|value| value.as_str()),
            Some("keep")
        );
    }

    #[test]
    fn codex_non_fast_main_does_not_enable_main_effort() {
        for model in ["GPT-5.6-sol-FAST", "gpt-5.6-sol-fast-extra", "gpt-5.6-sol"] {
            let tmp = TempDir::new().unwrap();
            let b = fake_home(&tmp);
            write_codex_config_with(&b, "http://relay", "kk", Some(model), None).unwrap();
            let doc = codex_doc(&b);
            assert!(doc.get("model_reasoning_effort").is_none(), "{model}");
        }
    }

    #[test]
    fn onboarding_flag_is_created_when_the_file_is_absent() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        ensure_claude_onboarded_with(&b).unwrap();
        let v: Value = serde_json::from_str(&b.read(&[".claude.json"]).unwrap().unwrap()).unwrap();
        assert_eq!(v["hasCompletedOnboarding"], Value::Bool(true));
    }

    #[test]
    fn onboarding_flag_is_added_without_disturbing_existing_state() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        b.write_atomic(
            &[".claude.json"],
            br#"{"numStartups":7,"mcpServers":{"x":{"command":"y"}}}"#,
        )
        .unwrap();

        ensure_claude_onboarded_with(&b).unwrap();

        let v: Value = serde_json::from_str(&b.read(&[".claude.json"]).unwrap().unwrap()).unwrap();
        assert_eq!(v["hasCompletedOnboarding"], Value::Bool(true));
        // The rest of Claude Code's state must survive — this file holds more
        // than the flag we care about.
        assert_eq!(v["numStartups"], 7);
        assert_eq!(v["mcpServers"]["x"]["command"], "y");
    }

    #[test]
    fn onboarding_flag_false_is_flipped_to_true() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        b.write_atomic(&[".claude.json"], br#"{"hasCompletedOnboarding":false}"#)
            .unwrap();
        ensure_claude_onboarded_with(&b).unwrap();
        let v: Value = serde_json::from_str(&b.read(&[".claude.json"]).unwrap().unwrap()).unwrap();
        assert_eq!(v["hasCompletedOnboarding"], Value::Bool(true));
    }

    #[test]
    fn already_onboarded_is_left_byte_for_byte_alone() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        // Deliberately compact and oddly ordered: if we rewrote it, pretty
        // printing would reorder/reformat and this comparison would fail.
        let original = r#"{"hasCompletedOnboarding":true,"numStartups":3}"#;
        b.write_atomic(&[".claude.json"], original.as_bytes())
            .unwrap();

        ensure_claude_onboarded_with(&b).unwrap();

        assert_eq!(
            b.read(&[".claude.json"]).unwrap().unwrap(),
            original,
            "a no-op write races Claude Code, which rewrites this file constantly"
        );
    }

    #[test]
    fn malformed_file_is_left_alone_rather_than_reset() {
        let tmp = TempDir::new().unwrap();
        let b = fake_home(&tmp);
        let broken = r#"{"numStartups": 7,,,"#;
        b.write_atomic(&[".claude.json"], broken.as_bytes())
            .unwrap();

        // Not an error: the endpoint config still applied, and clobbering the
        // user's Claude Code state would cost far more than the missing flag.
        ensure_claude_onboarded_with(&b).unwrap();

        assert_eq!(b.read(&[".claude.json"]).unwrap().unwrap(), broken);
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

// ─── Multi-target apply / clear ───
//
// `apply_to_targets` and `clear_targets_from_snapshots` are the new
// entry points used by `Service::set_active` / `Service::clear_active`.
// They iterate over a slice of `CliTarget` (Windows + each selected
// WSL distro) so each gets its own base_url + installed-tools mask
// + per-target snapshot.

pub fn shell_rc_paths(targets: &[crate::cli_target::CliTarget]) -> BTreeMap<String, Vec<String>> {
    let mut paths = BTreeMap::new();
    for target in targets.iter().filter(|target| target.installed.codex) {
        let shell = match target.snapshot_meta.target_type {
            crate::cli_target::TargetType::Windows => {
                detect_shell(std::env::var("SHELL").ok().as_deref())
            }
            crate::cli_target::TargetType::Wsl => detect_shell(
                target
                    .snapshot_meta
                    .distro_name
                    .as_deref()
                    .and_then(crate::wsl::distro::login_shell)
                    .as_deref(),
            ),
        };
        let key = match target.snapshot_meta.target_type {
            crate::cli_target::TargetType::Windows => "native".to_string(),
            crate::cli_target::TargetType::Wsl => format!(
                "wsl:{}",
                target.snapshot_meta.distro_name.as_deref().unwrap_or("")
            ),
        };
        paths.insert(
            key,
            shell
                .rc_relative_path()
                .into_iter()
                .map(str::to_string)
                .collect(),
        );
    }
    paths
}

#[derive(Debug, Default)]
pub struct ApplyReport {
    pub succeeded: std::collections::HashSet<String>,
    pub failed: BTreeMap<String, String>,
}

pub fn apply_to_targets(
    targets: &[crate::cli_target::CliTarget],
    retained_keys: Option<&std::collections::HashSet<String>>,
    api_key: &str,
    claude_model: Option<&str>,
    claude_subagent_model: Option<&str>,
    claude_haiku_model: Option<&str>,
    codex_model: Option<&str>,
    codex_subagent_model: Option<&str>,
    _gemini_model: Option<&str>,
    claude_extra_env: Option<&BTreeMap<String, String>>,
) -> Result<ApplyReport, AppError> {
    use crate::cli_target::TargetType;

    // Legacy field snapshots are retained only for compatibility diagnostics.
    // Full-file origin sidecars are the sole restoration authority.
    let prev_index = snapshot::build_index()?;
    let current_keys: std::collections::HashSet<String> =
        retained_keys.cloned().unwrap_or_else(|| {
            targets
                .iter()
                .map(|t| {
                    t.snapshot_meta
                        .distro_name
                        .clone()
                        .unwrap_or_else(|| "windows".to_string())
                })
                .collect()
        });

    // 1. Restore snapshots for targets removed since last apply. A failed
    // restore keeps its snapshot so a later apply can retry instead of losing
    // the only copy of the user's original config.
    let mut dropped_windows_failed = false;
    for (key, meta) in &prev_index {
        if current_keys.contains(key) {
            continue;
        }
        log::info!("dropping target {key} — restoring previous state");
        if let Some(snap) = snapshot::read(meta)? {
            let backend: Box<dyn CliBackend> = match meta.target_type {
                TargetType::Windows => Box::new(crate::cli_target::WindowsFsBackend::new()),
                TargetType::Wsl => Box::new(crate::cli_target::WslBackend {
                    distro: meta.distro_name.clone().unwrap_or_default(),
                    home: meta.home.clone().unwrap_or_default(),
                }),
            };
            if let Err(e) = snapshot::restore(&snap, &*backend) {
                log::warn!("restore failed for dropped target {key}: {e}");
                if matches!(meta.target_type, TargetType::Windows) {
                    dropped_windows_failed = true;
                }
                continue;
            }
            #[cfg(target_os = "windows")]
            if let TargetType::Wsl = meta.target_type {
                if let Some(distro) = meta.distro_name.as_deref() {
                    let hn = crate::wsl::hosts::relay_hostname();
                    if let Err(e) = crate::wsl::hosts::clear_hosts_entry(distro, &hn) {
                        log::warn!("clear hosts entry for dropped {distro}: {e}");
                        continue;
                    }
                }
            }
        }
        if let Err(e) = snapshot::delete(meta) {
            log::warn!("delete restored snapshot for {key} failed: {e}");
            if matches!(meta.target_type, TargetType::Windows) {
                dropped_windows_failed = true;
            }
        }
    }
    if dropped_windows_failed {
        return Err(AppError::Config(
            "apply: failed to restore removed Windows target".into(),
        ));
    }

    // 2. For each current target: capture snapshot if new, then write.
    let mut report = ApplyReport::default();
    let mut windows_failed = false;
    for target in targets {
        let key = target
            .snapshot_meta
            .distro_name
            .clone()
            .unwrap_or_else(|| "windows".to_string());
        let is_windows = matches!(target.snapshot_meta.target_type, TargetType::Windows);
        if !prev_index.contains_key(&key) {
            if let Err(e) = snapshot::capture(target) {
                log::warn!("snapshot capture failed for {key}: {e}");
                if is_windows {
                    windows_failed = true;
                }
                continue;
            }
        } else if let Err(e) = snapshot::backfill_extended_snapshot(target) {
            // An old snapshot cannot restore fields this release is about to
            // overwrite. Refuse this target rather than destroy user config.
            log::warn!("snapshot upgrade failed for {key}: {e}");
            if is_windows {
                windows_failed = true;
            }
            continue;
        }
        if let Err(e) = snapshot::capture_extra_env_originals(target, claude_extra_env) {
            log::warn!("Extra env snapshot upgrade failed for {key}: {e}");
            if is_windows {
                windows_failed = true;
            }
            continue;
        }
        let original_snapshot = snapshot::read(&target.snapshot_meta)?;
        match write_one_target(
            target,
            api_key,
            claude_model,
            claude_subagent_model,
            claude_haiku_model,
            codex_model,
            codex_subagent_model,
            claude_extra_env,
            original_snapshot.as_ref(),
        ) {
            Ok(()) => {
                report.succeeded.insert(key);
            }
            Err(e) => {
                log::warn!("apply failed for {key}: {e}");
                report.failed.insert(key, e.to_string());
                if is_windows {
                    windows_failed = true;
                }
            }
        }
    }

    if windows_failed {
        return Err(AppError::Config("apply: Windows target failed".into()));
    }
    if report.succeeded.is_empty() && !targets.is_empty() {
        return Err(AppError::Config("apply: no target succeeded".into()));
    }
    Ok(report)
}

pub(crate) fn write_one_target(
    target: &crate::cli_target::CliTarget,
    api_key: &str,
    claude_model: Option<&str>,
    claude_subagent_model: Option<&str>,
    claude_haiku_model: Option<&str>,
    codex_model: Option<&str>,
    codex_subagent_model: Option<&str>,
    claude_extra_env: Option<&BTreeMap<String, String>>,
    original_snapshot: Option<&snapshot::TargetSnapshot>,
) -> Result<(), AppError> {
    let b = &*target.backend;
    if target.installed.claude {
        write_claude_config_with_extra(
            b,
            &target.base_url,
            api_key,
            claude_model,
            claude_subagent_model,
            claude_haiku_model,
            claude_extra_env,
            original_snapshot.map(|snapshot| &snapshot.claude),
        )?;
        // Non-fatal: a settings.json pointing at the relay is still useful even
        // if this one file could not be touched, and failing the whole target
        // would also skip codex/gemini below.
        if let Err(e) = ensure_claude_onboarded_with(b) {
            log::warn!("could not set hasCompletedOnboarding in ~/.claude.json: {e}");
        }
    }
    if target.installed.codex {
        write_codex_config_with_snapshot(
            b,
            &target.base_url,
            api_key,
            codex_model,
            codex_subagent_model,
            original_snapshot.map(|snap| &snap.codex),
        )?;
        // Codex CLI refuses to start without OPENAI_API_KEY. The local host is
        // handled once in `ensure_openai_api_key_env` (which also covers the
        // Windows registry and macOS launchctl); a WSL distro has its own home
        // and its own login shell, so it needs its own rc line. Non-fatal for
        // the same reason as the claude case above.
        if target.snapshot_meta.target_type == crate::cli_target::TargetType::Wsl {
            if let Some(distro) = target.snapshot_meta.distro_name.as_deref() {
                let shell = detect_shell(crate::wsl::distro::login_shell(distro).as_deref());
                if let Err(e) = ensure_openai_api_key_in_rc(b, shell) {
                    log::warn!("could not set OPENAI_API_KEY in {}: {e}", target.label);
                }
            }
        }
    }
    if target.installed.gemini {
        write_gemini_config_with(b, &target.base_url, api_key)?;
    }
    Ok(())
}

/// Disable: restore every target from its snapshot. Each snapshot is deleted
/// only after its target is fully restored, so failures remain retryable.
pub fn clear_targets_from_snapshots() -> Result<(), AppError> {
    use crate::cli_target::TargetType;
    let dir = crate::paths::cli_config_backup_dir();
    if !dir.exists() {
        return Ok(());
    }

    let mut failures = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) => {
                let message = format!("read {} failed: {e}", path.display());
                log::warn!("clear: {message}");
                failures.push(message);
                continue;
            }
        };
        let snap: snapshot::TargetSnapshot = match serde_json::from_slice(&bytes) {
            Ok(snap) => snap,
            Err(e) => {
                let message = format!("malformed snapshot {}: {e}", path.display());
                log::warn!("{message}");
                failures.push(message);
                continue;
            }
        };
        let target_type = if snap.target_type == "wsl" {
            TargetType::Wsl
        } else {
            TargetType::Windows
        };
        let backend: Box<dyn CliBackend> = match target_type {
            TargetType::Windows => Box::new(crate::cli_target::WindowsFsBackend::new()),
            TargetType::Wsl => Box::new(crate::cli_target::WslBackend {
                distro: snap.distro_name.clone().unwrap_or_default(),
                home: snap.home.clone().unwrap_or_default(),
            }),
        };
        if let Err(e) = snapshot::restore(&snap, &*backend) {
            let message = format!("restore {} failed: {e}", path.display());
            log::warn!("clear {message}");
            failures.push(message);
            continue;
        }
        #[cfg(target_os = "windows")]
        if let TargetType::Wsl = target_type {
            if let Some(distro) = snap.distro_name.as_deref() {
                let hostname = crate::wsl::hosts::relay_hostname();
                if let Err(e) = crate::wsl::hosts::clear_hosts_entry(distro, &hostname) {
                    let message = format!("clear hosts entry for {distro} failed: {e}");
                    log::warn!("{message}");
                    failures.push(message);
                    continue;
                }
            }
        }
        if let Err(e) = std::fs::remove_file(&path) {
            let message = format!("delete restored snapshot {} failed: {e}", path.display());
            log::warn!("{message}");
            failures.push(message);
        }
    }

    if failures.is_empty() {
        match std::fs::remove_dir(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AppError::Config(format!(
                "clear: restored targets but could not remove snapshot directory: {e}"
            ))),
        }
    } else {
        Err(AppError::Config(format!(
            "clear: {} target snapshot(s) could not be restored; backups were kept: {}",
            failures.len(),
            failures.join("; ")
        )))
    }
}
