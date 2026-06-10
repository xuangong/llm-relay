//! HTTP `/_relay/ping` probe of a single URL inside a WSL2 distro.
//!
//! Pure: builds a shell script, runs it via `wsl.exe`, parses stdout.
//! Multi-candidate decision logic (mirror → HDI → hosts injection) lives
//! in [`crate::wsl::resolve`].
//!
//! curl or wget is required for a real HTTP 200 check. `/dev/tcp` only
//! gets used as a best-effort fallback when bash + GNU `timeout` are
//! present *and* the corresponding Relay listener was successfully
//! bound (so a "TCP open" can't be confused with some unrelated process
//! holding the port).

use crate::AppError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The URL responded 200 OK.
    Ok { method: ProbeMethod },
    /// Distro lacks curl/wget, and TCP fallback wasn't eligible. UI
    /// shows a specific "install curl" hint instead of generic
    /// "unreachable".
    NoProbeTool,
    /// Probe attempted but did not succeed (timeout / non-200 / TCP
    /// connect failed).
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeMethod {
    HttpCurl,
    HttpWget,
    /// /dev/tcp: TCP three-way handshake only. No HTTP validation.
    TcpOnly,
}

#[cfg(not(target_os = "windows"))]
pub fn probe_url(_distro: &str, _url: &str, _can_tcp: bool) -> Result<ProbeOutcome, AppError> {
    Err(AppError::Config("probe_url: Windows only".into()))
}

#[cfg(target_os = "windows")]
pub fn probe_url(distro: &str, url: &str, can_tcp: bool) -> Result<ProbeOutcome, AppError> {
    let script = format!(
        r#"probe() {{
  url="$1"
  can_tcp="$2"
  if command -v curl >/dev/null 2>&1; then
    code=$(curl -fsS -o /dev/null -w "%{{http_code}}" --max-time 2 "$url/_relay/ping" 2>/dev/null)
    if [ "$code" = "200" ]; then echo "curl"; return 0; fi
    return 1
  elif command -v wget >/dev/null 2>&1; then
    if wget -q -O /dev/null --timeout=2 --tries=1 "$url/_relay/ping" 2>/dev/null; then
      echo "wget"; return 0
    fi
    return 1
  elif [ "$can_tcp" = "1" ] && command -v bash >/dev/null 2>&1 && command -v timeout >/dev/null 2>&1; then
    host=$(echo "$url" | sed -E "s|http://([^:/]+).*|\1|")
    port=$(echo "$url" | sed -E "s|http://[^:]+:([0-9]+).*|\1|")
    if timeout 2 bash -c "exec 3<>/dev/tcp/$host/$port" 2>/dev/null; then
      echo "tcp"; return 0
    fi
    return 1
  else
    return 2
  fi
}}
if probe "{url}" "{can_tcp}"; then exit 0; fi
rc=$?
if [ "$rc" = "2" ]; then echo "NOTOOL"; exit 3; fi
echo "UNREACH"
exit 1
"#,
        url = url,
        can_tcp = if can_tcp { "1" } else { "0" },
    );
    let stdout = match crate::wsl::fs::__wsl_run_script_capture(distro, &script) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("probe_url({distro}, {url}): {e}");
            return Ok(ProbeOutcome::Unreachable);
        }
    };
    Ok(parse_probe_outcome(&stdout))
}

fn parse_probe_outcome(stdout: &str) -> ProbeOutcome {
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    let last = lines.last().copied().unwrap_or("");
    if last == "NOTOOL" {
        return ProbeOutcome::NoProbeTool;
    }
    if last == "UNREACH" {
        return ProbeOutcome::Unreachable;
    }
    match last {
        "curl" => ProbeOutcome::Ok { method: ProbeMethod::HttpCurl },
        "wget" => ProbeOutcome::Ok { method: ProbeMethod::HttpWget },
        "tcp" => ProbeOutcome::Ok { method: ProbeMethod::TcpOnly },
        _ => ProbeOutcome::Unreachable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ok_curl() {
        assert_eq!(
            parse_probe_outcome("curl\n"),
            ProbeOutcome::Ok { method: ProbeMethod::HttpCurl },
        );
    }

    #[test]
    fn parse_ok_wget() {
        assert_eq!(
            parse_probe_outcome("wget\n"),
            ProbeOutcome::Ok { method: ProbeMethod::HttpWget },
        );
    }

    #[test]
    fn parse_ok_tcp() {
        assert_eq!(
            parse_probe_outcome("tcp\n"),
            ProbeOutcome::Ok { method: ProbeMethod::TcpOnly },
        );
    }

    #[test]
    fn parse_notool() {
        assert_eq!(parse_probe_outcome("NOTOOL\n"), ProbeOutcome::NoProbeTool);
    }

    #[test]
    fn parse_unreach() {
        assert_eq!(parse_probe_outcome("UNREACH\n"), ProbeOutcome::Unreachable);
    }
}
