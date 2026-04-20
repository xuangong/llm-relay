#[path = "support/mod.rs"]
mod support;
use support::*;

/// Test requires binding port 18080 and spawning a real agent binary.
/// Marked #[ignore] to avoid default test-run failures.
/// Run with:
///   cargo build -p llm-relay-agent
///   cargo test -p llm-relay-agent --test mutual_exclusion -- --ignored --test-threads=1

#[ignore = "binds port 18080; run with --ignored serially"]
#[test]
fn agent_refuses_when_port_18080_already_bound() {
    // Bind 127.0.0.1:18080 from the test process to simulate the GUI.
    let _hold = std::net::TcpListener::bind(("127.0.0.1", 18080))
        .expect("bind 18080 — make sure no llm-relay is running");

    let dir = tempfile::tempdir().unwrap();
    let bin = AgentBin::locate();
    let child = bin.spawn(dir.path());
    let out = child.wait_with_output().expect("collect");
    assert!(!out.status.success(), "agent should refuse with port in use");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("port") || stderr.contains("AddrInUse") || stderr.contains("18080"),
        "expected port-in-use error, got: {stderr}"
    );
}
