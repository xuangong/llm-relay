use llm_relay_tui::app::{loop_, terminal::TermGuard};
use llm_relay_tui::bootstrap::{ensure_agent, AgentHandle, EnsureMode};
use llm_relay_core::paths;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let socket = paths::sock_file();
    let handle = ensure_agent(&socket, EnsureMode::AttachOrSpawn).await?;
    let client = match handle {
        AgentHandle::Attached(c) => c,
        AgentHandle::Spawned { client, .. } => client,
    };

    let mut guard = TermGuard::enter()?;
    // Take the terminal out of the guard so we can pass ownership to run().
    // We rebuild a dummy terminal in the guard so Drop still restores raw mode.
    let term = std::mem::replace(
        &mut guard.terminal,
        ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(std::io::stdout()))?,
    );
    loop_::run(term, client, socket).await?;
    Ok(())
}
