//! Abstraction layer that lets `config_writer` operate identically against
//! Windows-native paths and per-distro Linux filesystems via `wsl.exe`.
//!
//! Backends only cover filesystem ops; per-target metadata (base_url,
//! installed tools, snapshot identity) lives on `CliTarget` so the
//! same `CliBackend` impl can serve multiple distros / targets.

use crate::AppError;
use std::path::PathBuf;

pub trait CliBackend: Send + Sync {
    fn root_hint(&self) -> Option<String> {
        None
    }
    fn read_bytes(&self, rel_path: &[&str]) -> Result<Option<Vec<u8>>, AppError>;
    fn read(&self, rel_path: &[&str]) -> Result<Option<String>, AppError> {
        self.read_bytes(rel_path)?
            .map(|bytes| {
                String::from_utf8(bytes).map_err(|error| {
                    AppError::Config(format!(
                        "CLI config {} is not valid UTF-8: {error}",
                        rel_path.join("/")
                    ))
                })
            })
            .transpose()
    }
    fn write_atomic(&self, rel_path: &[&str], bytes: &[u8]) -> Result<(), AppError>;
    fn remove(&self, rel_path: &[&str]) -> Result<(), AppError>;
    fn exists(&self, rel_path: &[&str]) -> Result<bool, AppError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetType {
    Windows,
    Wsl,
}

#[derive(Debug, Clone)]
pub struct SnapshotMeta {
    pub target_type: TargetType,
    /// None for Windows; Some(<original wsl -d name>) for WSL targets.
    /// The original name (with spaces, capitalization, whatever) is
    /// what gets passed to wsl.exe at restore time.
    pub distro_name: Option<String>,
    /// For WSL: the probed `$HOME`. Stored in the snapshot JSON so a
    /// later restore doesn't need to re-probe a possibly-stopped distro.
    pub home: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedClients {
    pub claude: bool,
    pub codex: bool,
    pub gemini: bool,
}

impl ManagedClients {
    pub const CODEX_ONLY: Self = Self {
        claude: false,
        codex: true,
        gemini: false,
    };
    pub const ALL: Self = Self {
        claude: true,
        codex: true,
        gemini: true,
    };

    pub fn any(self) -> bool {
        self.claude || self.codex || self.gemini
    }

    pub fn intersect(self, installed: InstalledTools) -> InstalledTools {
        InstalledTools {
            claude: self.claude && installed.claude,
            codex: self.codex && installed.codex,
            gemini: self.gemini && installed.gemini,
        }
    }
}

impl Default for ManagedClients {
    fn default() -> Self {
        Self::ALL
    }
}

#[derive(Debug, Clone, Copy)]
pub struct InstalledTools {
    pub claude: bool,
    pub codex: bool,
    pub gemini: bool,
}

impl InstalledTools {
    /// Windows always treats all three as "installed" — config writes
    /// happen unconditionally there, matching pre-WSL2 behavior.
    pub const ALL: Self = Self {
        claude: true,
        codex: true,
        gemini: true,
    };
}

pub struct CliTarget {
    pub backend: Box<dyn CliBackend>,
    pub base_url: String,
    pub installed: InstalledTools,
    /// Display label, e.g. "windows" / "wsl:Ubuntu". Used for logging.
    pub label: String,
    pub snapshot_meta: SnapshotMeta,
}

/// Writes against the Windows user's home directory.
pub struct WindowsFsBackend {
    pub(crate) home: PathBuf,
}

impl WindowsFsBackend {
    pub fn new() -> Self {
        Self {
            home: dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
        }
    }

