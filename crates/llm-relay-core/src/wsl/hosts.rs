//! Per-distro `/etc/hosts` injection of a Relay-controlled hostname.
//!
//! WSL2 NAT mode + Docker Desktop's `host.docker.internal` hijack
//! together leave us without a stable, distro-resolvable hostname for
//! the Windows host's WSL gateway IP. We work around that by writing
//! our own line into the distro's `/etc/hosts`:
//!
//! ```text
//! 172.22.128.1    llm-relay-18080.host
//! ```
//!
//! The hostname is **port-suffixed** so multiple Relay instances on the
//! same host (real GUI on 18080, dev session on 18081, etc.) don't
//! collide. Each instance manages only its own line.
//!
//! `/etc/hosts` mutations need root. WSL's `-u root` flag never prompts
//! — the host already authenticated this distro at registration time
//! (by design — the host is already implicitly trusted).
//!
//! All ops are idempotent: setting refreshes the IP, clearing is a
//! no-op when no line exists.

use crate::AppError;
use std::net::IpAddr;

/// Hostname this Relay instance manages in distro `/etc/hosts` files.
/// Port-suffixed so multiple Relay instances coexist (real on 18080,
/// dev on 18081).
pub fn relay_hostname() -> String {
    format!("llm-relay-{}.host", crate::paths::proxy_port())
}

/// Set or refresh `<ip> <hostname>` in distro's `/etc/hosts`. Idempotent —
/// removes prior entries with the same hostname before adding. Atomic
/// via temp file + mv. Requires root inside the distro.
#[cfg(not(target_os = "windows"))]
pub fn set_hosts_entry(_distro: &str, _hostname: &str, _ip: IpAddr) -> Result<(), AppError> {
    Err(AppError::Config(
        "set_hosts_entry: Windows only".into(),
    ))
}

#[cfg(target_os = "windows")]
pub fn set_hosts_entry(distro: &str, hostname: &str, ip: IpAddr) -> Result<(), AppError> {
    validate_hostname(hostname)?;
    let ip_str = ip.to_string();
    // Awk filter strips any existing line whose 2nd+ fields contain our
    // hostname (whole-token match — won't catch "foo-llm-relay-18080.host"),
    // then we append the canonical line.
    let script = format!(
        r#"set -e
HOSTS=/etc/hosts
HN={hostname}
IP={ip}
TMP="$(mktemp /tmp/llmrelay-hosts.XXXXXX)"
awk -v hn="$HN" '
{{
  drop=0
  for (i=2; i<=NF; i++) if ($i == hn) {{ drop=1; break }}
  if (!drop) print $0
}}
' "$HOSTS" > "$TMP"
printf "%s\t%s\n" "$IP" "$HN" >> "$TMP"
chmod 644 "$TMP"
mv -f "$TMP" "$HOSTS"
"#,
        hostname = shell_squote(hostname),
        ip = shell_squote(&ip_str),
    );
    let _ = crate::wsl::fs::__wsl_run_script_root(distro, &script)?;
    Ok(())
}

/// Remove all lines for `<hostname>` from distro's `/etc/hosts`.
/// No-op if no matching line exists.
#[cfg(not(target_os = "windows"))]
pub fn clear_hosts_entry(_distro: &str, _hostname: &str) -> Result<(), AppError> {
    Err(AppError::Config(
        "clear_hosts_entry: Windows only".into(),
    ))
}

#[cfg(target_os = "windows")]
pub fn clear_hosts_entry(distro: &str, hostname: &str) -> Result<(), AppError> {
    validate_hostname(hostname)?;
    let script = format!(
        r#"set -e
HOSTS=/etc/hosts
HN={hostname}
TMP="$(mktemp /tmp/llmrelay-hosts.XXXXXX)"
awk -v hn="$HN" '
{{
  drop=0
  for (i=2; i<=NF; i++) if ($i == hn) {{ drop=1; break }}
  if (!drop) print $0
}}
' "$HOSTS" > "$TMP"
chmod 644 "$TMP"
mv -f "$TMP" "$HOSTS"
"#,
        hostname = shell_squote(hostname),
    );
    let _ = crate::wsl::fs::__wsl_run_script_root(distro, &script)?;
    Ok(())
}

/// Defense-in-depth check on hostname before splicing into a shell
/// script. Allowed: ASCII letters, digits, dots, hyphens, length 1..=253.
/// We always single-quote the value too, but rejecting outright is
/// cheaper than reasoning about quote-escaping for shell + awk.
///
/// Only reachable from the Windows hosts-entry writers; the check itself
/// is platform-independent, so keep it compiled for tests everywhere.
#[cfg(any(target_os = "windows", test))]
fn validate_hostname(h: &str) -> Result<(), AppError> {
    if h.is_empty() || h.len() > 253 {
        return Err(AppError::Config(format!("invalid hostname: {h:?}")));
    }
    let ok = h
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    if !ok {
        return Err(AppError::Config(format!("invalid hostname: {h:?}")));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn shell_squote(s: &str) -> String {
    let escaped = s.replace('\'', r#"'\''"#);
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_hostname_uses_proxy_port() {
        let h = relay_hostname();
        assert!(h.starts_with("llm-relay-"));
        assert!(h.ends_with(".host"));
    }

    #[test]
    fn validate_accepts_normal_hostnames() {
        assert!(validate_hostname("llm-relay-18080.host").is_ok());
        assert!(validate_hostname("foo.bar.baz").is_ok());
    }

    #[test]
    fn validate_rejects_metacharacters() {
        assert!(validate_hostname("foo;rm -rf /").is_err());
        assert!(validate_hostname("foo bar").is_err());
        assert!(validate_hostname("").is_err());
        assert!(validate_hostname("$x").is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn shell_squote_escapes_quotes() {
        assert_eq!(shell_squote("a'b"), r#"'a'\''b'"#);
        assert_eq!(shell_squote("safe"), "'safe'");
    }
}
