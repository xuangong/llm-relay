use llm_relay_tui::spawn;
use std::time::Duration;

#[test]
fn spawn_detached_returns_pid_and_child_outlives_parent_check() {
    // Spawn a long-running, harmless command (`sleep 5` on unix, `timeout 5` on windows).
    #[cfg(unix)]
    let (cmd, args): (&str, &[&str]) = ("sleep", &["5"]);
    #[cfg(windows)]
    let (cmd, args): (&str, &[&str]) = ("cmd", &["/C", "ping -n 5 127.0.0.1 >NUL"]);

    let pid = spawn::spawn_detached(cmd, args, &[]).expect("spawn ok");
    assert!(pid > 0);
    // Give the child a moment to register with the OS.
    std::thread::sleep(Duration::from_millis(50));
    assert!(spawn::process_alive(pid), "child should be alive");

    // Clean up
    #[cfg(unix)]
    unsafe { libc::kill(pid as i32, libc::SIGTERM); }
    #[cfg(windows)] {
        use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if h != 0 { TerminateProcess(h, 0); }
        }
    }
}

/// The first-run wizard hands the master key to the agent this way and nowhere
/// else — it is never written down. If the env didn't survive the double fork,
/// the agent would come up keyless and exit before the TUI noticed.
#[cfg(unix)]
#[test]
fn spawn_detached_passes_env_to_the_child() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("seen");
    let script = format!("printenv LLM_RELAY_TEST_KEY > {}", out.display());

    let pid = spawn::spawn_detached(
        "sh",
        &["-c", &script],
        &[("LLM_RELAY_TEST_KEY".into(), "s3cret-value".into())],
    )
    .expect("spawn ok");
    assert!(pid > 0);

    // The grandchild is reparented, so there is no handle to wait on.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !out.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let seen = std::fs::read_to_string(&out).expect("child should have written the env var");
    assert_eq!(seen.trim(), "s3cret-value");
}