    fn full_path(&self, rel: &[&str]) -> PathBuf {
        let mut p = self.home.clone();
        for seg in rel {
            p.push(seg);
        }
        p
    }
}

impl Default for WindowsFsBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CliBackend for WindowsFsBackend {
    fn root_hint(&self) -> Option<String> {
        Some(self.home.to_string_lossy().into_owned())
    }
    fn read_bytes(&self, rel: &[&str]) -> Result<Option<Vec<u8>>, AppError> {
        let p = self.full_path(rel);
        if !p.exists() {
            return Ok(None);
        }
        Ok(Some(std::fs::read(p)?))
    }
    fn write_atomic(&self, rel: &[&str], bytes: &[u8]) -> Result<(), AppError> {
        let p = self.full_path(rel);
        atomic_write(&p, bytes)
    }
    fn remove(&self, rel: &[&str]) -> Result<(), AppError> {
        let p = self.full_path(rel);
        if p.exists() {
            std::fs::remove_file(p)?;
        }
        Ok(())
    }
    fn exists(&self, rel: &[&str]) -> Result<bool, AppError> {
        Ok(self.full_path(rel).exists())
    }
}

/// Atomic write: write to a temp file, then rename. Same logic as the
/// private fn that lived in `config_writer.rs` — moved here so the
/// trait impl doesn't depend on it being inside the writer module.
pub fn atomic_write(path: &std::path::Path, content: &[u8]) -> Result<(), AppError> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let file_name = path
        .file_name()
        .ok_or_else(|| AppError::Config("invalid file name".into()))?
        .to_string_lossy();
    let mut tmp = path
        .parent()
        .ok_or_else(|| AppError::Config("invalid path".into()))?
        .to_path_buf();
    tmp.push(format!("{}.tmp.{}", file_name, ts));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content)?;
        f.flush()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let perm = meta.permissions().mode();
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(perm));
        }
    }
    #[cfg(windows)]
    {
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn windows_backend_round_trip() {
        let tmp = TempDir::new().unwrap();
        let b = WindowsFsBackend {
            home: tmp.path().to_path_buf(),
        };
        assert!(!b.exists(&[".claude", "settings.json"]).unwrap());
        b.write_atomic(&[".claude", "settings.json"], b"{}")
            .unwrap();
        assert!(b.exists(&[".claude", "settings.json"]).unwrap());
        assert_eq!(
            b.read(&[".claude", "settings.json"]).unwrap().as_deref(),
            Some("{}"),
        );
        b.remove(&[".claude", "settings.json"]).unwrap();
        assert_eq!(b.read(&[".claude", "settings.json"]).unwrap(), None);
    }
}

/// Backend that reads/writes via `wsl.exe` inside a specific distro.
/// `home` is the probed `$HOME`; relative paths are joined under it.
pub struct WslBackend {
    pub distro: String,
    pub home: String,
}

impl WslBackend {
    fn full_path(&self, rel: &[&str]) -> String {
        let mut s = self.home.clone();
        for seg in rel {
            if !s.ends_with('/') {
                s.push('/');
            }
            s.push_str(seg);
        }
        s
    }
}

impl CliBackend for WslBackend {
    fn read_bytes(&self, rel: &[&str]) -> Result<Option<Vec<u8>>, AppError> {
        crate::wsl::fs::wsl_read_bytes(&self.distro, &self.full_path(rel))
    }
    fn write_atomic(&self, rel: &[&str], bytes: &[u8]) -> Result<(), AppError> {
        crate::wsl::fs::wsl_atomic_write(&self.distro, &self.full_path(rel), bytes)
    }
    fn remove(&self, rel: &[&str]) -> Result<(), AppError> {
        crate::wsl::fs::wsl_remove(&self.distro, &self.full_path(rel))
    }
    fn exists(&self, rel: &[&str]) -> Result<bool, AppError> {
        crate::wsl::fs::wsl_exists(&self.distro, &self.full_path(rel))
    }
}

#[cfg(test)]
mod wsl_tests {
    use super::*;

    #[test]
    fn wsl_backend_path_join() {
        let b = WslBackend {
            distro: "Ubuntu".into(),
            home: "/home/x".into(),
        };
        assert_eq!(
            b.full_path(&[".claude", "settings.json"]),
            "/home/x/.claude/settings.json"
        );
        let b2 = WslBackend {
            distro: "Ubuntu".into(),
            home: "/home/x/".into(),
        };
        assert_eq!(
            b2.full_path(&[".claude", "settings.json"]),
            "/home/x/.claude/settings.json"
        );
    }
}
