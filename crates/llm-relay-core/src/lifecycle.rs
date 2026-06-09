//! Mutual-exclusion lifecycle guard shared by GUI and agent.
//!
//! A LifecycleGuard atomically:
//!   1. Acquires an exclusive file lock at `paths::lock_file()`.
//!   2. Binds TCP port `paths::PROXY_PORT` (handed off to the proxy server).
//!   3. Cleans stale `pid_file` / `sock_file` from prior unclean exits.
//!   4. Writes a fresh pidfile.
//!
//! Drop releases the lock and removes pid/sock files.
//!
//! Both the GUI (in `src-tauri`) and the headless agent acquire one of these.
//! The lock is the source of truth for "another LLM Relay process is running";
//! the port bind is a secondary defense (and serves as the listener handed to
//! the proxy server, closing the bind/probe TOCTOU window).

use crate::paths;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::net::{IpAddr, TcpListener};

#[derive(Debug)]
pub enum AcquireError {
    /// Another LLM Relay process (GUI or agent) already holds the lock.
    AlreadyRunning,
    /// Port `paths::PROXY_PORT` is bound by some other process — likely a
    /// previous build, a stale daemon, or an unrelated service. Lock was free.
    PortInUse(std::io::Error),
    /// Other I/O failure (lock file unopenable, pidfile unwritable, …).
    Io(std::io::Error),
}

impl std::fmt::Display for AcquireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRunning => write!(
                f,
                "another LLM Relay process is already running (lock held at {})",
                paths::lock_file().display()
            ),
            Self::PortInUse(e) => write!(
                f,
                "port {} is in use ({}). Another LLM Relay process or service is bound to it.",
                paths::proxy_port(),
                e
            ),
            Self::Io(e) => write!(f, "lifecycle io: {e}"),
        }
    }
}

impl std::error::Error for AcquireError {}

impl From<std::io::Error> for AcquireError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

pub struct LifecycleGuard {
    _lock: File,
    /// Pre-bound TCP listener for the proxy port. Held here so the bind
    /// happens atomically with the file lock — no TOCTOU window between
    /// "probe and drop" and "proxy server binds for real".
    pub proxy_listener: Option<TcpListener>,
    /// Pre-bound listener on the WSL2 virtual NIC IP, when present.
    /// Best-effort: missing WSL adapter, or some other process holding
    /// `<wsl_ip>:18080`, leaves this `None` and the agent stays usable
    /// for Windows-side CLIs.
    pub wsl_listener: Option<(IpAddr, TcpListener)>,
}

impl LifecycleGuard {
    /// Acquire the global lock + bind port + write pidfile.
    pub fn acquire() -> Result<Self, AcquireError> {
        std::fs::create_dir_all(paths::config_dir())?;
        std::fs::create_dir_all(paths::runtime_dir())?;

        // 1. Cross-platform exclusive file lock. This is the canonical
        //    "is another LLM Relay running?" gate. Both GUI and agent
        //    acquire it; whichever loses gets a clean error.
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(paths::lock_file())?;
        if fs2::FileExt::try_lock_exclusive(&lock).is_err() {
            return Err(AcquireError::AlreadyRunning);
        }

        // 2. Bind port atomically with the lock.
        //    NOTE on ordering: `remove_file(sock_file())` happens AFTER the
        //    lock + bind succeed. If another LLM Relay process were still
        //    alive, lock_exclusive() would have failed above and we'd never
        //    reach here, so we won't yank a live agent's socket.
        let proxy_listener = match TcpListener::bind(("127.0.0.1", paths::proxy_port())) {
            Ok(l) => l,
            Err(e) => return Err(AcquireError::PortInUse(e)),
        };

        // 3. Clean stale runtime files from a prior unclean exit.
        let _ = std::fs::remove_file(paths::sock_file());
        let _ = std::fs::remove_file(paths::pid_file());

        // 4. Write fresh pidfile.
        let pid = std::process::id();
        let mut pf = File::create(paths::pid_file())?;
        writeln!(pf, "{pid}")?;

        // 5. Best-effort WSL2 listener bind. Failure here is non-fatal:
        //    no WSL adapter, port already taken on that IP, etc. all
        //    leave the agent fully functional for Windows-side CLIs.
        let wsl_listener = crate::wsl::network::find_wsl_gateway_ip().and_then(|ip| {
            match TcpListener::bind((ip, paths::proxy_port())) {
                Ok(l) => Some((ip, l)),
                Err(e) => {
                    log::warn!(
                        "WSL bind {ip}:{} skipped: {e}",
                        paths::proxy_port()
                    );
                    None
                }
            }
        });

        Ok(Self {
            _lock: lock,
            proxy_listener: Some(proxy_listener),
            wsl_listener,
        })
    }

