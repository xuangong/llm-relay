//! Detection cadence state machine. See spec §3.5.
//!
//! - **Active**: WSL available + ≥1 distro → reconcile + URL probe
//!   every 60s, plus rebind WSL listener if the gateway IP changed.
//! - **Lazy**:   WSL absent / no distros → no periodic work; only
//!   reconciles on explicit `request_refresh()` (Settings "Refresh"
//!   button, Tauri/TUI command, etc.).
//!
//! Spawned by `Service::spawn_wsl_state_machine()`. Holds an
//! `Arc<ProxyHandle>` so it can call `rebind_wsl` when the gateway IP
//! changes without round-tripping through the Service.

use std::sync::Arc;
use std::time::Duration;

const ACTIVE_TICK_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Active,
    Lazy,
}

pub struct StateMachine {
    service: Arc<crate::Service>,
    db: Arc<crate::Database>,
    proxy: Arc<crate::proxy_server::ProxyHandle>,
    mode: tokio::sync::Mutex<Mode>,
    refresh_signal: tokio::sync::Notify,
}

impl StateMachine {
    pub fn new(
        service: Arc<crate::Service>,
        proxy: Arc<crate::proxy_server::ProxyHandle>,
    ) -> Arc<Self> {
        Arc::new(Self {
            db: service.db.clone(),
            service,
            proxy,
            mode: tokio::sync::Mutex::new(Mode::Lazy),
            refresh_signal: tokio::sync::Notify::new(),
        })
    }

    /// Trigger a one-off refresh. Wakes `run` if it's idle.
    pub fn request_refresh(&self) {
        self.refresh_signal.notify_one();
    }

    /// Long-running task: ticks on a 60s schedule when Active, otherwise
    /// sleeps until `request_refresh` is called. Never returns under
    /// normal operation.
    pub async fn run(self: Arc<Self>) {
        // Initial detection: discover, probe URLs, set mode.
        self.tick().await;

        loop {
            let mode = *self.mode.lock().await;
            match mode {
                Mode::Active => {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(ACTIVE_TICK_SECS)) => {}
                        _ = self.refresh_signal.notified() => {}
                    }
                }
                Mode::Lazy => {
                    self.refresh_signal.notified().await;
                }
            }
            self.tick().await;
        }
    }

    async fn tick(&self) {
        // 1. Reconcile distros + their installed-tools cache.
        let distros = match tokio::task::spawn_blocking({
            let db = self.db.clone();
            move || crate::wsl::distro::refresh_distros_in_db(&db)
        })
        .await
        {
            Ok(Ok(d)) => d,
            Ok(Err(e)) => {
                log::warn!("refresh_distros: {e}");
                Vec::new()
            }
            Err(e) => {
                log::warn!("refresh_distros join: {e}");
                Vec::new()
            }
        };

        // 2. Re-bind WSL listener if gateway IP changed.
        let current_ip = self.proxy.wsl_ip();
        let new_ip = tokio::task::spawn_blocking(crate::wsl::network::find_wsl_gateway_ip)
            .await
            .ok()
            .flatten();
        if current_ip != new_ip {
            if let Err(e) = self.proxy.rebind_wsl(new_ip).await {
                log::warn!("rebind_wsl: {e}");
            }
        }

        // 3. Re-probe URL for each selected distro. Skip unselected to
        //    avoid the wsl.exe cold-start cost on distros the user
        //    doesn't want to manage. Also skip the entire loop when no
        //    gateway is active — otherwise tier-3 hosts injection
        //    re-writes `/etc/hosts` pointing at a dead gateway after
        //    Disable Relay (see followups spec #1).
        let has_active_gateway = self
            .db
            .get_active_config()
            .ok()
            .and_then(|c| c.gateway_id)
            .is_some();
        if has_active_gateway {
            let gw_ip = self.proxy.wsl_ip();
            let binds = crate::wsl::resolve::ListenerBinds {
                loopback: true,
                host_docker_internal: gw_ip.is_some(),
            };
            for d in &distros {
                if !d.selected {
                    continue;
                }
                let name = d.name.clone();
                let resolve_res = tokio::task::spawn_blocking(move || {
                    crate::wsl::resolve::resolve_url_for_distro(&name, binds, gw_ip)
                })
                .await;
                let resolved_url = match resolve_res {
                    Ok(Ok(crate::wsl::resolve::ResolveOutcome::Ok(url))) => Some(url),
                    Ok(Ok(_)) => None,
                    Ok(Err(e)) => {
                        log::warn!("resolve {}: {e}", d.name);
                        None
                    }
                    Err(e) => {
                        log::warn!("resolve join {}: {e}", d.name);
                        None
                    }
                };
                let mut row = d.clone();
                row.resolved_url = resolved_url;
                let _ = self.db.upsert_wsl_distro(&row);
            }
        }

        if has_active_gateway && crate::config_writer::lifecycle::has_pending_wsl() {
            if let Err(error) = self.service.retry_pending_wsl_apply().await {
                log::warn!("retry pending WSL apply: {error}");
            }
        }

        // 4. Keep probing while a selected lifecycle target is pending, even
        // when one transient discovery call returned no distros.
        let mode_new = if distros.is_empty() && !crate::config_writer::lifecycle::has_pending_wsl()
        {
            Mode::Lazy
        } else {
            Mode::Active
        };
        *self.mode.lock().await = mode_new;
        log::debug!(
            "WSL state machine tick: {} distros, mode={:?}",
            distros.len(),
            mode_new
        );
    }
}
