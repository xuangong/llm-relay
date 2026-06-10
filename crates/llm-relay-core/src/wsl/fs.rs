//! File I/O inside a WSL2 distro via `wsl.exe -d <D> -e sh -c ...`.
//!
//! Used by `WslBackend` so all snapshot/apply logic in `config_writer`
//! works against per-distro Linux filesystems with the same shape as the
//! native Windows backend. A stopped or unregistered distro surfaces as
//! `AppError::Config` so the caller can warn-and-skip per target.

use crate::AppError;

#[cfg(target_os = "windows")]
use std::io::Write;
#[cfg(target_os = "windows")]
use std::process::{Command, Stdio};

#[cfg(target_os = "windows")]
const WSL_TIMEOUT_SECS: u64 = 5;

#[cfg(not(target_os = "windows"))]
pub fn wsl_read(_distro: &str, _path: &str) -> Result<Option<String>, AppError> {
    Err(AppError::Config(
        "WSL fs ops only available on Windows".into(),
    ))
}
#[cfg(not(target_os = "windows"))]
pub fn wsl_atomic_write(_distro: &str, _path: &str, _bytes: &[u8]) -> Result<(), AppError> {
    Err(AppError::Config(
        "WSL fs ops only available on Windows".into(),
    ))
}
#[cfg(not(target_os = "windows"))]
pub fn wsl_remove(_distro: &str, _path: &str) -> Result<(), AppError> {
    Err(AppError::Config(
        "WSL fs ops only available on Windows".into(),
    ))
}
#[cfg(not(target_os = "windows"))]
pub fn wsl_exists(_distro: &str, _path: &str) -> Result<bool, AppError> {
    Err(AppError::Config(
        "WSL fs ops only available on Windows".into(),
    ))
}

#[cfg(target_os = "windows")]
pub fn wsl_read(distro: &str, path: &str) -> Result<Option<String>, AppError> {
    // `[ -f X ] && cat X` distinguishes "file absent" (empty stdout, exit 0)
    // from "distro broken" (non-zero exit). The exists() round-trip below
    // disambiguates "absent" vs "empty file".
    let script = format!(
        "if [ -f {path} ]; then cat {path}; fi",
        path = shell_escape(path),
    );
    let out = run_wsl(distro, &script, None)?;
    if out.is_empty() {
        if wsl_exists(distro, path)? {
            Ok(Some(String::new()))
        } else {
            Ok(None)
        }
    } else {
        Ok(Some(String::from_utf8_lossy(&out).into_owned()))
    }
}

#[cfg(target_os = "windows")]
pub fn wsl_atomic_write(distro: &str, path: &str, bytes: &[u8]) -> Result<(), AppError> {
    // Inside the distro: mkdir parent → mktemp → cat stdin → mv -f.
    // Atomic rename on the same filesystem; stdin avoids any shell escaping
    // for the content.
    let script = format!(
        r#"umask 077
d="$(dirname {path})"
mkdir -p "$d"
t="$(mktemp "$d/.llmrelay.tmp.XXXXXX")"
cat > "$t"
mv -f "$t" {path}"#,
        path = shell_escape(path),
    );
    let _ = run_wsl(distro, &script, Some(bytes))?;
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn wsl_remove(distro: &str, path: &str) -> Result<(), AppError> {
    let script = format!("rm -f {path}", path = shell_escape(path));
    let _ = run_wsl(distro, &script, None)?;
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn wsl_exists(distro: &str, path: &str) -> Result<bool, AppError> {
    let script = format!(
        "if [ -e {path} ]; then echo 1; else echo 0; fi",
        path = shell_escape(path),
    );
    let out = run_wsl(distro, &script, None)?;
    Ok(String::from_utf8_lossy(&out).trim() == "1")
}

/// Run a shell script inside a distro and return stdout. Used by the four
/// public APIs and by `wsl::distro::probe_distro` (which needs structured
/// stdout from a custom script).
#[cfg(target_os = "windows")]
pub(crate) fn __wsl_run_script(distro: &str, script: &str) -> Result<String, AppError> {
    let bytes = run_wsl(distro, script, None)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Run a script and return its stdout regardless of exit status. Used by
/// `probe::probe_url_for_distro`, where the script signals UNREACH/NOTOOL
/// via stdout + non-zero exit and we MUST read the stdout. Returns Err
/// only on real spawn / timeout / IO failures.
#[cfg(target_os = "windows")]
pub(crate) fn __wsl_run_script_capture(distro: &str, script: &str) -> Result<String, AppError> {
    let bytes = run_wsl_capture(distro, script)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(target_os = "windows")]
fn run_wsl_capture(distro: &str, script: &str) -> Result<Vec<u8>, AppError> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut cmd = Command::new("wsl.exe");
    cmd.args(["-d", distro, "-e", "sh", "-c", script]);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.stdin(Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::Config(format!("wsl.exe spawn ({distro}): {e}")))?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed().as_secs() >= WSL_TIMEOUT_SECS {
                    let _ = child.kill();
                    return Err(AppError::Config(format!(
                        "wsl.exe -d {distro} timed out after {WSL_TIMEOUT_SECS}s"
                    )));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(AppError::Config(format!("wsl wait: {e}"))),
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| AppError::Config(format!("wsl output: {e}")))?;
    Ok(out.stdout)
}

#[cfg(target_os = "windows")]
fn run_wsl(distro: &str, script: &str, stdin_bytes: Option<&[u8]>) -> Result<Vec<u8>, AppError> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut cmd = Command::new("wsl.exe");
    cmd.args(["-d", distro, "-e", "sh", "-c", script]);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    if stdin_bytes.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::Config(format!("wsl.exe spawn ({distro}): {e}")))?;
    if let (Some(bytes), Some(mut stdin)) = (stdin_bytes, child.stdin.take()) {
        stdin
            .write_all(bytes)
            .map_err(|e| AppError::Config(format!("wsl stdin: {e}")))?;
        drop(stdin);
    }
    // Manual timeout: wait_timeout would be nicer but introduces a new dep.
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                if start.elapsed().as_secs() >= WSL_TIMEOUT_SECS {
                    let _ = child.kill();
                    return Err(AppError::Config(format!(
                        "wsl.exe -d {distro} timed out after {WSL_TIMEOUT_SECS}s"
                    )));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(AppError::Config(format!("wsl wait: {e}"))),
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| AppError::Config(format!("wsl output: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::Config(format!(
            "wsl.exe -d {distro} exited with {}: {stderr}",
            out.status
        )));
    }
    Ok(out.stdout)
}

#[cfg(target_os = "windows")]
fn shell_escape(s: &str) -> String {
    // Single-quote for sh; escape inner single quotes as '\''.
    let escaped = s.replace('\'', r#"'\''"#);
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn unknown_distro_returns_err_not_panic() {
        let r = wsl_exists("__definitely_not_a_real_distro__", "/tmp/foo");
        assert!(r.is_err(), "expected err, got {r:?}");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_returns_err() {
        assert!(wsl_read("anything", "/tmp/x").is_err());
        assert!(wsl_atomic_write("anything", "/tmp/x", b"").is_err());
        assert!(wsl_remove("anything", "/tmp/x").is_err());
        assert!(wsl_exists("anything", "/tmp/x").is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn shell_escape_handles_single_quotes() {
        assert_eq!(shell_escape("a'b"), r#"'a'\''b'"#);
        assert_eq!(shell_escape("/home/x/foo bar"), "'/home/x/foo bar'");
    }
}
