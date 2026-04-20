use llm_relay_tui::spawn;
use std::time::Duration;

#[test]
fn spawn_detached_returns_pid_and_child_outlives_parent_check() {
    // Spawn a long-running, harmless command (`sleep 5` on unix, `timeout 5` on windows).
    #[cfg(unix)]
    let (cmd, args): (&str, &[&str]) = ("sleep", &["5"]);
    #[cfg(windows)]
    let (cmd, args): (&str, &[&str]) = ("cmd", &["/C", "ping -n 5 127.0.0.1 >NUL"]);

    let pid = spawn::spawn_detached(cmd, args).expect("spawn ok");
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
