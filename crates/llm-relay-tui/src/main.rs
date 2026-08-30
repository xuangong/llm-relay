use llm_relay_tui::app::{loop_, terminal::TermGuard};
use llm_relay_tui::bootstrap::{ensure_agent, AgentHandle, BootstrapError, EnsureMode};
use llm_relay_tui::preflight;
use llm_relay_core::paths;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let socket = paths::sock_file();

    // Try attaching before anything else. An agent that is already running
    // has its own master key (from systemd, a shell export, whatever started
    // it), so the TUI needs none — asking for one would be a prompt with no
    // purpose and no effect.
    let attached = ensure_agent(&socket, EnsureMode::AttachOnly, &[]).await;

    let mut guard = TermGuard::enter()?;
    // Take the terminal out of the guard so we can pass ownership to run().
    // We rebuild a dummy terminal in the guard so Drop still restores raw mode.
    let mut term = std::mem::replace(
        &mut guard.terminal,
        ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(std::io::stdout()))?,
    );

    let (client, spawn_env) = match attached {
        Ok(handle) => (client_of(handle), Vec::new()),
        Err(BootstrapError::NoAgent) => {
            // Nothing running: we are about to spawn the agent, so we are the
            // ones who owe it a key.
            let env = preflight::ensure_master_key(&mut term, &paths::config_dir())?;
            let handle = ensure_agent(&socket, EnsureMode::AttachOrSpawn, &env).await?;
            (client_of(handle), env)
        }
        Err(e) => return Err(e.into()),
    };

    loop_::run(term, client, socket, spawn_env).await?;
    Ok(())
}

fn client_of(handle: AgentHandle) -> Arc<llm_relay_tui::ipc_client::IpcClient> {
    match handle {
        AgentHandle::Attached(c) => c,
        AgentHandle::Spawned { client, .. } => client,
    }
}
