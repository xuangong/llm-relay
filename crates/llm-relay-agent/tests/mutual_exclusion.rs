#[path = "support/mod.rs"]
mod support;
use support::*;
use std::process::{Command, Stdio};

/// Test requires binding the proxy port and spawning a real agent binary.
/// Marked #[ignore] to avoid default test-run failures.
/// Run with:
///   cargo build -p llm-relay-agent
///   cargo test -p llm-relay-agent --test mutual_exclusion -- --ignored --test-threads=1

#[ignore = "binds a real port; run with --ignored serially"]
#[test]
fn agent_refuses_when_proxy_port_already_bound() {
    // Bind an ephemeral port from the test process to simulate the GUI /
    // an unrelated process holding the proxy port. Then point the agent at
    // the same port via LLM_RELAY_PROXY_PORT and confirm it exits non-zero
    // with a useful error.
    let hold = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let port = hold.local_addr().unwrap().port();

    let dir = tempfile::tempdir().unwrap();
    let bin = AgentBin::locate();
    let child = Command::new(&bin.bin)
        .env("LLM_RELAY_RUNTIME_DIR", dir.path())
        .env("LLM_RELAY_PROXY_PORT", port.to_string())
        .env("LLM_RELAY_MASTER_KEY", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agent");
    let out = child.wait_with_output().expect("collect");
    assert!(!out.status.success(), "agent should refuse with port in use");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("port") || stderr.contains("AddrInUse") || stderr.contains(&port.to_string()),
        "expected port-in-use error, got: {stderr}"
    );
}
