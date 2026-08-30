//! Spawn the agent binary as a detached background process.
//!
//! Unix: double-fork via `pre_exec` so the agent reparents to init/launchd
//! and survives the TUI exiting. The intermediate child writes the
//! grandchild's PID through a pipe, then `_exit`s; the parent reads the
//! PID from the pipe and reaps the intermediate to avoid a zombie.
//!
//! Windows: `CreateProcessW` with `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`
//! flags so the child does not inherit the parent's console.

use std::io;

/// Extra environment for the spawned agent, as `(name, value)` pairs.
///
/// Passed explicitly rather than via `std::env::set_var`: mutating the
/// process environment is unsound once other threads exist, and the TUI is
/// a multi-threaded tokio runtime long before it decides to spawn an agent.
pub type EnvPairs = [(String, String)];

#[cfg(unix)]
pub fn spawn_detached(cmd: &str, args: &[&str], envs: &EnvPairs) -> io::Result<u32> {
    use std::io::Read;
    use std::os::unix::io::{FromRawFd, IntoRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    // Anonymous pipe: write end is closed-on-exec for the grandchild, write
    // end is used by the intermediate fork to publish the grandchild PID.
    let mut pipe_fds: [libc::c_int; 2] = [-1, -1];
    if unsafe { libc::pipe(pipe_fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let read_fd: OwnedFd = unsafe { OwnedFd::from_raw_fd(pipe_fds[0]) };
    let write_fd: OwnedFd = unsafe { OwnedFd::from_raw_fd(pipe_fds[1]) };
    let write_raw = write_fd.into_raw_fd();

    // SAFETY: pre_exec runs in the forked child between fork() and exec().
    // We use only async-signal-safe libc calls (setsid, fork, write, _exit, close).
    let mut child = unsafe {
        Command::new(cmd)
            .args(args)
            .envs(envs.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .pre_exec(move || {
                // Detach from controlling terminal.
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                // Second fork: intermediate child publishes grandchild PID
                // to the pipe and exits, leaving grandchild reparented to
                // init/launchd. Without this the agent inherits the TUI as
                // its parent and zombifies on TUI exit.
                match libc::fork() {
                    -1 => Err(io::Error::last_os_error()),
                    0 => {
                        // grandchild: close write end and exec the agent.
                        libc::close(write_raw);
                        Ok(())
                    }
                    grandchild_pid => {
                        // intermediate: write grandchild pid out, then vanish.
                        let pid_u32 = grandchild_pid as u32;
                        let bytes = pid_u32.to_le_bytes();
                        let mut written = 0usize;
                        while written < bytes.len() {
                            let n = libc::write(
                                write_raw,
                                bytes.as_ptr().add(written) as *const _,
                                bytes.len() - written,
                            );
                            if n <= 0 {
                                break;
                            }
                            written += n as usize;
                        }
                        libc::_exit(0);
                    }
                }
            })
            .spawn()?
    };

    // Reap the intermediate child. It exited via _exit(0) above (or is about to).
    let _ = child.wait();

    // Read the grandchild PID from the pipe. Drop the write end held by us
    // so the read returns EOF if something went wrong.
    drop(unsafe { OwnedFd::from_raw_fd(write_raw) });
    let mut buf = [0u8; 4];
    let mut f = std::fs::File::from(read_fd);
    f.read_exact(&mut buf)
        .map_err(|e| io::Error::other(format!("read grandchild pid: {e}")))?;
    Ok(u32::from_le_bytes(buf))
}

#[cfg(windows)]
pub fn spawn_detached(cmd: &str, args: &[&str], envs: &EnvPairs) -> io::Result<u32> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    // Constants from windows-sys::Win32::System::Threading
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let child = Command::new(cmd)
        .args(args)
        .envs(envs.iter().map(|(k, v)| (k, v)))
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
pub fn process_alive(pid: u32) -> bool {
    llm_relay_core::process::is_alive(pid)
}
