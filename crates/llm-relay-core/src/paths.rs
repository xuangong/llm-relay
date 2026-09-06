use std::path::PathBuf;

/// Application config + runtime directory.
///
/// Honors the `LLM_RELAY_HOME` environment variable so dev sessions can
/// run alongside a real GUI / agent without trampling each other's state
/// (use a different `LLM_RELAY_HOME` + `LLM_RELAY_PROXY_PORT` in the dev
/// shell). Falls back to `~/.llm-relay`.
pub fn config_dir() -> PathBuf {
    if let Ok(p) = std::env::var("LLM_RELAY_HOME") {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".llm-relay")
}

/// Runtime directory for sockets, PID files, and lock files.
/// Honors the `LLM_RELAY_RUNTIME_DIR` environment variable if set;
/// otherwise falls back to `config_dir()`.
pub fn runtime_dir() -> PathBuf {
    if let Ok(p) = std::env::var("LLM_RELAY_RUNTIME_DIR") {
        return PathBuf::from(p);
    }
    config_dir()
}

pub fn pid_file() -> PathBuf {
    runtime_dir().join("agent.pid")
}
pub fn lock_file() -> PathBuf {
    runtime_dir().join("agent.lock")
}
pub fn sock_file() -> PathBuf {
    runtime_dir().join("agent.sock")
}
pub fn log_file() -> PathBuf {
    runtime_dir().join("agent.log")
}
pub fn db_file() -> PathBuf {
    runtime_dir().join("config.db")
}

pub const PROXY_PORT: u16 = 18080;

/// Resolve the proxy port. Honors the `LLM_RELAY_PROXY_PORT` environment
/// variable (useful for tests so they can run alongside a real GUI / agent
/// holding the default port). Falls back to `PROXY_PORT`.
pub fn proxy_port() -> u16 {
    std::env::var("LLM_RELAY_PROXY_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(PROXY_PORT)
}

/// Per-target CLI snapshot directory. Each file inside is a snapshot for
/// either the Windows host or one WSL2 distro. Pre-WSL2 versions stored a
/// single file at `legacy_cli_config_backup_file()`.
pub fn cli_config_backup_dir() -> PathBuf {
    config_dir().join("cli-config-backup")
}

/// Pre-WSL2 single-file snapshot path. Read once on startup by the legacy
/// migration in `config_writer::snapshot::migrate_legacy_if_needed()`.
pub fn legacy_cli_config_backup_file() -> PathBuf {
    config_dir().join("cli-config-backup.json")
}

pub fn cli_file_lifecycle_manifest() -> PathBuf {
    config_dir().join("cli-file-lifecycle.json")
}

pub fn cli_file_lifecycle_blocked() -> PathBuf {
    config_dir().join("cli-file-lifecycle.blocked")
}
