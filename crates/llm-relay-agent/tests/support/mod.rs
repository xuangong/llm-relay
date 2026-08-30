use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct AgentBin {
    pub bin: PathBuf,
}

// Same reason as the free functions below: `spawn` is used by some test
// binaries and not others, and each recompiles this module independently.
#[allow(dead_code)]
impl AgentBin {
    pub fn locate() -> Self {
        // CARGO_BIN_EXE_<name> is set by cargo when building the test crate
        // for binaries declared in the same package. The agent binary is in
        // its own package, so we walk back from the test exe instead.
        let bin = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap()    // crates/
            .parent().unwrap()    // workspace root
            .join("target")
            .join(if cfg!(debug_assertions) { "debug" } else { "release" })
            .join(if cfg!(windows) { "llm-relay-agent.exe" } else { "llm-relay-agent" });
        assert!(bin.exists(), "agent binary not built — run `cargo build -p llm-relay-agent` first");
        Self { bin }
    }

    pub fn spawn(&self, runtime_dir: &Path) -> Child {
        // Pick an ephemeral free port so the test never collides with a real
        // GUI / agent that may be holding the default 18080.
        let port = pick_free_port();
        Command::new(&self.bin)
            .env("LLM_RELAY_RUNTIME_DIR", runtime_dir)
            .env("LLM_RELAY_PROXY_PORT", port.to_string())
            // Headless agent requires LLM_RELAY_MASTER_KEY (base64 32 bytes).
            // Use a deterministic per-test key so secrets.env.enc can roundtrip
            // across restarts within the same test.
            .env("LLM_RELAY_MASTER_KEY", test_master_key())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agent")
    }
}

#[allow(dead_code)]
fn test_master_key() -> &'static str {
    // base64 of 32 zero bytes — fine for tests, never write this in prod.
    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
}

#[allow(dead_code)]
fn pick_free_port() -> u16 {
    // Bind to port 0, read assigned port, drop. Brief race window but fine
    // for tests that immediately respawn into a fresh runtime dir.
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    l.local_addr().expect("local_addr").port()
}

// Each integration test binary recompiles this module independently,
// so a helper used in one binary appears dead in another. Suppress the
// warning rather than scattering #[allow] at every import site.
#[allow(dead_code)]
pub fn wait_for_socket(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() { return true; }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[allow(dead_code)]
pub fn wait_for_no_socket(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !path.exists() { return true; }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}
