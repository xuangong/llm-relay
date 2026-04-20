//! Cross-platform local socket transport.
//! Unix: socket file at the given path.
//! Windows: named pipe whose name is derived from the path's file_name.

use interprocess::local_socket::{
    tokio::{prelude::*, Stream, Listener},
    ListenerOptions,
};
#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, ToFsName};
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ToNsName};
use std::io;
use std::path::Path;

/// Build a listener bound to the given socket path (Unix) or namespaced name (Windows).
///
/// Security: on Unix the socket file is chmod'd to 0o600 immediately after bind so only
/// the owning user can connect. On Windows we cannot directly attach a custom DACL via
/// the `interprocess` crate's current API surface; the named pipe inherits the default
/// DACL which permits local interactive users. The agent enforces caller identity by
/// checking the peer process owner after `accept` (see `ipc_server::handle_conn`).
pub fn build_listener(path: &Path) -> io::Result<Listener> {
    #[cfg(unix)]
    {
        // If a stale socket file exists, remove it first.
        if path.exists() { let _ = std::fs::remove_file(path); }
        let name = path.to_fs_name::<GenericFilePath>()?;
        let listener = ListenerOptions::new().name(name).create_tokio()?;
        // Restrict the socket file so only the owning user (UID) can connect.
        // On Linux the kernel enforces filesystem perms on AF_UNIX sockets.
        use std::fs::Permissions;
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(path, Permissions::from_mode(0o600)) {
            log::warn!("failed to chmod 0600 socket {}: {}", path.display(), e);
        }
        Ok(listener)
    }
    #[cfg(windows)]
    {
        // NOTE: `interprocess` does not currently expose a hook to install a custom
        // SECURITY_ATTRIBUTES / DACL on the named pipe. The agent therefore enforces
        // owner-only access in software by checking the client process owner after
        // accept (see `ipc_server::handle_conn`).
        let pipe_name = format!(
            r"llm-relay-agent-{}",
            path.file_stem().and_then(|s| s.to_str()).unwrap_or("default")
        );
        let name = pipe_name.to_ns_name::<GenericNamespaced>()?;
        ListenerOptions::new().name(name).create_tokio()
    }
}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn unix_socket_is_chmod_0600() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.sock");
        let _listener = build_listener(&path).expect("listener");
        let meta = std::fs::metadata(&path).expect("metadata");
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "socket perms should be 0600, got {:o}", mode);
    }
}

pub async fn connect(path: &Path) -> io::Result<Stream> {
    #[cfg(unix)]
    {
        let name = path.to_fs_name::<GenericFilePath>()?;
        Stream::connect(name).await
    }
    #[cfg(windows)]
    {
        let pipe_name = format!(
            r"llm-relay-agent-{}",
            path.file_stem().and_then(|s| s.to_str()).unwrap_or("default")
        );
        let name = pipe_name.to_ns_name::<GenericNamespaced>()?;
        Stream::connect(name).await
    }
}
