//! High-level façade used by both the Tauri GUI and the IPC agent.
//! Each public method maps 1:1 to an `ipc::Request` variant.
//!
//! # ID conversion
//! The database layer stores gateway/key IDs as `String`.
//! The IPC protocol uses `uuid::Uuid`. Conversion is isolated here:
//!   - `Uuid → String`: `id.to_string()`
//!   - `String → Uuid`: `Uuid::parse_str(&s).unwrap_or_default()`

use crate::database::{ActiveConfig, Gateway};
use crate::ipc::protocol::*;
use crate::{AppError, Database, SharedEventSink};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

/// Decide which key id an activation of `gw` should use.
///
/// Key ids are per-gateway, so the previously active key is only a candidate
/// when we are re-applying the *same* gateway. Carrying it across gateways
/// hands `set_active` an id the new gateway has never heard of, and every
/// caller that resolves a key id needs the same rule — tray click, auto-switch
/// on failure, and the GUI's Apply button.
///
/// Returns `None` when the gateway has never been configured, which callers
/// surface as "log in to this gateway first" rather than guessing.
pub fn pick_key_id(gw: &Gateway, existing: Option<&ActiveConfig>) -> Option<String> {
    gw.preferred_key_id.clone().or_else(|| {
        existing
            .filter(|c| c.gateway_id.as_deref() == Some(gw.id.as_str()))
            .and_then(|c| c.key_id.clone())
    })
}

#[derive(Clone)]
pub struct Service {
    pub db: Arc<Database>,
    pub switch_lock: Arc<Mutex<()>>,
    pub sink: SharedEventSink,
    /// Set once the proxy is up via `with_proxy(handle)`. None during the
    /// brief window between Service construction and proxy startup, and
    /// in tests. Long-running tasks (WSL state machine, Tauri commands)
    /// that need rebind/shutdown clone this Arc.
    pub proxy: Option<Arc<crate::proxy_server::ProxyHandle>>,
}

impl Service {
    pub fn new(db: Arc<Database>, sink: SharedEventSink) -> Self {
        Self {
            db,
            sink,
            switch_lock: Arc::new(Mutex::new(())),
            proxy: None,
        }
    }

    /// Attach the running proxy handle so callers can reach rebind/shutdown
    /// through the service. Idempotent in test code; production callers
    /// invoke once during startup.
    pub fn with_proxy(mut self, proxy: Arc<crate::proxy_server::ProxyHandle>) -> Self {
        self.proxy = Some(proxy);
        self
    }

    /// Build the full Snapshot returned by `Request::GetSnapshot`.
    pub async fn snapshot(
        &self,
        agent_pid: u32,
        agent_started_at: chrono::DateTime<chrono::Utc>,
        proxy_port: u16,
        keystore_kind: KeystoreKind,
    ) -> Result<Snapshot, AppError> {
        let gateways = self.list_gateway_views()?;
        let active = self.active_view()?;
        let auto_failover = self
            .db
            .get_setting("auto_failover")?
            .as_deref()
            == Some("true");
        Ok(Snapshot {
            gateways,
            active,
            auto_failover,
            agent_pid,
            agent_started_at,
            proxy_port,
            keystore_kind,
        })
    }

