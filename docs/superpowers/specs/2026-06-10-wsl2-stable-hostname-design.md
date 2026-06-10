# WSL2 Stable Hostname — Design Addendum

> Addendum to `2026-06-09-wsl2-integration-design.md`. Captures findings from
> the 2026-06-10 manual verification and replaces the original §2.1 base_url
> selection logic.

## Context

The original spec assumed `host.docker.internal` was a stable hostname that
WSL2 always resolves to the Windows host. Manual verification on a real
Windows + Ubuntu-24.04 box invalidated that assumption:

- `getent hosts host.docker.internal` returned **`192.168.31.177`** — the LAN
  Wi-Fi IP, not the WSL gateway.
- Origin: **Docker Desktop** writes its own `/etc/hosts` entries when its WSL
  integration is active. It hijacks `host.docker.internal` to point at the LAN
  IP for cross-distro reachability.
- Relay deliberately does NOT bind on the LAN IP (firewall safety, §2.1), so
  `host.docker.internal:18081/_relay/ping` times out.

Writing the WSL gateway IP (e.g. `172.22.128.1`) directly into CLI configs is
not viable: that IP is recomputed by the WSL networking layer on every reboot
and on `wsl --shutdown`. A baked-in IP becomes stale silently — the user
re-launches their CLI, gets connection refused, has no clue why.

We need a **distro-local hostname that we control**.

## Decision

**Per-distro base_url selection at apply time:**

```
1. Probe http://127.0.0.1:<port>/_relay/ping
   → 200 OK   ⇒ mirror mode. Write base_url = http://127.0.0.1:<port>.
                Stable forever (mirror IS the host's loopback).

2. Else, probe http://host.docker.internal:<port>/_relay/ping
   → 200 OK   ⇒ HDI resolves to our gateway (no DD hijack).
                Write base_url = http://host.docker.internal:<port>.

3. Else (NAT + DD-hijacked OR HDI not present)
   ⇒ Inject /etc/hosts via `wsl.exe -d <D> -u root`:
       <gw_ip> llm-relay-<port>.host
     Probe http://llm-relay-<port>.host:<port>/_relay/ping
     → 200 OK  ⇒ Write base_url = http://llm-relay-<port>.host:<port>.
     → fail    ⇒ ProbeOutcome::Unreachable. Surface in UI.
```

The hostname is **port-suffixed** (`llm-relay-18080.host` for the production
GUI, `llm-relay-18081.host` for a dev session) so multiple Relay instances on
the same host can coexist without their hosts entries colliding. The state
machine knows its port via `paths::proxy_port()` and only manages its own
line; it never touches another instance's entry. On disable / toggle-off, only
the current instance's hostname is removed.

`llm-relay-<port>.host` is our own name, no chance of upstream collision. Hosts
file manipulation needs root, but WSL's `-u root` flag never prompts (the host
has already authenticated this distro at registration time — by design).

**State machine maintenance:**

The 60-second tick already detects `gateway_ip` changes and rebinds the WSL
listener. When the IP changes AND any selected distro's stored `base_url`
points at `llm-relay.host`, the tick must also rewrite that distro's
`/etc/hosts` line. The CLI configs themselves do NOT change — `llm-relay.host`
keeps resolving correctly because we updated the entry under it.

This means the CLI process never has to be restarted on IP changes. A claude /
codex / gemini session that was running before `wsl --shutdown` continues to
work after WSL comes back up, as soon as the state machine has rewritten the
hosts entry.

## Decision Tree (Probe Stage)

```
                    ┌─────────────────────────┐
                    │ Distro becomes selected │
                    └────────────┬────────────┘
                                 │
                                 ▼
                    ┌─────────────────────────┐
                    │  curl 127.0.0.1:port    │
                    └────┬───────────────┬────┘
                       200│              │fail
                          │              │
                ┌─────────▼────┐   ┌─────▼───────────────┐
                │ MIRROR mode  │   │ curl HDI:port       │
                │ url=127.0.0.1│   └────┬───────────┬────┘
                └──────────────┘      200│           │fail
                                         │           │
                              ┌──────────▼─┐   ┌─────▼─────────────────┐
                              │ HDI healthy│   │ Inject /etc/hosts:    │
                              │ url=HDI    │   │ <gw_ip> llm-relay.host│
                              └────────────┘   │ curl llm-relay.host   │
                                               └────┬───────────────┬──┘
                                                  200│               │fail
                                                     │               │
                                          ┌──────────▼──────┐  ┌─────▼──────┐
                                          │ url=llm-relay.host│  │ Unreachable│
                                          └─────────────────┘  └────────────┘
```

## Why Not the Alternatives

**Alternative A: Write the gateway IP directly + rewrite CLI configs on IP
change.** Requires three writers (claude / codex / gemini) to be invoked every
tick that detects an IP change. CLI processes already running won't pick up
the new config until restart, so the user observes a window of broken state
on every WSL reboot. Plus: more files dirtied, more snapshot churn, more
audit-log noise.

