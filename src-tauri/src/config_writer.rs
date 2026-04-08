use crate::error::AppError;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use toml_edit::DocumentMut;

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Atomic write: write to a temp file then rename.
fn atomic_write(path: &PathBuf, content: &[u8]) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

// ─── Claude Code ───
// ~/.claude/settings.json — merge into "env" block

fn claude_settings_path() -> PathBuf {
    home_dir().join(".claude").join("settings.json")
}

pub fn write_claude_config(
    base_url: &str,
    api_key: &str,
    model: Option<&str>,
    small_model: Option<&str>,
) -> Result<(), AppError> {
    let path = claude_settings_path();

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

    if let Some(env_obj) = env.as_object_mut() {
        env_obj.insert(
            "ANTHROPIC_AUTH_TOKEN".to_string(),
            Value::String(api_key.to_string()),
        );
        env_obj.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            Value::String(base_url.to_string()),
        );
        if let Some(m) = model {
            env_obj.insert("ANTHROPIC_MODEL".to_string(), Value::String(m.to_string()));
        }
        if let Some(m) = small_model {
            env_obj.insert(
                "ANTHROPIC_SMALL_FAST_MODEL".to_string(),
                Value::String(m.to_string()),
            );
        }
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
        env.remove("ANTHROPIC_AUTH_TOKEN");
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

    // Write auth.json
    let auth_path = dir.join("auth.json");
    let auth = serde_json::json!({
        "OPENAI_API_KEY": api_key
    });
    let auth_str = serde_json::to_string_pretty(&auth)?;
    atomic_write(&auth_path, auth_str.as_bytes())?;

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
            // Ensure base_url ends with /
            let url = if base_url.ends_with('/') {
                base_url.to_string()
            } else {
                format!("{base_url}/")
            };
            gw["base_url"] = toml_edit::value(&url);
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

fn parse_env_file(content: &str) -> HashMap<String, String> {
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

fn serialize_env_file(map: &HashMap<String, String>) -> String {
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

    env_map.insert("GEMINI_API_KEY".to_string(), api_key.to_string());
    env_map.insert("GOOGLE_GEMINI_BASE_URL".to_string(), base_url.to_string());

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

    let content = serialize_env_file(&env_map);
    atomic_write(&env_path, content.as_bytes())
}

/// Write all three CLI configs at once.
pub fn apply_all_configs(
    base_url: &str,
    api_key: &str,
    claude_model: Option<&str>,
    claude_small_model: Option<&str>,
    codex_model: Option<&str>,
    _gemini_model: Option<&str>,
) -> Result<(), AppError> {
    write_claude_config(base_url, api_key, claude_model, claude_small_model)?;
    write_codex_config(base_url, api_key, codex_model)?;
    write_gemini_config(base_url, api_key)?;
    Ok(())
}

/// Clear all three CLI configs.
pub fn clear_all_configs() -> Result<(), AppError> {
    clear_claude_config()?;
    clear_codex_config()?;
    clear_gemini_config()?;
    Ok(())
}

/// Read current config from all three CLI tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentCliConfig {
    pub claude: Option<Value>,
    pub codex_auth: Option<Value>,
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