    fn list_gateway_views(&self) -> Result<Vec<GatewayView>, AppError> {
        let rows = self.db.list_gateways_with_health()?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let id = Uuid::parse_str(&r.gateway.id).unwrap_or_default();
                GatewayView {
                    id,
                    name: r.gateway.name,
                    url: r.gateway.url,
                    sort_order: r.gateway.sort_order,
                    health: if r.is_healthy {
                        Some(HealthStatus::Healthy)
                    } else if r.last_checked.is_some() {
                        Some(HealthStatus::Down)
                    } else {
                        None
                    },
                    last_check_at: r.last_checked.as_deref().and_then(|s| {
                        chrono::DateTime::parse_from_rfc3339(s)
                            .ok()
                            .map(|dt| dt.with_timezone(&chrono::Utc))
                    }),
                    model_count: r.model_count.map(|n| n as u32),
                }
            })
            .collect())
    }

    /// List gateways with health info, mapped to `GatewaySummary` for TUI use.
    pub fn list_gateways(&self) -> Result<Vec<crate::ipc::GatewaySummary>, AppError> {
        // Pull raw gateways once (resolves auth_key from keystore) plus active
        // config so we can compute `needs_login` for each summary.
        let gateways = self.db.list_gateways()?;
        let active = self.db.get_active_config().ok();
        let active_gw_id = active.as_ref().and_then(|a| a.gateway_id.clone());
        let active_key_value = active.as_ref().and_then(|a| a.key_value.clone());

        let active_key_name = active.as_ref().and_then(|a| a.key_name.clone());

        let mut summaries = Vec::with_capacity(gateways.len());
        for gw in gateways {
            let health = self.db.get_health(&gw.id).ok().flatten();
            let id = Uuid::parse_str(&gw.id).unwrap_or_default();
            let is_active = active_gw_id.as_deref() == Some(gw.id.as_str());
            let active_key_empty = active_key_value
                .as_deref()
                .map(|s| s.is_empty())
                .unwrap_or(true);
            let has_auth = !gw.auth_key.is_empty()
                || gw.session_token.as_deref().is_some_and(|s| !s.is_empty());
            let needs_login = !has_auth && (!is_active || active_key_empty);
            summaries.push(crate::ipc::GatewaySummary {
                id,
                name: gw.name,
                url: gw.url,
                starred: is_active,
                healthy: health.as_ref().map(|h| h.is_healthy),
                latency_ms: health.as_ref().and_then(|h| h.latency_ms),
                needs_login,
                active_key_name: if is_active { active_key_name.clone() } else { None },
                claude_model: gw.claude_model,
                claude_small_model: gw.claude_small_model,
                codex_model: gw.codex_model,
                gemini_model: gw.gemini_model,
                user_name: gw.user_name,
            });
        }
        Ok(summaries)
    }

    fn active_view(&self) -> Result<Option<ActiveView>, AppError> {
        let cfg = self.db.get_active_config()?;
        let (gw_id_str, key_id_str) = match (cfg.gateway_id.as_deref(), cfg.key_id.as_deref()) {
            (Some(g), Some(k)) => (g.to_string(), k.to_string()),
            _ => return Ok(None),
        };
        let gateway_id = Uuid::parse_str(&gw_id_str).unwrap_or_default();
        let key_id = Uuid::parse_str(&key_id_str).unwrap_or_default();
        let key_name = cfg.key_name.unwrap_or_default();
        Ok(Some(ActiveView {
            gateway_id,
            key_id,
            key_name,
            models: ModelSelection {
                claude: cfg.claude_model,
                claude_small: cfg.claude_small_model,
                codex: cfg.codex_model,
                gemini: cfg.gemini_model,
            },
        }))
    }

    /// Add a new gateway; returns the newly assigned ID.
    pub async fn add_gateway(&self, input: GatewayInput) -> Result<Uuid, AppError> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now().to_rfc3339();
        let gw = Gateway {
            id: id.to_string(),
            name: input.name,
            url: input.url,
            auth_key: input.auth_key,
            is_admin: false,
            session_token: None,
            user_id: None,
            user_name: None,
            sort_order: 0,
            created_at: now,
            claude_model: None,
            claude_small_model: None,
            codex_model: None,
            gemini_model: None,
            preferred_key_id: None,
        };
        self.db.add_gateway(&gw)?;
        Ok(id)
    }

    /// Update mutable fields of an existing gateway.
    pub async fn update_gateway(&self, id: Uuid, fields: GatewayUpdate) -> Result<(), AppError> {
        let id_str = id.to_string();
        let existing = self
            .db
            .get_gateway(&id_str)?
            .ok_or_else(|| AppError::Config(format!("gateway {id} not found")))?;

        let name = fields.name.as_deref().unwrap_or(&existing.name);
        let url = fields.url.as_deref().unwrap_or(&existing.url);
        let auth_key = fields.auth_key.as_deref().unwrap_or(&existing.auth_key);

        self.db.update_gateway(&id_str, name, url, auth_key)?;
        Ok(())
    }

    /// Delete a gateway and its associated keychain secrets.
    pub async fn delete_gateway(&self, id: Uuid) -> Result<(), AppError> {
        self.db.delete_gateway(&id.to_string())?;
        Ok(())
    }

    /// Set the active gateway/key/model selection and apply CLI configs.
    pub async fn set_active(
        &self,
        gateway_id: Uuid,
        key_id: Uuid,
        models: ModelSelection,
    ) -> Result<(), AppError> {
        let _g = self.switch_lock.lock().await;

        let gw_id_str = gateway_id.to_string();
        let key_id_str = key_id.to_string();

        // Fetch the key value from the gateway so the proxy can forward with it.
        let gw = self.db.get_gateway(&gw_id_str)?
            .ok_or_else(|| AppError::Config(format!("gateway {gateway_id} not found")))?;
        let keys = crate::gateway::fetch_keys_with_fallback(
            &gw.url,
            gw.session_token.as_deref(),
            &gw.auth_key,
            Some(&key_id_str),
        )
        .await?;
        // Refuse rather than store NULLs. A config with no key_value makes the
        // proxy fall back to the gateway's own auth_key, so every request would
        // silently go out under a different credential than the one that was
        // picked — wrong quota, wrong attribution, and no sign anything failed.
        let matched_key = keys.iter().find(|k| k.id == key_id_str).ok_or_else(|| {
            AppError::Config(format!(
                "key {key_id} is not visible on {} — log in to that gateway again, \
                 then pick a key from the refreshed list",
                gw.name
            ))
        })?;
        let key_name = Some(matched_key.name.clone());
        let key_value = Some(matched_key.key.clone());

        // Fetch current config to preserve auto_switch / last_switched_at
        let existing = self.db.get_active_config()?;

        let config = ActiveConfig {
            gateway_id: Some(gw_id_str),
            key_id: Some(key_id_str),
            key_name,
            key_value,
            claude_model: models.claude.clone(),
            claude_small_model: models.claude_small.clone(),
            codex_model: models.codex.clone(),
            gemini_model: models.gemini.clone(),
            auto_switch: existing.auto_switch,
            applied_at: Some(chrono::Utc::now().to_rfc3339()),
            last_switched_at: existing.last_switched_at,
        };
        self.db.set_active_config(&config)?;

        // Persist per-gateway model preferences + key
        self.db.update_gateway_config(
            &gateway_id.to_string(),
            Some(&key_id.to_string()),
            models.claude.as_deref(),
            models.claude_small.as_deref(),
            models.codex.as_deref(),
            models.gemini.as_deref(),
        )?;

        // Apply CLI config files so claude/codex/gemini CLIs use the proxy.
        // Iterate over Windows + every selected WSL distro that has a
        // resolved URL. Distros without resolved_url are skipped here
        // and surface as "Unreachable" in the UI until Refresh succeeds.
        let targets = self.build_apply_targets();
        crate::config_writer::apply_to_targets(
            &targets,
            crate::proxy_server::PLACEHOLDER_KEY,
            models.claude.as_deref(),
            models.claude_small.as_deref(),
            models.codex.as_deref(),
            models.gemini.as_deref(),
        )?;
        // Windows-host shell env (OPENAI_API_KEY=dummy in shell rc / registry)
        // is unrelated to per-target writes — keep this side-effect.
        crate::config_writer::ensure_openai_api_key_in_shell_rc()?;

        crate::events::emit_typed(
            &*self.sink,
            "active_changed",
            &Event::ActiveChanged {
                gateway_id: Some(gateway_id),
            },
        );
        Ok(())
    }

    /// Clear the active selection and wipe CLI config files.
    pub async fn clear_active(&self) -> Result<(), AppError> {
        let _g = self.switch_lock.lock().await;

        let existing = self.db.get_active_config()?;
        let config = ActiveConfig {
            gateway_id: None,
            key_id: None,
            key_name: None,
            key_value: None,
            claude_model: None,
            claude_small_model: None,
            codex_model: None,
            gemini_model: None,
            auto_switch: existing.auto_switch,
            applied_at: None,
            last_switched_at: existing.last_switched_at,
        };
        self.db.set_active_config(&config)?;

        crate::config_writer::clear_targets_from_snapshots()?;

        // Defensive: state machine may have injected /etc/hosts entries on
        // selected distros that never had an apply run against them (so no
        // snapshot exists for clear_targets_from_snapshots to walk).
        // Sweep every selected distro for our hostname so disable leaves
        // distros clean regardless of apply history.
        #[cfg(target_os = "windows")]
        {
            let hostname = crate::wsl::hosts::relay_hostname();
            if let Ok(distros) = self.db.list_wsl_distros() {
                for d in distros.iter().filter(|d| d.selected) {
                    if let Err(e) = crate::wsl::hosts::clear_hosts_entry(&d.name, &hostname) {
                        log::warn!("clear hosts entry on disable for {}: {e}", d.name);
                    }
                }
            }
        }

        crate::events::emit_typed(
            &*self.sink,
            "active_changed",
            &Event::ActiveChanged { gateway_id: None },
        );
        Ok(())
    }

    /// Persist the auto-failover preference.
    /// Maps to `ActiveConfig::auto_switch` (same concept, different name in DB).
    pub async fn set_auto_failover(&self, on: bool) -> Result<(), AppError> {
        self.db.set_setting("auto_failover", if on { "true" } else { "false" })?;
        Ok(())
    }

    /// Reorder gateways by providing a sorted list of IDs.
    pub async fn reorder(&self, ids: Vec<Uuid>) -> Result<(), AppError> {
        let id_strings: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
        self.db.reorder_gateways(&id_strings)?;
        Ok(())
    }

    /// Return the base URL for a gateway (used by login flow).
    pub async fn get_gateway_url(&self, gateway_id: Uuid) -> Result<String, AppError> {
        let gw = self
            .db
            .get_gateway(&gateway_id.to_string())?
            .ok_or_else(|| AppError::Config(format!("gateway {gateway_id} not found")))?;
        Ok(gw.url)
    }

    /// Fetch API keys available on a gateway.
    pub async fn fetch_keys(&self, gateway_id: Uuid) -> Result<Vec<KeyInfo>, AppError> {
        let gw = self
            .db
            .get_gateway(&gateway_id.to_string())?
            .ok_or_else(|| AppError::Config(format!("gateway {gateway_id} not found")))?;

        // Prefer session token for auth, fall back to auth_key — a session that
        // owns no keys answers with an empty list, which would leave the picker
        // blank on a gateway that has keys.
        let keys = crate::gateway::fetch_keys_with_fallback(
            &gw.url,
            gw.session_token.as_deref(),
            &gw.auth_key,
            None,
        )
        .await?;
        Ok(keys
            .into_iter()
            .map(|k| KeyInfo {
                id: Uuid::parse_str(&k.id).unwrap_or_default(),
                name: k.name,
                key: k.key,
            })
            .collect())
    }

    /// Fetch the model catalog for a gateway/key combination.
    ///
    /// NOTE: The upstream `/api/models` endpoint returns a flat list of ModelInfo
    /// objects. There is no server-side categorisation by provider in the current
    /// gateway API. We classify models by their ID prefix as a best-effort mapping.
    /// TODO Phase 6: real key store — retrieve key value from persistent store
    pub async fn fetch_models(
        &self,
        gateway_id: Uuid,
        _key_id: Uuid,
    ) -> Result<ModelCatalog, AppError> {
        let gw = self
            .db
            .get_gateway(&gateway_id.to_string())?
            .ok_or_else(|| AppError::Config(format!("gateway {gateway_id} not found")))?;

        // TODO Phase 6: real key store — look up key value by key_id
        // For now use the gateway auth_key / session_token as the bearer token.
        let auth = gw
            .session_token
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&gw.auth_key);

        let model_list = crate::gateway::fetch_models(&gw.url, auth).await?;

        let mut claude = Vec::new();
        let mut codex = Vec::new();
        let mut gemini = Vec::new();

        for m in model_list.data {
            let id_lower = m.id.to_lowercase();
            if id_lower.contains("claude") {
                claude.push(m.id);
            } else if id_lower.contains("gpt")
                || id_lower.contains("o1")
                || id_lower.contains("o3")
                || id_lower.contains("o4")
                || id_lower.contains("codex")
            {
                codex.push(m.id);
            } else if id_lower.contains("gemini") {
                gemini.push(m.id);
            } else {
                // Unknown provider — add to claude bucket as a fallback
                claude.push(m.id);
            }
        }

        Ok(ModelCatalog {
            claude,
            codex,
            gemini,
        })
    }

    /// Return aggregated usage statistics for the requested time range.
    pub async fn get_usage(
        &self,
        range: TimeRange,
        gateway_id: Option<Uuid>,
    ) -> Result<UsageReport, AppError> {
        let period = range_to_period(range);
        let gw_id_str = gateway_id.map(|id| id.to_string());
        let rows = self
            .db
            .get_usage_stats(gw_id_str.as_deref(), period)?;
        Ok(UsageReport {
            range,
            rows: rows
                .into_iter()
                .map(|r| UsageRow {
                    model: r.model,
                    input: r.input_tokens as u64,
                    output: r.output_tokens as u64,
                    cache: (r.cache_read_tokens + r.cache_creation_tokens) as u64,
                    total: (r.input_tokens + r.output_tokens) as u64,
                })
                .collect(),
        })
    }

    /// Return recent traffic log entries.
    pub async fn get_traffic_log(
        &self,
        gateway_id: Option<Uuid>,
    ) -> Result<Vec<TrafficEntry>, AppError> {
        let gw_id_str = gateway_id.map(|id| id.to_string());
        let logs = self
            .db
            .get_traffic_log(gw_id_str.as_deref(), 500)?;
        Ok(logs
            .into_iter()
            .map(|l| {
                let at = chrono::DateTime::parse_from_rfc3339(&l.logged_at)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now());
                let gw_id =
                    Uuid::parse_str(&l.gateway_id).unwrap_or_default();
                TrafficEntry {
                    at,
                    gateway_id: gw_id,
                    gateway_name: l.gateway_name.unwrap_or_default(),
                    status: l.status,
                    path: l.path,
                    latency_ms: l.latency_ms as u32,
                    detail: l.error_detail.unwrap_or_default(),
                }
            })
            .collect())
    }

    /// Return the current application settings.
    pub async fn get_settings(&self) -> Result<Settings, AppError> {
        let client_name = self
            .db
            .get_setting("client_name")?
            .unwrap_or_default();
        let auto_failover = self
            .db
            .get_setting("auto_failover")?
            .as_deref()
            == Some("true");
        let launch_at_login = self
            .db
            .get_setting("launch_at_login")?
            .as_deref()
            == Some("true");
        Ok(Settings {
            client_name,
            auto_failover,
            launch_at_login,
        })
    }

    /// Persist one or more settings fields.
    pub async fn update_settings(&self, u: SettingsUpdate) -> Result<(), AppError> {
        if let Some(n) = u.client_name {
            self.db.set_setting("client_name", &n)?;
        }
        if let Some(b) = u.launch_at_login {
            self.db
                .set_setting("launch_at_login", if b { "true" } else { "false" })?;
        }
        Ok(())
    }

    /// Return per-gateway, per-model usage rows for the TUI Usage tab.
    pub async fn get_usage_rows(&self, range: UsageRange) -> Result<Vec<UsageRowDetail>, AppError> {
        let period = match range {
            UsageRange::Today => "today",
            UsageRange::Last7Days => "7d",
            UsageRange::Last30Days => "30d",
            UsageRange::AllTime => "all",
        };
        let stats = self.db.get_usage_stats_by_gateway(period)?;
        Ok(stats
            .into_iter()
            .map(|s| UsageRowDetail {
                gateway_id: uuid::Uuid::parse_str(&s.gateway_id).unwrap_or_default(),
                gateway_name: s.gateway_name,
                model: s.model,
                requests: s.requests as u64,
                input_tokens: s.input_tokens as u64,
                output_tokens: s.output_tokens as u64,
                cost_usd: 0.0,
            })
            .collect())
    }

    /// Return recent error rows for the TUI Errors tab.
    ///
    /// Not yet wired to a real `error_log` table. Returns `NotImplemented` so
    /// the TUI surfaces the gap rather than presenting an empty (and untrue)
    /// "no errors" view.
    pub async fn get_errors(&self, limit: u32) -> Result<Vec<ErrorRow>, AppError> {
        let entries = self.db.get_traffic_log(None, limit as usize)?;
        Ok(entries
            .into_iter()
            .filter(|e| e.status >= 400 || e.error_detail.is_some())
            .map(|e| {
                let kind = if e.status == 401 { "auth" }
                    else if e.status >= 500 { "proxy" }
                    else { "error" };
                ErrorRow {
                    timestamp_iso: e.logged_at,
                    gateway_name: e.gateway_name.unwrap_or_default(),
                    kind: kind.to_string(),
                    message: e.error_detail.unwrap_or_else(|| format!("HTTP {}", e.status)),
                }
            })
            .collect())
    }

    /// Return a TuiSettings snapshot for the Settings tab.
    ///
    /// `agent_pid`, `keystore_kind`, `socket_path`, `proxy_port`, and `log_path` are
    /// passed in from `ServerCtx` so the service layer stays stateless.
    pub async fn get_tui_settings(
        &self,
        agent_pid: u32,
        keystore_kind: KeystoreKind,
        socket_path: String,
        proxy_port: u16,
        log_path: String,
    ) -> Result<TuiSettings, AppError> {
        // NOTE: auto_launch reads from a DB setting, but it is NOT yet wired to
        // any OS-level launch agent (launchd / systemd). The value reflects
        // user intent, not actual OS state.
        log::warn!("auto_launch not yet implemented: returning DB-stored intent only");
        let auto_launch = self.db.get_setting("auto_launch")?.as_deref() == Some("true");
        let auto_failover = self.db.get_setting("auto_failover")?.as_deref() == Some("true");
        let keystore_str = match keystore_kind {
            KeystoreKind::System => "system".to_string(),
            KeystoreKind::EncryptedFile => "encrypted-file".to_string(),
            KeystoreKind::Env => "env".to_string(),
        };
        Ok(TuiSettings {
            keystore_kind: keystore_str,
            agent_pid,
            socket_path,
            proxy_port,
            log_path,
            auto_launch,
            auto_failover,
        })
    }

    /// Persist the auto-launch-on-boot preference.
    ///
    /// Currently a DB-only no-op; not yet wired to launchd / systemd.
    pub async fn set_auto_launch(&self, enabled: bool) -> Result<(), AppError> {
        log::warn!(
            "auto_launch not yet implemented: storing intent={} in DB but no OS registration",
            enabled
        );
        self.db.set_setting("auto_launch", if enabled { "true" } else { "false" })?;
        Ok(())
    }

    /// TUI: add a gateway with name+url only (auth_key defaults to empty).
    pub async fn add_gateway_simple(&self, name: String, url: String) -> Result<Uuid, AppError> {
        self.add_gateway(GatewayInput {
            name,
            url,
            auth_key: String::new(),
        })
        .await
    }

    /// TUI: update a gateway's name and url only.
    pub async fn update_gateway_simple(&self, id: Uuid, name: String, url: String) -> Result<(), AppError> {
        self.update_gateway(id, GatewayUpdate {
            name: Some(name),
            url: Some(url),
            auth_key: None,
        })
        .await
    }

    /// Persist session_token after a successful device-code login.
    pub async fn save_login_session(
        &self,
        gateway_id: Uuid,
        session_token: String,
        _user_id: Option<String>,
        _user_name: Option<String>,
    ) -> Result<(), AppError> {
        let id_str = gateway_id.to_string();
        self.db.update_gateway_session(&id_str, false, Some(&session_token))?;
        Ok(())
    }

    /// Get the active key_id and model preferences for a gateway.
    pub async fn get_gateway_config(&self, gateway_id: Uuid) -> Result<(Option<Uuid>, Option<String>, Option<String>, Option<String>, Option<String>), AppError> {
        let id_str = gateway_id.to_string();
        let gw = self.db.get_gateway(&id_str)?
            .ok_or_else(|| AppError::Config(format!("gateway {gateway_id} not found")))?;
        let preferred = gw.preferred_key_id.and_then(|k| Uuid::parse_str(&k).ok());
        Ok((preferred, gw.claude_model, gw.claude_small_model, gw.codex_model, gw.gemini_model))
    }

    /// Save key + model config for a gateway without activating it.
    pub async fn save_gateway_config(
        &self,
        gateway_id: Uuid,
        key_id: Uuid,
        models: ModelSelection,
    ) -> Result<(), AppError> {
        self.db.update_gateway_config(
            &gateway_id.to_string(),
            Some(&key_id.to_string()),
            models.claude.as_deref(),
            models.claude_small.as_deref(),
            models.codex.as_deref(),
            models.gemini.as_deref(),
        )?;
        Ok(())
    }

    /// Build the list of CLI targets to write configs for: Windows is
    /// always present; each selected WSL distro is included only when it
    /// has a probed `home` AND a `resolved_url`. Distros missing either
    /// are logged and skipped (the UI surfaces them as Unreachable /
    /// Unknown until the user clicks Refresh).
    pub fn build_apply_targets(&self) -> Vec<crate::cli_target::CliTarget> {
        use crate::cli_target::{
            CliTarget, InstalledTools, SnapshotMeta, TargetType, WindowsFsBackend, WslBackend,
        };

        let mut targets: Vec<CliTarget> = Vec::new();

        targets.push(CliTarget {
            backend: Box::new(WindowsFsBackend::new()),
            base_url: crate::proxy_server::proxy_base_url(),
            installed: InstalledTools::ALL,
            label: "windows".into(),
            snapshot_meta: SnapshotMeta {
                target_type: TargetType::Windows,
                distro_name: None,
                home: None,
            },
        });

        let rows = self.db.list_wsl_distros().unwrap_or_default();
        for row in rows {
            if !row.selected {
                continue;
            }
            let Some(url) = row.resolved_url.clone() else {
                log::warn!(
                    "WSL distro {} has no resolved_url — skipping apply",
                    row.name
                );
                continue;
            };
            let Some(home) = row.home.clone() else {
                log::warn!(
                    "WSL distro {} has no probed home — skipping apply",
                    row.name
                );
                continue;
            };
            targets.push(CliTarget {
                backend: Box::new(WslBackend {
                    distro: row.name.clone(),
                    home: home.clone(),
                }),
                base_url: url,
                installed: InstalledTools {
                    claude: row.has_claude,
                    codex: row.has_codex,
                    gemini: row.has_gemini,
                },
                label: format!("wsl:{}", row.name),
                snapshot_meta: SnapshotMeta {
                    target_type: TargetType::Wsl,
                    distro_name: Some(row.name),
                    home: Some(home),
                },
            });
        }

        targets
    }

    /// Build the WSL detection state machine. The caller is responsible for
    /// spawning `sm.clone().run()` on the appropriate runtime (tokio for the
    /// headless agent, `tauri::async_runtime` for the GUI). Returns `None`
    /// if `proxy` isn't attached (test code or pre-startup).
    pub fn spawn_wsl_state_machine(
        &self,
    ) -> Option<Arc<crate::wsl::state::StateMachine>> {
        let proxy = self.proxy.as_ref()?.clone();
        Some(crate::wsl::state::StateMachine::new(self.db.clone(), proxy))
    }

    /// List the WSL distros known to LLM Relay (cached in `wsl_distros`).
    /// Empty Vec on non-Windows or when no distros are installed.
    pub async fn list_wsl_distros(
        &self,
    ) -> Result<Vec<crate::ipc::protocol::WslDistroInfo>, AppError> {
        use crate::ipc::protocol::{WslDistroInfo, WslDistroStatus};
        let rows = self.db.list_wsl_distros()?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let status = match (&r.resolved_url, r.probed_at.is_some()) {
                    (Some(_), _) => WslDistroStatus::Ready,
                    (None, true) => WslDistroStatus::Unreachable,
                    (None, false) => WslDistroStatus::Unknown,
                };
                WslDistroInfo {
                    name: r.name,
                    is_default: r.is_default,
                    selected: r.selected,
                    home: r.home,
                    has_claude: r.has_claude,
                    has_codex: r.has_codex,
                    has_gemini: r.has_gemini,
                    resolved_url: r.resolved_url,
                    status,
                }
            })
            .collect())
    }

    /// Toggle whether a WSL distro is included in apply targets.
    pub async fn toggle_wsl_distro(&self, name: String, selected: bool) -> Result<(), AppError> {
        self.db.set_wsl_distro_selected(&name, selected)?;
        Ok(())
    }
}

// ─── Helpers ───────────────────────────────────────────────────────────────

fn range_to_period(r: TimeRange) -> &'static str {
    match r {
        TimeRange::Today => "today",
        TimeRange::Week => "week",
        TimeRange::Days7 => "7d",
        TimeRange::Days30 => "30d",
    }
}
