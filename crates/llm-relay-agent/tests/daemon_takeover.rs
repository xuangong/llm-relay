#[path = "support/mod.rs"]
mod support;
use support::*;
use std::time::Duration;

/// Verifies the shared `request_agent_stop` helper used by the GUI's
/// "stop daemon and start GUI" dialog: sends a Shutdown frame over the IPC
/// socket and waits for the lifecycle lock to release.
///
/// Run with:
///   cargo build -p llm-relay-agent
///   cargo test -p llm-relay-agent --test daemon_takeover -- --ignored --test-threads=1

/// End-to-end simulation of the GUI's "stop daemon and start GUI" flow:
///   1. spawn a real agent (plays the role of the running daemon)
///   2. simulate GUI startup — `LifecycleGuard::acquire` must fail with
///      `AlreadyRunning`, and `live_agent_pid()` must point at the daemon
///   3. call `request_agent_stop` (the same helper the GUI dialog calls)
///   4. re-acquire the lock — this must now succeed, proving the GUI can
///      truly take over after the daemon stops
#[cfg_attr(windows, ignore = "uses Unix socket semantics")]
#[ignore = "requires built agent binary; run with --ignored"]
#[test]
fn gui_takeover_flow_end_to_end() {
    use llm_relay_core::lifecycle::{self, AcquireError, LifecycleGuard};

    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("LLM_RELAY_RUNTIME_DIR", dir.path());
    // Pick an ephemeral proxy port so we don't collide with a real GUI.
    let port_holder = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = port_holder.local_addr().unwrap().port();
    drop(port_holder);
    std::env::set_var("LLM_RELAY_PROXY_PORT", port.to_string());

    // 1. Spawn the daemon.
    let bin = AgentBin::locate();
    let mut child = bin.spawn(dir.path());
    let sock = dir.path().join("agent.sock");
    let pidf = dir.path().join("agent.pid");
    assert!(wait_for_socket(&sock, Duration::from_secs(5)));
    let daemon_pid: u32 = std::fs::read_to_string(&pidf)
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    // 2. Simulate GUI startup: acquire must fail, and live_agent_pid must
    //    see the daemon (this is exactly what the GUI dialog predicates on).
    match LifecycleGuard::acquire() {
        Err(AcquireError::AlreadyRunning) => {}
        Err(e) => panic!("expected AlreadyRunning, got Err({e:?})"),
        Ok(_) => panic!("expected AlreadyRunning, but acquire succeeded"),
    }
    let seen_pid = lifecycle::live_agent_pid()
        .expect("GUI must be able to see the running daemon");
    assert_eq!(seen_pid, daemon_pid);

    // 3. User clicks "Yes, stop daemon".
    lifecycle::request_agent_stop(Duration::from_secs(5))
        .expect("request_agent_stop should succeed");

    // Daemon should be gone.
    assert!(wait_for_no_socket(&sock, Duration::from_secs(3)));
    assert!(!pidf.exists());
    let _ = child.wait();

    // 4. GUI re-acquires — this is the success case the dialog promises.
    let guard = LifecycleGuard::acquire().expect("GUI should acquire after takeover");
    // And it really got the port.
    assert!(guard.proxy_listener.is_some());
    drop(guard);

    std::env::remove_var("LLM_RELAY_RUNTIME_DIR");
    std::env::remove_var("LLM_RELAY_PROXY_PORT");
}

#[cfg_attr(windows, ignore = "uses Unix socket semantics")]
#[ignore = "requires built agent binary; run with --ignored"]
#[test]
fn request_agent_stop_releases_lock() {
    let dir = tempfile::tempdir().unwrap();
    // The helper reads `paths::sock_file()` / `paths::lock_file()`, which
    // honor LLM_RELAY_RUNTIME_DIR — point them at the same tempdir the
    // agent spawns into.
    std::env::set_var("LLM_RELAY_RUNTIME_DIR", dir.path());

    let bin = AgentBin::locate();
    let mut child = bin.spawn(dir.path());
    let sock = dir.path().join("agent.sock");
    let pidf = dir.path().join("agent.pid");
    assert!(wait_for_socket(&sock, Duration::from_secs(5)));
    assert!(pidf.exists());

    // Ask nicely.
    llm_relay_core::lifecycle::request_agent_stop(Duration::from_secs(5))
        .expect("request_agent_stop should succeed");

    // Agent should have exited cleanly.
    assert!(wait_for_no_socket(&sock, Duration::from_secs(3)));
    assert!(!pidf.exists());

    // Reap.
    let _ = child.wait();

    std::env::remove_var("LLM_RELAY_RUNTIME_DIR");
}
