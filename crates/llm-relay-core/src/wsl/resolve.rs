//! Per-distro base URL resolution. Decides which `base_url` gets
//! written to the WSL2 CLI configs by the apply path.
//!
//! Decision tree (see `2026-06-10-wsl2-stable-hostname-design.md`):
//!
//! 1. `http://127.0.0.1:<port>` — succeeds in **mirror** networking mode
//!    (the host's loopback IS the distro's loopback). Stable forever.
//! 2. `http://host.docker.internal:<port>` — succeeds when HDI resolves
//!    to our WSL gateway. Often hijacked by Docker Desktop to a LAN IP
//!    we deliberately don't bind on, so this probe fails on those hosts.
//! 3. `http://llm-relay-<port>.host:<port>` — distro-local hostname we
//!    inject into `/etc/hosts` pointing at the WSL gateway IP.
//!
//! The hosts-injection path requires `gateway_ip`; without it we can
//! only attempt steps 1 and 2.

use crate::wsl::probe::{probe_url, ProbeOutcome};
use crate::AppError;
use std::net::IpAddr;

/// Outcome of resolving a distro's base URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveOutcome {
    /// Use this URL as `base_url` in CLI configs.
    Ok(String),
    /// Distro lacks curl/wget and TCP fallback wasn't eligible. UI
    /// shows a specific install hint.
    NoProbeTool,
    /// All candidates failed.
    Unreachable,
}

/// What listeners the relay process actually bound. Gates whether
/// `/dev/tcp` fallback is trustworthy for a given URL (a TCP-open
/// without a real listener could be a different process).
#[derive(Debug, Clone, Copy)]
pub struct ListenerBinds {
    /// 127.0.0.1 listener — always bound when the proxy is up.
    pub loopback: bool,
    /// Listener on the WSL gateway IP — bound only when the gateway
    /// IP was discoverable. host.docker.internal and the injected
    /// `llm-relay-<port>.host` both resolve to this IP, so they share
    /// this gate.
    pub host_docker_internal: bool,
}

/// Resolve the URL the CLIs in `distro` should use to reach this Relay
/// instance. May write to `/etc/hosts` inside the distro on the
/// hosts-injection branch.
#[cfg(not(target_os = "windows"))]
pub fn resolve_url_for_distro(
    _distro: &str,
    _binds: ListenerBinds,
    _gateway_ip: Option<IpAddr>,
) -> Result<ResolveOutcome, AppError> {
    Err(AppError::Config(
        "resolve_url_for_distro: Windows only".into(),
    ))
}

#[cfg(target_os = "windows")]
pub fn resolve_url_for_distro(
    distro: &str,
    binds: ListenerBinds,
    gateway_ip: Option<IpAddr>,
) -> Result<ResolveOutcome, AppError> {
    let port = crate::paths::proxy_port();

    // 1) Loopback — mirror mode wins outright.
    let lo_url = format!("http://127.0.0.1:{port}");
    match probe_url(distro, &lo_url, binds.loopback)? {
        ProbeOutcome::Ok { .. } => return Ok(ResolveOutcome::Ok(lo_url)),
        ProbeOutcome::NoProbeTool => {
            // No curl/wget AND no eligible TCP fallback — no point
            // trying further URLs from inside this distro.
            return Ok(ResolveOutcome::NoProbeTool);
        }
        ProbeOutcome::Unreachable => {}
    }

    // 2) host.docker.internal — works when not hijacked.
    let hdi_url = format!("http://host.docker.internal:{port}");
    if let ProbeOutcome::Ok { .. } = probe_url(distro, &hdi_url, binds.host_docker_internal)? {
        return Ok(ResolveOutcome::Ok(hdi_url));
    }

    // 3) Hosts injection — only when we have a gateway IP to point at.
    let Some(gw) = gateway_ip else {
        return Ok(ResolveOutcome::Unreachable);
    };
    let hostname = crate::wsl::hosts::relay_hostname();
    if let Err(e) = crate::wsl::hosts::set_hosts_entry(distro, &hostname, gw) {
        log::warn!("set_hosts_entry({distro}, {hostname}, {gw}): {e}");
        return Ok(ResolveOutcome::Unreachable);
    }
    let host_url = format!("http://{hostname}:{port}");
    if let ProbeOutcome::Ok { .. } = probe_url(distro, &host_url, binds.host_docker_internal)? {
        return Ok(ResolveOutcome::Ok(host_url));
    }
    Ok(ResolveOutcome::Unreachable)
}