**Alternative B: Tell the user to enable mirror mode in `~/.wslconfig`.**
Works, but pushes config burden onto the user, and breaks any per-distro
`/etc/wsl.conf` they've set up. We can mention this as a "if you don't want
the hosts injection, do this instead" note in README, but it shouldn't be the
default path.

**Alternative C: Accept Docker Desktop's `host.docker.internal` and bind on the
LAN IP it points to.** Direct violation of §2.1 firewall safety — that's
exactly what we're avoiding.

## Migration

Old snapshots that captured `/etc/hosts` (none today, since we never wrote it)
need no migration. New `clear_targets_from_snapshots` path needs to remove the
`llm-relay.host` line if present (regardless of whether snapshot recorded it),
to keep distros clean when relay is disabled.

## New Trait Requirements on `WslBackend`

Today `CliBackend` has read / write_atomic / remove / exists. Hosts file
manipulation needs **append-line-if-absent** + **rewrite-line** semantics
operating as **root**. Two options:

1. Add `write_root_atomic(rel, bytes)` to `CliBackend`. WindowsFsBackend
   panics or returns Err — Windows host never does root writes. WslBackend
   uses `wsl.exe -d <D> -u root`.
2. Keep `CliBackend` user-facing; introduce a separate `wsl::hosts` module
   with a small dedicated API: `set_hosts_entry(distro, hostname, ip)`,
   `clear_hosts_entry(distro, hostname)`. No trait changes; only WSL knows
   about hosts files.

**Going with option 2** — hosts is a WSL-specific concern; pushing it through
the cross-platform trait dilutes that trait's purpose.

## API Sketch

```rust
// crates/llm-relay-core/src/wsl/hosts.rs (new)

/// Set or refresh `<ip> <hostname>` in distro's /etc/hosts. Idempotent —
/// removes prior entries with the same hostname before adding. Atomic via
/// temp file + mv.
pub fn set_hosts_entry(distro: &str, hostname: &str, ip: IpAddr) -> Result<()>;

/// Remove all lines for `<hostname>` from distro's /etc/hosts.
pub fn clear_hosts_entry(distro: &str, hostname: &str) -> Result<()>;

// All ops use `wsl.exe -d <D> -u root -e sh -c '...'` and finish in a
// single subprocess call (read, mutate in shell, atomic write).
```

```rust
// probe.rs — extend ProbeOutcome
pub enum ProbeOutcome {
    Ok { url: String, method: ProbeMethod },
    NeedsHostsInjection { gateway_ip: IpAddr },  // NEW: 127 + HDI both failed; caller should inject + retry
    NoProbeTool,
    Unreachable,
}
```

```rust
// state.rs — tick logic per selected distro
let outcome = probe_url_for_distro(distro, binds, gw_ip).await?;
let url = match outcome {
    ProbeOutcome::Ok { url, .. } => Some(url),
    ProbeOutcome::NeedsHostsInjection { gateway_ip } => {
        wsl::hosts::set_hosts_entry(distro, "llm-relay.host", gateway_ip)?;
        // re-probe with the new hostname
        match probe_url_for_distro_with_hostname(distro, "llm-relay.host", port).await? {
            ProbeOutcome::Ok { url, .. } => Some(url),
            _ => None,
        }
    }
    _ => None,
};
```

## Edge Cases

- **Multiple Relay instances** (real GUI on 18080, dev on 18081): `llm-relay.host`
  collides if both inject. Disambiguate by suffixing port: `llm-relay-18080.host`,
  `llm-relay-18081.host`. State machine knows its own port via
  `paths::proxy_port()`. Or: only the GUI/agent that "wins" the lock manages
  the entry — but with `LLM_RELAY_PROXY_PORT` env support, dev sessions DO
  legitimately exist alongside, so port-suffixed hostname is the clean answer.

- **User has their own `llm-relay.host` entry**: extremely unlikely, but the
  port-suffixed variant avoids collision entirely.

- **IP-change race**: state machine sees old IP at tick T (stored), reads adapter
  at T+ε (new IP), rewrites hosts. Window during which CLI sends to old IP →
  TCP connect fails → CLI sees a single error → user retries → hosts now
  correct. Acceptable: we don't promise zero-downtime across `wsl --shutdown`.

- **Distro stopped during tick**: `wsl.exe -d <stopped_distro> -u root ...`
  starts the distro for ~2-3s. We've already accepted this cost in §3.5; no
  new overhead.

- **`/etc/hosts` immutable / read-only mount**: rare, but `wsl.exe -u root`
  failing returns `AppError::Config`, surfaced as ProbeOutcome::Unreachable.
  UI shows the user a hint to investigate.

## Verification

Add to §6 verification plan:

- §6.4: Docker Desktop installed → confirm hosts injection path → CLI works
- §6.5: `wsl --shutdown && wsl -d Ubuntu-24.04 -e true` → state machine
  rewrites hosts within 60s → CLI written before still works
- §6.6: Two Relay instances (real on 18080 + dev on 18081) → port-suffixed
  hosts entries don't collide → both work
- §6.7: `clear_targets_from_snapshots` → `llm-relay.host` line removed from
  every selected distro's `/etc/hosts`
