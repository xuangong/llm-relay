use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".llm-relay")
}

pub fn pid_file() -> PathBuf { config_dir().join("agent.pid") }
pub fn lock_file() -> PathBuf { config_dir().join("agent.lock") }
pub fn sock_file() -> PathBuf { config_dir().join("agent.sock") }
pub fn log_file() -> PathBuf { config_dir().join("agent.log") }
pub fn db_file() -> PathBuf { config_dir().join("config.db") }

pub const PROXY_PORT: u16 = 18080;
