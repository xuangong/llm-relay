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
            } else if assigned_openai_value(trimmed, shell)
                .is_some_and(is_relay_owned_openai_value)
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
fn ensure_openai_api_key_in_rc(
    backend: &dyn CliBackend,
    shell: ShellKind,
) -> Result<(), AppError> {
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
    let ours = inherited
        .as_deref()
        .is_none_or(is_relay_owned_openai_value);
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
        let b = WindowsFsBackend { home: tmp.path().to_path_buf() };
        ensure_openai_api_key_in_rc(&b, ShellKind::Fish).unwrap();
        // fish's config dir does not exist beforehand; the backend must create it.
        let written = b.read(&[".config", "fish", "config.fish"]).unwrap().unwrap();
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
        b.write_atomic(&[".claude.json"], br#"{"hasCompletedOnboarding":false}"#).unwrap();
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
        b.write_atomic(&[".claude.json"], original.as_bytes()).unwrap();

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
        b.write_atomic(&[".claude.json"], broken.as_bytes()).unwrap();

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

pub fn apply_to_targets(
    targets: &[crate::cli_target::CliTarget],
    api_key: &str,
    claude_model: Option<&str>,
    claude_small_model: Option<&str>,
    codex_model: Option<&str>,
    _gemini_model: Option<&str>,
) -> Result<(), AppError> {
    use crate::cli_target::TargetType;

    let prev_index = snapshot::build_index()?;
    let current_keys: std::collections::HashSet<String> = targets
        .iter()
        .map(|t| {
            t.snapshot_meta
                .distro_name
                .clone()
                .unwrap_or_else(|| "windows".to_string())
        })
        .collect();

    // 1. Restore + delete snapshots for targets removed since last apply.
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
            }
            #[cfg(target_os = "windows")]
            if let TargetType::Wsl = meta.target_type {
                if let Some(distro) = meta.distro_name.as_deref() {
                    let hn = crate::wsl::hosts::relay_hostname();
                    if let Err(e) = crate::wsl::hosts::clear_hosts_entry(distro, &hn) {
                        log::warn!("clear hosts entry for dropped {distro}: {e}");
                    }
                }
            }
        }
        let _ = snapshot::delete(meta);
    }

    // 2. For each current target: capture snapshot if new, then write.
    let mut at_least_one_success = false;
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
        }
        match write_one_target(target, api_key, claude_model, claude_small_model, codex_model) {
            Ok(()) => {
                at_least_one_success = true;
            }
            Err(e) => {
                log::warn!("apply failed for {key}: {e}");
                if is_windows {
                    windows_failed = true;
                }
            }
        }
    }

    if windows_failed {
        return Err(AppError::Config("apply: Windows target failed".into()));
    }
    if !at_least_one_success && !targets.is_empty() {
        return Err(AppError::Config("apply: no target succeeded".into()));
    }
    Ok(())
}

fn write_one_target(
    target: &crate::cli_target::CliTarget,
    api_key: &str,
    claude_model: Option<&str>,
    claude_small_model: Option<&str>,
    codex_model: Option<&str>,
) -> Result<(), AppError> {
    let b = &*target.backend;
    if target.installed.claude {
        write_claude_config_with(b, &target.base_url, api_key, claude_model, claude_small_model)?;
        // Non-fatal: a settings.json pointing at the relay is still useful even
        // if this one file could not be touched, and failing the whole target
        // would also skip codex/gemini below.
        if let Err(e) = ensure_claude_onboarded_with(b) {
            log::warn!("could not set hasCompletedOnboarding in ~/.claude.json: {e}");
        }
    }
    if target.installed.codex {
        write_codex_config_with(b, &target.base_url, api_key, codex_model)?;
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

/// Disable: walk the snapshot directory and restore every target it
/// covers, then delete the directory. Replaces `clear_all_configs` for
/// callers that want the multi-target behavior.
pub fn clear_targets_from_snapshots() -> Result<(), AppError> {
    use crate::cli_target::TargetType;
    let dir = crate::paths::cli_config_backup_dir();
    if !dir.exists() {
        return Ok(());
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
                log::warn!("clear: read {} failed: {e}", path.display());
                continue;
            }
        };
        let snap: snapshot::TargetSnapshot = match serde_json::from_slice(&bytes) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("malformed snapshot {}: {e}", path.display());
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
            log::warn!("clear restore failed for {}: {e}", path.display());
        }
        #[cfg(target_os = "windows")]
        if let TargetType::Wsl = target_type {
            if let Some(distro) = snap.distro_name.as_deref() {
                let hn = crate::wsl::hosts::relay_hostname();
                if let Err(e) = crate::wsl::hosts::clear_hosts_entry(distro, &hn) {
                    log::warn!("clear hosts entry on disable for {distro}: {e}");
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
