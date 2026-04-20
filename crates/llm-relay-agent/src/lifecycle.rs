//! Agent lifecycle: thin re-export of the core lifecycle guard.
//!
//! The actual implementation lives in `llm_relay_core::lifecycle` so the GUI
//! and the agent share the SAME lock + port bind path and emit consistent
//! "another LLM Relay process is running" errors.

pub use llm_relay_core::lifecycle::{live_agent_pid, AcquireError, LifecycleGuard};
pub use llm_relay_core::process::is_alive as process_alive;
