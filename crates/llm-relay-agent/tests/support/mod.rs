use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct AgentBin {
    pub bin: PathBuf,
}

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
        Command::new(&self.bin)
            .env("LLM_RELAY_RUNTIME_DIR", runtime_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agent")
    }
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
