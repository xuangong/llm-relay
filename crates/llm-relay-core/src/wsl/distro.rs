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
