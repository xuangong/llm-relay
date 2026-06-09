//! Discover the Windows-side IPv4 of the `vEthernet (WSL)` adapter.
//!
//! The WSL2 NAT gateway IP changes whenever Windows or WSL restarts, so
//! callers must re-probe periodically (see `wsl::state`). Non-Windows
//! builds return `None` unconditionally.

use std::net::IpAddr;

/// Returns the IPv4 of the WSL virtual NIC, or `None` if not found / not
/// on Windows. The adapter name on Windows is "vEthernet (WSL)" (English
/// locale) or sometimes "vEthernet (WSL (Hyper-V firewall))" on newer
/// builds. Match case-insensitively on the substring "WSL".
pub fn find_wsl_gateway_ip() -> Option<IpAddr> {
    #[cfg(target_os = "windows")]
    {
        let adapters = match ipconfig::get_adapters() {
            Ok(a) => a,
            Err(_) => return None,
        };
        for ad in adapters {
            let name_lc = ad.friendly_name().to_lowercase();
            if !name_lc.contains("wsl") {
                continue;
            }
            for ip in ad.ip_addresses() {
                if let IpAddr::V4(v4) = ip {
                    // Skip 169.254.* link-local; real WSL NAT IPs are 172.x
                    if !v4.is_link_local() {
                        return Some(IpAddr::V4(*v4));
                    }
                }
            }
        }
        None
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_none_on_non_windows_or_when_wsl_absent() {
        // Linux/macOS: always None. Windows without WSL: also None. Windows
        // with WSL: Some(IpAddr). This test only asserts the function
        // doesn't panic and returns a typed value of either variant.
        let _ = find_wsl_gateway_ip();
    }
}
