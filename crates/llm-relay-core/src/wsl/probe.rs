//! Per-distro URL probe. Decides which `base_url` gets written to the
//! WSL2 CLI configs:
//!   - `host.docker.internal:18080` (NAT or mirror, when not hijacked)
//!   - `<wsl_gateway_ip>:18080` (NAT, fallback when host.docker.internal
//!     is hijacked by Docker Desktop or similar — common case)
//!   - `127.0.0.1:18080` (mirror only)
//!
//! curl or wget is required for a real HTTP 200 check. `/dev/tcp` only
//! gets used as a best-effort fallback when bash + GNU `timeout` are
//! present *and* the corresponding Relay listener was successfully
//! bound (so a "TCP open" can't be confused with some unrelated process
//! holding the port).

use crate::AppError;
use std::net::IpAddr;

#[derive(Debug, Clone, Copy)]
pub struct ListenerBinds {
    /// 127.0.0.1 listener is mandatory and always bound when the proxy
    /// is up.
    pub loopback: bool,
    /// host.docker.internal target: TRUE only if the WSL gateway IP
    /// listener was bound. Gates the TCP-only fallback for that URL so
    /// we don't accept "port open by someone else" as Relay reachable.
    pub host_docker_internal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    Ok { url: String, method: ProbeMethod },
    /// Distro lacks curl/wget, and TCP fallback wasn't eligible. UI
    /// shows a specific "install curl" hint instead of generic
    /// "unreachable".
    NoProbeTool,
    /// All candidates failed.
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
pub fn probe_url_for_distro(
    _distro: &str,
    _binds: ListenerBinds,
    _gateway_ip: Option<IpAddr>,
) -> Result<ProbeOutcome, AppError> {
    Err(AppError::Config(
        "probe_url_for_distro: Windows only".into(),
    ))
}

#[cfg(target_os = "windows")]
pub fn probe_url_for_distro(
    distro: &str,
    binds: ListenerBinds,
    gateway_ip: Option<IpAddr>,
) -> Result<ProbeOutcome, AppError> {
    let port = crate::paths::proxy_port();
    // Build candidate list. host.docker.internal is preferred when it
    // resolves to our gateway (the stable case), but Docker Desktop's
    // /etc/hosts entry hijacks it to a LAN IP that we deliberately do NOT
    // bind, so the gateway IP candidate must run too. Order: HDI →
    // gateway IP → 127.0.0.1.
    let mut candidates: Vec<(String, &'static str, &'static str)> = Vec::new();
    candidates.push((
        format!("http://host.docker.internal:{port}"),
        if binds.host_docker_internal { "1" } else { "0" },
        "hdi",
    ));
    if let Some(ip) = gateway_ip {
        candidates.push((
            format!("http://{ip}:{port}"),
            if binds.host_docker_internal { "1" } else { "0" },
            "gw",
        ));
    }
    candidates.push((
        format!("http://127.0.0.1:{port}"),
        if binds.loopback { "1" } else { "0" },
        "lo",
    ));

    let mut probe_calls = String::new();
    let mut tail = String::new();
    for (i, (url, can_tcp, _label)) in candidates.iter().enumerate() {
        probe_calls.push_str(&format!(
            "U{i}=\"{url}\"\nif probe \"$U{i}\" \"{can_tcp}\"; then echo \"OK $U{i}\"; exit 0; fi\nrc{i}=$?\n",
            i = i,
            url = url,
            can_tcp = can_tcp,
        ));
        if !tail.is_empty() {
            tail.push_str(" && ");
        }
        tail.push_str(&format!("[ \"$rc{i}\" = \"2\" ]", i = i));
    }
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
{probe_calls}if {tail}; then echo "NOTOOL"; exit 3; fi
echo "UNREACH"
exit 1
"#,
        probe_calls = probe_calls,
        tail = tail,
    );
    // Use _capture so we get stdout even when the script exits non-zero
    // (UNREACH/NOTOOL are signaled via stdout + non-zero exit). A real
    // wsl.exe spawn / timeout failure returns Err and we treat as
    // Unreachable.
    let stdout = match crate::wsl::fs::__wsl_run_script_capture(distro, &script) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("probe_url_for_distro({distro}): {e}");
            return Ok(ProbeOutcome::Unreachable);
        }
    };
    Ok(parse_probe_outcome(&stdout))
}

fn parse_probe_outcome(stdout: &str) -> ProbeOutcome {
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    let last = lines.last().copied().unwrap_or("");
    if let Some(rest) = last.strip_prefix("OK ") {
        let url = rest.trim().to_string();
        let method = lines
            .iter()
            .rev()
            .find_map(|l| match *l {
                "curl" => Some(ProbeMethod::HttpCurl),
                "wget" => Some(ProbeMethod::HttpWget),
                "tcp" => Some(ProbeMethod::TcpOnly),
                _ => None,
            })
            .unwrap_or(ProbeMethod::HttpCurl);
        return ProbeOutcome::Ok { url, method };
    }
    if last == "NOTOOL" {
        return ProbeOutcome::NoProbeTool;
    }
    ProbeOutcome::Unreachable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ok_with_curl() {
        let out = "curl\nOK http://host.docker.internal:18080\n";
        assert_eq!(
            parse_probe_outcome(out),
            ProbeOutcome::Ok {
                url: "http://host.docker.internal:18080".into(),
                method: ProbeMethod::HttpCurl,
            },
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

    #[test]
    fn parse_ok_with_tcp_fallback() {
        let out = "tcp\nOK http://127.0.0.1:18080\n";
        let got = parse_probe_outcome(out);
        if let ProbeOutcome::Ok { method, .. } = got {
            assert_eq!(method, ProbeMethod::TcpOnly);
        } else {
            panic!("expected Ok");
        }
    }
}
