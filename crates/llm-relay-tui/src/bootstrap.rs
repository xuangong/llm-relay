//! Decide whether to attach to a running agent or spawn a fresh one.
//!
//! Order of operations:
//!  1. If the socket exists, try a `Ping` over it. On success → Attached.
//!  2. Otherwise, if `mode == AttachOnly`, return `NoAgent`.
//!  3. Otherwise: spawn the agent binary detached, then poll the socket
//!     for up to 5 seconds, retrying `Ping` until it succeeds.

use crate::ipc_client::IpcClient;
use crate::spawn;
use llm_relay_core::ipc::Request;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub enum EnsureMode {
    /// Attach if running; spawn a new agent if not.
    AttachOrSpawn,
    /// Attach only — fail if no agent is running.
    AttachOnly,
}

pub enum AgentHandle {
    /// Connected to a pre-existing agent.
    Attached(Arc<IpcClient>),
    /// Spawned a new agent and connected.
    Spawned { client: Arc<IpcClient>, pid: u32 },
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("no agent running and AttachOnly was requested")]
    NoAgent,
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("agent did not become ready within timeout")]
    Timeout,
    #[error(transparent)]
    Client(#[from] crate::ipc_client::ClientError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub async fn ensure_agent(
    socket: &Path,
    mode: EnsureMode,
) -> Result<AgentHandle, BootstrapError> {
    if socket.exists() {
        if let Ok(client) = IpcClient::connect(socket).await {
            if client.request(Request::Ping).await.is_ok() {
                return Ok(AgentHandle::Attached(client));
            }
        }
        // Stale socket: remove and fall through to spawn (or error if AttachOnly).
        let _ = std::fs::remove_file(socket);
    }

    match mode {
        EnsureMode::AttachOnly => Err(BootstrapError::NoAgent),
        EnsureMode::AttachOrSpawn => {
            let agent_bin = locate_agent_binary()?;
            let pid = spawn::spawn_detached(
                agent_bin.to_str().expect("utf-8 path"),
                &[],
            )
            .map_err(|e| BootstrapError::Spawn(e.to_string()))?;

            // Wait up to 5s for the agent to bind its socket and answer Ping.
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if Instant::now() >= deadline {
                    return Err(BootstrapError::Timeout);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                if !socket.exists() {
                    continue;
                }
                if let Ok(client) = IpcClient::connect(socket).await {
                    if client.request(Request::Ping).await.is_ok() {
                        return Ok(AgentHandle::Spawned { client, pid });
                    }
                }
            }
        }
    }
}

/// Find the agent binary alongside the current TUI executable.
/// Cargo lays them out as `target/<profile>/llm-relay-agent` and
/// `target/<profile>/llm-relay-tui`, so a sibling lookup works.
fn locate_agent_binary() -> std::io::Result<PathBuf> {
    let me = std::env::current_exe()?;
    let dir = me
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent dir for current_exe"))?;
    let name = if cfg!(windows) {
        "llm-relay-agent.exe"
    } else {
        "llm-relay-agent"
    };
    let candidate = dir.join(name);
    if !candidate.exists() {
        return Err(std::io::Error::other(format!(
            "agent binary not found at {}",
            candidate.display()
        )));
    }
    Ok(candidate)
}
