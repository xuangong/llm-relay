//! Agent lifecycle: file lock + pidfile + port probe + cleanup.

use anyhow::{anyhow, Context, Result};
use llm_relay_core::paths;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::net::TcpListener;

pub struct LifecycleGuard {
    _lock: File,
    /// Pre-bound TCP listener for the proxy port. Held here so the bind
    /// happens atomically with the file lock — no TOCTOU window between
    /// "probe and drop" and "proxy server binds for real".
    pub proxy_listener: Option<TcpListener>,
}

impl LifecycleGuard {
    /// Acquire the agent lock + bind port + write pidfile.
    /// Returns a guard whose Drop releases the lock and removes pid/sock files.
    /// `take_listener()` transfers ownership of the bound proxy port to the caller.
    pub fn acquire() -> Result<Self> {
        std::fs::create_dir_all(paths::config_dir())?;

        // 1. Try the cross-platform exclusive lock.
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(paths::lock_file())
            .with_context(|| format!("open {}", paths::lock_file().display()))?;
        fs2::FileExt::try_lock_exclusive(&lock)
            .map_err(|_| anyhow!("another llm-relay-agent already holds {}", paths::lock_file().display()))?;

        // 2. Bind port 18080 NOW and keep the listener alive for hand-off.
        // This closes the TOCTOU window where another process could grab the
        // port between a bind+drop probe and the proxy server's later bind.
        let proxy_listener = match TcpListener::bind(("127.0.0.1", paths::PROXY_PORT)) {
            Ok(l) => l,
            Err(e) => return Err(anyhow!(
                "port {} in use ({}). Is the GUI running? Stop it first.",
                paths::PROXY_PORT, e
            )),
        };

        // 3. Remove stale socket file from a previous unclean exit.
        let _ = std::fs::remove_file(paths::sock_file());

        // 4. Write pidfile.
        let pid = std::process::id();
        let mut pf = File::create(paths::pid_file())?;
        writeln!(pf, "{pid}")?;

        Ok(Self { _lock: lock, proxy_listener: Some(proxy_listener) })
    }

    pub fn pid(&self) -> u32 { std::process::id() }

    /// Hand off the pre-bound proxy listener to the proxy server.
    pub fn take_listener(&mut self) -> Option<TcpListener> {
        self.proxy_listener.take()
    }
}

impl Drop for LifecycleGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(paths::pid_file());
        let _ = std::fs::remove_file(paths::sock_file());
        // Lock is released when File drops.
    }
}

/// Read pidfile and check if the named process is alive.
/// Returns Some(pid) only if the process exists.
pub fn live_agent_pid() -> Option<u32> {
    let s = std::fs::read_to_string(paths::pid_file()).ok()?;
    let pid: u32 = s.trim().parse().ok()?;
    if process_alive(pid) { Some(pid) } else { None }
}

#[cfg(unix)]
pub fn process_alive(pid: u32) -> bool {
    // signal 0 = check existence
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
pub fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() { false } else { CloseHandle(h); true }
    }
}
