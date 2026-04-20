//! Cross-platform local socket transport.
//! Unix: socket file at the given path.
//! Windows: named pipe whose name is derived from the path's file_name.

use interprocess::local_socket::{
    tokio::{prelude::*, Stream, Listener},
    GenericFilePath, GenericNamespaced, ListenerOptions, ToFsName, ToNsName,
};
use std::io;
use std::path::Path;

/// Build a listener bound to the given socket path (Unix) or namespaced name (Windows).
pub fn build_listener(path: &Path) -> io::Result<Listener> {
    #[cfg(unix)]
    {
        // If a stale socket file exists, remove it first.
        if path.exists() { let _ = std::fs::remove_file(path); }
        let name = path.to_fs_name::<GenericFilePath>()?;
        ListenerOptions::new().name(name).create_tokio()
    }
    #[cfg(windows)]
    {
        let pipe_name = format!(
            r"llm-relay-agent-{}",
            path.file_stem().and_then(|s| s.to_str()).unwrap_or("default")
        );
        let name = pipe_name.to_ns_name::<GenericNamespaced>()?;
        ListenerOptions::new().name(name).create_tokio()
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
