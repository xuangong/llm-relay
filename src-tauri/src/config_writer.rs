use crate::error::AppError;
use serde::{Deserialize, Serialize};
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

    // Check if the current config already matches
    let needs_update = if let Some(env_obj) = env.as_object() {
        env_obj.get("ANTHROPIC_AUTH_TOKEN").and_then(|v| v.as_str()) != Some(api_key)
            || env_obj.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str()) != Some(base_url)
            || (model.is_some() && env_obj.get("ANTHROPIC_MODEL").and_then(|v| v.as_str()) != model)
            || (small_model.is_some() && env_obj.get("ANTHROPIC_SMALL_FAST_MODEL").and_then(|v| v.as_str()) != small_model)
    } else {
        true
    };

    if !needs_update {
        // Config already correct, skip write
        return Ok(());
    }

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

    // Check if update needed
    let needs_update = env_map.get("GEMINI_API_KEY") != Some(&api_key.to_string())
        || env_map.get("GOOGLE_GEMINI_BASE_URL") != Some(&base_url.to_string());

    if !needs_update {
        // Config already correct, skip write
        return Ok(());
    }

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

/// Write all three CLI configs pointing to the local proxy.
/// base_url and api_key are ignored — the proxy handles routing and key injection.
pub fn apply_all_configs(
    _base_url: &str,
    _api_key: &str,
    claude_model: Option<&str>,
    claude_small_model: Option<&str>,
    codex_model: Option<&str>,
    _gemini_model: Option<&str>,
) -> Result<(), AppError> {
    let proxy_url = crate::proxy_server::proxy_base_url();
    let key = crate::proxy_server::PLACEHOLDER_KEY;
    write_claude_config(&proxy_url, key, claude_model, claude_small_model)?;
    write_codex_config(&proxy_url, key, codex_model)?;
    write_gemini_config(&proxy_url, key)?;
    ensure_openai_api_key_in_shell_rc()?;
    Ok(())
}

/// Clear all three CLI configs.
pub fn clear_all_configs() -> Result<(), AppError> {
    clear_claude_config()?;
    clear_codex_config()?;
    clear_gemini_config()?;
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
