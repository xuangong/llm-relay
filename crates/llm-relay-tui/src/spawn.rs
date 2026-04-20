//! Spawn the agent binary as a detached background process.
//!
//! Unix: double-fork via `daemonize`-style call with `setsid` and stdio
//! redirected to /dev/null so the child outlives the parent shell session.
//!
//! Windows: `CreateProcessW` with `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`
//! flags so the child does not inherit the parent's console.

use std::io;

#[cfg(unix)]
pub fn spawn_detached(cmd: &str, args: &[&str]) -> io::Result<u32> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let mut child = unsafe {
        Command::new(cmd)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .pre_exec(|| {
                // Become a new session leader so we are detached from the
                // controlling terminal. Errors here propagate to the caller.
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            })
            .spawn()?
    };
    let pid = child.id();
    // Don't wait — we want a true detach. Caller relies on PID file written by
    // the agent itself for liveness tracking, not on our `Child` handle.
    std::mem::forget(child);
    Ok(pid)
}

#[cfg(windows)]
pub fn spawn_detached(cmd: &str, args: &[&str]) -> io::Result<u32> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    // Constants from windows-sys::Win32::System::Threading
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
        .spawn()?;
    let pid = child.id();
    std::mem::forget(child);
    Ok(pid)
}

/// Probe whether a process with the given pid is still alive.
/// Implementation lives in `llm-relay-core::process` — re-exported for
/// convenience so `spawn` consumers don't need an extra import.
pub fn process_alive(pid: u32) -> bool {
    llm_relay_core::process::is_alive(pid)
}