    pub fn pid(&self) -> u32 {
        std::process::id()
    }

    /// Hand off the pre-bound proxy listener.
    pub fn take_listener(&mut self) -> Option<TcpListener> {
        self.proxy_listener.take()
    }
}

impl Drop for LifecycleGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(paths::pid_file());
        let _ = std::fs::remove_file(paths::sock_file());
        // File lock releases when `_lock` drops.
    }
}

/// Read pidfile and check if the named process is alive.
pub fn live_agent_pid() -> Option<u32> {
    let s = std::fs::read_to_string(paths::pid_file()).ok()?;
    let pid: u32 = s.trim().parse().ok()?;
    if crate::process::is_alive(pid) {
        Some(pid)
    } else {
        None
    }
}

/// Synchronously ask a running agent to shut down via its IPC socket and
/// wait until it releases the lifecycle lock (or `timeout` elapses).
///
/// Returns Ok(()) on confirmed release, Err with a human-readable message
/// otherwise. Used by the GUI when the user confirms "stop daemon and start
/// GUI" — the GUI runs sync code before its tokio runtime exists, so this
/// helper deliberately uses blocking std I/O (no tokio dependency).
pub fn request_agent_stop(timeout: std::time::Duration) -> Result<(), String> {
    use std::io::{Read, Write};

    let sock = paths::sock_file();
    if !sock.exists() {
        return Err(format!(
            "agent socket {} does not exist — is the agent really running?",
            sock.display()
        ));
    }

    // 1. Connect + send a Shutdown request (request_id is arbitrary; the
    //    agent honors Shutdown and we don't need to read the response —
    //    the lock release is the real signal).
    #[cfg(unix)]
    let send_result: Result<(), String> = (|| {
        let mut stream = std::os::unix::net::UnixStream::connect(&sock)
            .map_err(|e| format!("connect agent socket: {e}"))?;
        stream
            .set_write_timeout(Some(std::time::Duration::from_secs(2)))
            .ok();
        let frame = crate::ipc::ClientFrame {
            request_id: 1,
            payload: crate::ipc::Request::Shutdown,
        };
        let body = serde_json::to_vec(&frame).map_err(|e| format!("encode: {e}"))?;
        stream
            .write_all(&(body.len() as u32).to_be_bytes())
            .map_err(|e| format!("write len: {e}"))?;
        stream.write_all(&body).map_err(|e| format!("write body: {e}"))?;
        stream.flush().ok();
        // Drain a few bytes (best effort) so the agent's response write
        // doesn't EPIPE before it processes the request.
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));
        let mut buf = [0u8; 64];
        let _ = stream.read(&mut buf);
        Ok(())
    })();

    #[cfg(windows)]
    let send_result: Result<(), String> = (|| {
        // Windows named pipe — open with std OpenOptions; the agent uses
        // the same byte stream framing.
        use std::os::windows::fs::OpenOptionsExt;
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true).write(true);
        opts.custom_flags(0); // no overlapped — blocking client
        let pipe_name = sock.to_string_lossy().to_string();
        let mut stream = opts
            .open(&pipe_name)
            .map_err(|e| format!("connect agent pipe: {e}"))?;
        let frame = crate::ipc::ClientFrame {
            request_id: 1,
            payload: crate::ipc::Request::Shutdown,
        };
        let body = serde_json::to_vec(&frame).map_err(|e| format!("encode: {e}"))?;
        stream
            .write_all(&(body.len() as u32).to_be_bytes())
            .map_err(|e| format!("write len: {e}"))?;
        stream.write_all(&body).map_err(|e| format!("write body: {e}"))?;
        stream.flush().ok();
        let mut buf = [0u8; 64];
        let _ = stream.read(&mut buf);
        Ok(())
    })();

    send_result?;

    // 2. Wait for the lock to release. We poll `try_lock_exclusive` on the
    //    lock file — when it succeeds, the agent has exited and dropped
    //    its guard. We immediately drop our probe handle so the caller's
    //    LifecycleGuard::acquire() can pick it up.
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(f) = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(paths::lock_file())
        {
            if fs2::FileExt::try_lock_exclusive(&f).is_ok() {
                // Got it — release immediately so caller can re-acquire.
                let _ = fs2::FileExt::unlock(&f);
                drop(f);
                return Ok(());
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "timed out after {:?} waiting for agent to release lock",
                timeout
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
