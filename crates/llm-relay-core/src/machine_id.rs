//! OS-level machine identity for heartbeat de-duplication.
//!
//! The gateway dashboard keys devices by `clientId` — a UUID this app stores in
//! its own SQLite settings table. That UUID dies with the config database, so
//! reinstalling the app, resetting config, or moving to a new OS user makes the
//! same physical machine show up as a brand-new device. Nothing else in the
//! heartbeat can undo that: hostnames get renamed and collide (`localhost`), and
//! NAT makes several machines share one IP.
//!
//! The values read here survive an app reinstall and a process restart, and only
//! change when the OS itself is reinstalled — which genuinely is a new device.
//!
//! The value is reported to the server **raw**. Do not mix in a hostname, user
//! name, IP, or install path: every one of those reintroduces the instability we
//! are trying to remove. Hashing and redaction are the server's job.

use std::sync::OnceLock;

/// The OS machine identifier, or `None` where it cannot be read (locked-down
/// permissions, a stripped container image, an unsupported platform).
///
/// Callers must omit the field entirely when this is `None` rather than send an
/// empty string or a placeholder — the server falls back to `clientId`, and a
/// placeholder would instead collapse every such install into one fake device.
///
/// Cached: the value cannot change while the process lives, and the macOS and
/// Windows lookups both spawn a subprocess.
pub fn machine_id() -> Option<&'static str> {
    static ID: OnceLock<Option<String>> = OnceLock::new();
    ID.get_or_init(|| {
        let id = read_machine_id().filter(|s| !s.is_empty());
        match &id {
            Some(v) => log::info!("machine_id resolved ({} chars)", v.len()),
            None => log::warn!("machine_id unavailable; heartbeat will omit it"),
        }
        id
    })
    .as_deref()
}

#[cfg(target_os = "macos")]
fn read_machine_id() -> Option<String> {
    use std::process::Command;

    let out = Command::new("/usr/sbin/ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .ok()?;
    if !out.status.success() {
        log::debug!("ioreg exited {}", out.status);
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Line looks like:  "IOPlatformUUID" = "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE"
    let line = text.lines().find(|l| l.contains("IOPlatformUUID"))?;
    let value = line.split('=').nth(1)?;
    Some(value.trim().trim_matches('"').to_string())
}

#[cfg(target_os = "windows")]
fn read_machine_id() -> Option<String> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // `/reg:64` so a 32-bit build still reads the native hive instead of the
    // WOW6432Node redirect, where MachineGuid does not live.
    let out = Command::new("reg.exe")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Cryptography",
            "/v",
            "MachineGuid",
            "/reg:64",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !out.status.success() {
        log::debug!("reg.exe query MachineGuid exited {}", out.status);
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Line looks like:  MachineGuid    REG_SZ    aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee
    let line = text.lines().find(|l| l.contains("MachineGuid"))?;
    Some(line.split_whitespace().last()?.to_string())
}

#[cfg(target_os = "linux")]
fn read_machine_id() -> Option<String> {
    // /etc/machine-id is the systemd location; the dbus copy predates it and is
    // still the only one present on some minimal images.
    for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(s) = std::fs::read_to_string(path) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn read_machine_id() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On the three supported desktop platforms this should resolve in CI and on
    /// developer machines alike. A failure here means the heartbeat silently
    /// degrades to `clientId` and device de-duplication stops working.
    #[test]
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    fn resolves_on_this_platform() {
        let id = machine_id().expect("no OS machine id on a supported platform");
        assert!(!id.is_empty());
        // Guard against the parser handing back the whole matched line — every
        // platform's value is a single token well under this length.
        assert!(id.len() < 64, "suspiciously long machine id: {id:?}");
        assert!(
            !id.contains(char::is_whitespace),
            "machine id should be a single token: {id:?}"
        );
    }

    #[test]
    fn is_stable_across_calls() {
        assert_eq!(machine_id(), machine_id());
    }
}
