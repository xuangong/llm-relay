use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".llm-relay")
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

pub fn pid_file() -> PathBuf { runtime_dir().join("agent.pid") }
pub fn lock_file() -> PathBuf { runtime_dir().join("agent.lock") }
pub fn sock_file() -> PathBuf { runtime_dir().join("agent.sock") }
pub fn log_file() -> PathBuf { runtime_dir().join("agent.log") }
pub fn db_file() -> PathBuf { runtime_dir().join("config.db") }

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
