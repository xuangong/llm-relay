#[path = "support/mod.rs"]
mod support;
use support::*;
use std::time::Duration;

/// All lifecycle tests require the agent binary to be pre-built and spawn real
/// OS processes + Unix sockets. They are marked #[ignore] so they don't run in
/// the default `cargo test` pass (which runs without a pre-built binary in CI
/// until the build step finishes). Run them explicitly with:
///   cargo build -p llm-relay-agent
///   cargo test -p llm-relay-agent --test lifecycle_integration -- --ignored --test-threads=1

#[cfg_attr(windows, ignore = "uses Unix socket semantics")]
#[ignore = "requires built agent binary; run with --ignored"]
#[test]
fn agent_recovers_from_stale_socket_left_by_killed_process() {
    let dir = tempfile::tempdir().unwrap();
    let bin = AgentBin::locate();

    // Spawn agent, wait for socket, kill -9, restart, expect success.
    let mut child = bin.spawn(dir.path());
    let sock = dir.path().join("agent.sock");
    assert!(wait_for_socket(&sock, Duration::from_secs(5)), "agent did not bind socket");

    // SIGKILL — leaves socket file dangling.
    #[cfg(unix)]
    unsafe { libc::kill(child.id() as i32, libc::SIGKILL); }
    let _ = child.wait();
    // Socket file persists after SIGKILL on unix — confirm.
    assert!(sock.exists(), "stale socket should still be on disk");

    // Restart should succeed by removing the stale socket.
    let mut child2 = bin.spawn(dir.path());
    assert!(wait_for_socket(&sock, Duration::from_secs(5)), "restart did not rebind");

    // Cleanup
    #[cfg(unix)]
    unsafe { libc::kill(child2.id() as i32, libc::SIGTERM); }
    let _ = child2.wait();
}

#[ignore = "requires built agent binary; run with --ignored"]
#[test]
fn second_agent_refuses_to_start_when_first_holds_lock() {
    let dir = tempfile::tempdir().unwrap();
    let bin = AgentBin::locate();

    let mut a = bin.spawn(dir.path());
    let sock = dir.path().join("agent.sock");
    assert!(wait_for_socket(&sock, Duration::from_secs(5)));

    // Second invocation must exit non-zero (lock held).
    let b = bin.spawn(dir.path());
    let status = b.wait_with_output().expect("collect output");
    assert!(!status.status.success(), "second agent should refuse to start");
    let stderr = String::from_utf8_lossy(&status.stderr);
    assert!(
        stderr.contains("already running") || stderr.contains("lock"),
        "expected lock error, got: {stderr}"
    );

    #[cfg(unix)]
    unsafe { libc::kill(a.id() as i32, libc::SIGTERM); }
    let _ = a.wait();
}

#[ignore = "requires built agent binary; run with --ignored"]
#[test]
fn agent_starts_when_pidfile_holds_a_pid_no_longer_alive() {
    let dir = tempfile::tempdir().unwrap();
    // Pre-write a pidfile pointing at a guaranteed-dead PID. PID 0 is
    // never a real process; on Unix `kill(0, 0)` would refer to the
    // process group, so use u32::MAX which is reliably non-existent.
    std::fs::write(dir.path().join("agent.pid"), "4294967295").unwrap();
    // Also drop a stale socket file in.
    std::fs::write(dir.path().join("agent.sock"), b"").unwrap();

    let bin = AgentBin::locate();
    let mut child = bin.spawn(dir.path());
    let sock = dir.path().join("agent.sock");
    assert!(
        wait_for_socket(&sock, std::time::Duration::from_secs(5)),
        "agent should start despite stale pidfile + socket"
    );

    #[cfg(unix)]
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM); }
    let _ = child.wait();
}

#[ignore = "requires built agent binary; run with --ignored"]
#[test]
fn graceful_shutdown_removes_pidfile_and_socket() {
    let dir = tempfile::tempdir().unwrap();
    let bin = AgentBin::locate();
    let mut child = bin.spawn(dir.path());

    let sock = dir.path().join("agent.sock");
    let pidf = dir.path().join("agent.pid");
    assert!(wait_for_socket(&sock, std::time::Duration::from_secs(5)));
    assert!(pidf.exists());

    #[cfg(unix)]
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM); }
    let _ = child.wait();

    assert!(wait_for_no_socket(&sock, std::time::Duration::from_secs(3)),
        "socket should be cleaned up on graceful exit");
    assert!(!pidf.exists(), "pidfile should be cleaned up on graceful exit");
}
