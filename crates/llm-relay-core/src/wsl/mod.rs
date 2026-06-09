//! WSL2 integration. All cross-platform stubs here; Windows-only impls in
//! submodules gated by `#[cfg(target_os = "windows")]`.
pub mod distro;
pub mod fs;
pub mod network;
pub mod probe;
