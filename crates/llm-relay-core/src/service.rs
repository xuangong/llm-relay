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

#[derive(Clone)]
pub struct Service {
    pub db: Arc<Database>,
    pub switch_lock: Arc<Mutex<()>>,
    pub sink: SharedEventSink,
}

impl Service {
    pub fn new(db: Arc<Database>, sink: SharedEventSink) -> Self {
        Self {
            db,
            sink,
            switch_lock: Arc::new(Mutex::new(())),
        }
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
        let views = self.list_gateway_views()?;
        Ok(views
            .into_iter()
            .map(|v| crate::ipc::GatewaySummary {
                id: v.id,
                name: v.name,
                url: v.url,
                starred: false,
                healthy: v.health.map(|h| matches!(h, HealthStatus::Healthy)),
                latency_ms: None,
            })
            .collect())
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

        // Fetch current config to preserve key_name / key_value
        let existing = self.db.get_active_config()?;

        let config = ActiveConfig {
            gateway_id: Some(gw_id_str),
            key_id: Some(key_id_str),
            key_name: existing.key_name,
            key_value: existing.key_value,
            claude_model: models.claude.clone(),
            claude_small_model: models.claude_small.clone(),
            codex_model: models.codex.clone(),
            gemini_model: models.gemini.clone(),
            auto_switch: existing.auto_switch,
            applied_at: Some(chrono::Utc::now().to_rfc3339()),
            last_switched_at: existing.last_switched_at,
        };
        self.db.set_active_config(&config)?;

        // Persist per-gateway model preferences
        self.db.update_gateway_models(
            &gateway_id.to_string(),
            models.claude.as_deref(),
            models.claude_small.as_deref(),
            models.codex.as_deref(),
            models.gemini.as_deref(),
        )?;

        // Apply CLI config files if we have a key value
        // TODO wire apply_active: retrieve key_value from keychain and call apply_all_configs
        // For now, emit the event so the GUI can react.

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

        crate::config_writer::clear_all_configs()?;

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

        // Prefer session token for auth, fall back to auth_key
        let auth = gw
            .session_token
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&gw.auth_key);

        let keys = crate::gateway::fetch_keys(&gw.url, auth).await?;
        Ok(keys
            .into_iter()
            .map(|k| KeyInfo {
                id: Uuid::parse_str(&k.id).unwrap_or_default(),
                name: k.name,
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
    ///
    /// TODO: wire to a real DB query once the `usage_by_gateway` view exists.
    /// For now returns an empty vec so the TUI compiles and shows an empty table.
    pub async fn get_usage_rows(&self, _range: UsageRange) -> Result<Vec<UsageRowDetail>, AppError> {
        // TODO: query `usage_log` table grouped by gateway_id+model for the requested range
        Ok(Vec::new())
    }

    /// Return recent error rows for the TUI Errors tab.
    ///
    /// TODO: wire to a real DB query once an `error_log` table/view exists.
    /// For now returns an empty vec so the TUI compiles and shows an empty table.
    pub async fn get_errors(&self, _limit: u32) -> Result<Vec<ErrorRow>, AppError> {
        // TODO: query `error_log` or equivalent table, ORDER BY timestamp DESC LIMIT _limit
        Ok(Vec::new())
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
        // TODO: persist & read auto_launch from a DB setting.
        let auto_launch = self.db.get_setting("auto_launch")?.as_deref() == Some("true");
        let keystore_str = match keystore_kind {
            KeystoreKind::System => "system".to_string(),
            KeystoreKind::EncryptedFile => "encrypted-file".to_string(),
        };
        Ok(TuiSettings {
            keystore_kind: keystore_str,
            agent_pid,
            socket_path,
            proxy_port,
            log_path,
            auto_launch,
        })
    }

    /// Persist the auto-launch-on-boot preference.
    ///
    /// TODO: wire to OS-level launch agent registration (launchd / systemd).
    pub async fn set_auto_launch(&self, enabled: bool) -> Result<(), AppError> {
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
