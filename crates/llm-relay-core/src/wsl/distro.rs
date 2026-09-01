//! Distro discovery + probe + SQLite cache. See parent module for overview.

use crate::AppError;

#[derive(Debug, Clone)]
pub struct DistroRow {
    pub name: String,
    pub is_default: bool,
    pub selected: bool,
    pub home: Option<String>,
    pub user: Option<String>,
    pub has_claude: bool,
    pub has_codex: bool,
    pub has_gemini: bool,
    pub resolved_url: Option<String>,
    pub probed_at: Option<String>,
}

/// One row of `wsl.exe -l -v` output, after WSL1 entries are filtered out.
#[derive(Debug, Clone)]
pub struct DiscoveredDistro {
    pub name: String,
    pub is_default: bool,
    pub running: bool,
}

/// Returns every WSL2 distro known to the system, or an empty Vec when WSL
/// is absent / disabled / has no installed distros. Never an Err for those
/// cases — the UI surfaces "no distros" rather than an error toast.
#[cfg(not(target_os = "windows"))]
pub fn discover_distros() -> Result<Vec<DiscoveredDistro>, AppError> {
    Ok(Vec::new())
}

#[cfg(target_os = "windows")]
pub fn discover_distros() -> Result<Vec<DiscoveredDistro>, AppError> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let out = match Command::new("wsl.exe")
        .args(["-l", "-v"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(AppError::Config(format!("wsl.exe -l -v: {e}"))),
    };
    let text = decode_wsl_output(&out.stdout);
    if !out.status.success() {
        // "no installed distributions" prints to stdout with exit != 0 on
        // some builds. Return empty rather than error.
        log::debug!("wsl.exe -l -v exited {}; stdout: {}", out.status, text);
        return Ok(Vec::new());
    }
    Ok(parse_wsl_list(&text))
}

#[cfg(target_os = "windows")]
fn decode_wsl_output(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        // UTF-16LE BOM.
        let u16s: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&u16s);
    }
    if bytes.len() % 2 == 0
        && bytes.len() >= 2
        && bytes
            .iter()
            .enumerate()
            .filter(|(i, _)| i % 2 == 1)
            .all(|(_, b)| *b == 0)
    {
        // No BOM but every odd byte is 0 — almost certainly UTF-16LE.
        let u16s: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&u16s);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// Parse the columnar text of `wsl -l -v`. Header on line 1:
///   "  NAME       STATE    VERSION"
///   "* Ubuntu     Running  2"
///   "  Debian     Stopped  2"
///   "  Legacy     Stopped  1"   ← filtered (WSL1)
///
/// Only reachable from the Windows `list_distros`; the parser itself is
/// platform-independent, so keep it compiled for tests everywhere.
#[cfg(any(target_os = "windows", test))]
fn parse_wsl_list(text: &str) -> Vec<DiscoveredDistro> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim_end();
        if i == 0 || trimmed.is_empty() {
            continue;
        }
        let is_default = trimmed.starts_with('*');
        let rest = trimmed.trim_start_matches('*').trim_start();
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let name = parts[0].to_string();
        let state = parts[1];
        let version = parts[2];
        if version != "2" {
            continue;
        }
        out.push(DiscoveredDistro {
            name,
            is_default,
            running: state.eq_ignore_ascii_case("Running"),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typical_output() {
        let text = "  NAME       STATE    VERSION\n\
                    * Ubuntu     Running  2\n\
                      Debian     Stopped  2\n\
                      Legacy     Stopped  1\n";
        let got = parse_wsl_list(text);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "Ubuntu");
        assert!(got[0].is_default);
        assert!(got[0].running);
        assert_eq!(got[1].name, "Debian");
        assert!(!got[1].is_default);
        assert!(!got[1].running);
    }

    #[test]
    fn empty_output_yields_empty() {
        assert!(parse_wsl_list("").is_empty());
        assert!(parse_wsl_list("  NAME STATE VERSION\n").is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn decode_handles_utf16_bom() {
        let mut bytes = vec![0xFF, 0xFE];
        for c in "hello".encode_utf16() {
            bytes.extend_from_slice(&c.to_le_bytes());
        }
        assert_eq!(decode_wsl_output(&bytes), "hello");
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProbeResult {
    pub home: Option<String>,
    pub user: Option<String>,
    pub has_claude: bool,
    pub has_codex: bool,
    pub has_gemini: bool,
}

/// Probe a single distro for $HOME, whoami, and presence of the three CLI
/// binaries. Single `wsl.exe` invocation to amortize cold-start cost. The
/// per-binary loop avoids `&&` short-circuiting (a missing claude would
/// otherwise mask later codex/gemini results).
///
/// Detection runs under the **user's $SHELL with `-ic`** (interactive shell)
/// because user-level package managers (linuxbrew, asdf, nvm, npm-global)
/// add their PATH only via `.bashrc` / `.zshrc`, which a login shell
/// (`bash -lc`) does NOT source. Interactive shells in non-tty contexts
/// still source their rc files, which is exactly what we need to see PATH
/// the way the user actually does. Detection is forgiving — we only look
/// for `<cli>=1` lines, so stray rc-file output is ignored.
#[cfg(target_os = "windows")]
pub fn probe_distro(name: &str) -> Result<ProbeResult, AppError> {
    // Outer `sh` runs the user's interactive shell via $SHELL -ic (so .bashrc
    // / .zshrc are sourced and brew/asdf/nvm/etc PATH adjustments apply).
    // Single-quoted heredoc to the inner shell so no host-side substitution.
    let script = r#"$SHELL -ic '
echo "home=$HOME"
echo "user=$(whoami)"
for c in claude codex gemini; do
  if command -v "$c" >/dev/null 2>&1; then
    echo "$c=1"
  else
    echo "$c=0"
  fi
done'"#;
    let out = crate::wsl::fs::__wsl_run_script(name, script)?;
    Ok(parse_probe_output(&out))
}

#[cfg(not(target_os = "windows"))]
pub fn probe_distro(_name: &str) -> Result<ProbeResult, AppError> {
    Err(AppError::Config("probe_distro: Windows only".into()))
}

/// The distro user's login shell, e.g. `/usr/bin/zsh`.
///
/// Read from passwd rather than `$SHELL`: we reach the distro through
/// `wsl.exe -e sh -c`, and `$SHELL` there reflects whatever WSL happened to
/// export, not the shell the user's own terminal starts. Not cached — this
/// runs once per apply, and a distro that changed its shell should not need
/// an app restart to be written correctly.
#[cfg(target_os = "windows")]
pub fn login_shell(name: &str) -> Option<String> {
    let out = crate::wsl::fs::__wsl_run_script(name, r#"getent passwd "$(id -u)" | cut -d: -f7"#)
        .map_err(|e| log::warn!("could not read login shell for {name}: {e}"))
        .ok()?;
    let shell = out.trim().to_string();
    if shell.is_empty() {
        None
    } else {
        Some(shell)
    }
}

#[cfg(not(target_os = "windows"))]
pub fn login_shell(_name: &str) -> Option<String> {
    None
}

/// Only reachable from the Windows `probe_distro`; the parser itself is
/// platform-independent, so keep it compiled for tests everywhere.
#[cfg(any(target_os = "windows", test))]
fn parse_probe_output(text: &str) -> ProbeResult {
    let mut r = ProbeResult::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k.trim() {
            "home" => r.home = Some(v.trim().to_string()).filter(|s| !s.is_empty()),
            "user" => r.user = Some(v.trim().to_string()).filter(|s| !s.is_empty()),
            "claude" => r.has_claude = v.trim() == "1",
            "codex" => r.has_codex = v.trim() == "1",
            "gemini" => r.has_gemini = v.trim() == "1",
            _ => {}
        }
    }
    r
}

#[cfg(test)]
mod probe_tests {
    use super::*;

    #[test]
    fn parses_complete_output() {
        let text = "home=/home/xanzh\nuser=xanzh\nclaude=1\ncodex=0\ngemini=1\n";
        let r = parse_probe_output(text);
        assert_eq!(r.home.as_deref(), Some("/home/xanzh"));
        assert_eq!(r.user.as_deref(), Some("xanzh"));
        assert!(r.has_claude);
        assert!(!r.has_codex);
        assert!(r.has_gemini);
    }

    #[test]
    fn missing_values_default_to_false_none() {
        let r = parse_probe_output("");
        assert!(r.home.is_none());
        assert!(!r.has_claude);
    }
}

/// Re-discover distros from `wsl.exe`, run `probe_distro` for each, and
/// reconcile into the `wsl_distros` table:
/// - new distro → insert with `selected = is_default`
/// - existing distro → refresh home/user/installed/probed_at, preserve
///   user-set `selected` flag and existing `resolved_url` (URL probing
///   is a separate step done by `wsl::probe`)
/// - distro removed from discovery → delete row
///
/// Probe failures for an individual distro do NOT abort reconciliation;
/// the row is still upserted so the UI can show "found, not probed".
pub fn refresh_distros_in_db(db: &crate::Database) -> Result<Vec<DistroRow>, AppError> {
    let discovered = discover_distros()?;
    let existing = db.list_wsl_distros()?;
    let now = chrono::Utc::now().to_rfc3339();

    use std::collections::HashSet;
    let discovered_names: HashSet<&str> = discovered.iter().map(|d| d.name.as_str()).collect();
    for ex in &existing {
        if !discovered_names.contains(ex.name.as_str()) {
            log::info!("WSL distro removed: {}", ex.name);
            db.delete_wsl_distro(&ex.name)?;
        }
    }

    let mut out = Vec::with_capacity(discovered.len());
    for d in discovered {
        let prior = existing.iter().find(|e| e.name == d.name);
        let probe = match probe_distro(&d.name) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("probe_distro({}) failed: {e}", d.name);
                ProbeResult::default()
            }
        };
        let row = DistroRow {
            name: d.name.clone(),
            is_default: d.is_default,
            selected: prior.map(|p| p.selected).unwrap_or(d.is_default),
            home: probe.home.or_else(|| prior.and_then(|p| p.home.clone())),
            user: probe.user.or_else(|| prior.and_then(|p| p.user.clone())),
            has_claude: probe.has_claude,
            has_codex: probe.has_codex,
            has_gemini: probe.has_gemini,
            resolved_url: prior.and_then(|p| p.resolved_url.clone()),
            probed_at: Some(now.clone()),
        };
        db.upsert_wsl_distro(&row)?;
        out.push(row);
    }
    Ok(out)
}
