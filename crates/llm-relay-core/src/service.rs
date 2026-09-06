//! High-level façade used by both the Tauri GUI and the IPC agent.
//! Each public method maps 1:1 to an `ipc::Request` variant.
//!
//! # ID conversion
//! The database layer stores gateway/key IDs as `String`.
//! The IPC protocol uses `uuid::Uuid`. Conversion is isolated here:
//!   - `Uuid → String`: `id.to_string()`
//!   - `String → Uuid`: `Uuid::parse_str(&s).unwrap_or_default()`

use crate::database::{ActiveConfig, ClaudeExtraConfig, Gateway};
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

const RESERVED_CLAUDE_EXTRA_KEYS: &[&str] = &[
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "ANTHROPIC_CUSTOM_HEADERS",
];

fn validate_claude_extra_config(
    name: &str,
    env: &std::collections::BTreeMap<String, String>,
) -> Result<(), AppError> {
    if name.trim().is_empty() {
        return Err(AppError::Config(
            "Claude Extra config name cannot be empty".into(),
        ));
    }
    if env.is_empty() {
        return Err(AppError::Config(
            "Claude Extra config must contain at least one entry".into(),
        ));
    }
    for key in env.keys() {
        let mut chars = key.chars();
        let valid = chars
            .next()
            .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
            && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric());
        if !valid {
            return Err(AppError::Config(format!("Invalid environment key: {key}")));
        }
        if RESERVED_CLAUDE_EXTRA_KEYS
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(key))
        {
            return Err(AppError::Config(format!(
                "{key} is managed by LLM Relay and cannot be used in Claude Extra config"
            )));
        }
    }
    Ok(())
}

pub struct PendingWslTarget {
    pub name: String,
    pub home: Option<String>,
    pub installed: crate::cli_target::InstalledTools,
    pub reason: String,
}

pub struct ApplyPlan {
    pub ready: Vec<crate::cli_target::CliTarget>,
    pub pending: Vec<PendingWslTarget>,
    pub retained_keys: std::collections::HashSet<String>,
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
        let auto_failover = self.db.get_setting("auto_failover")?.as_deref() == Some("true");
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
                active_key_name: if is_active {
                    active_key_name.clone()
                } else {
                    None
                },
                claude_model: gw.claude_model,
                claude_subagent_model: gw.claude_subagent_model,
                claude_small_model: gw.claude_small_model,
                codex_model: gw.codex_model,
                codex_subagent_model: gw.codex_subagent_model,
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
                claude_subagent: cfg.claude_subagent_model,
                claude_small: cfg.claude_small_model,
                codex: cfg.codex_model,
                codex_subagent: cfg.codex_subagent_model,
                gemini: cfg.gemini_model,
                claude_extra: cfg
                    .claude_extra_config_id
                    .and_then(|id| Uuid::parse_str(&id).ok())
                    .map(ClaudeExtraSelection::Preset)
                    .unwrap_or(ClaudeExtraSelection::Disabled),
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
            claude_subagent_model: None,
            claude_small_model: None,
            codex_model: None,
            codex_subagent_model: None,
            gemini_model: None,
            preferred_key_id: None,
            claude_extra_config_id: self.db.default_claude_extra_config_id()?,
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

    pub fn list_claude_extra_configs(&self) -> Result<Vec<ClaudeExtraConfig>, AppError> {
        self.db.list_claude_extra_configs()
    }

    pub fn create_claude_extra_config(
        &self,
        name: String,
        env: std::collections::BTreeMap<String, String>,
    ) -> Result<ClaudeExtraConfig, AppError> {
        validate_claude_extra_config(&name, &env)?;
        self.db.create_claude_extra_config(name.trim(), &env)
    }

    pub fn update_claude_extra_config(
        &self,
        id: Uuid,
        name: String,
        env: std::collections::BTreeMap<String, String>,
    ) -> Result<ClaudeExtraConfig, AppError> {
        validate_claude_extra_config(&name, &env)?;
        self.db
            .update_claude_extra_config(&id.to_string(), name.trim(), &env)
    }

    pub fn delete_claude_extra_config(&self, id: Uuid) -> Result<(), AppError> {
        self.db.delete_claude_extra_config(&id.to_string())
    }

    /// Set the active gateway/key/model selection and apply CLI configs.
    pub async fn set_active(
        &self,
        gateway_id: Uuid,
        key_id: Uuid,
        models: ModelSelection,
    ) -> Result<(), AppError> {
        let _g = self.switch_lock.lock().await;
        self.set_active_locked(gateway_id, key_id, models).await
    }

    async fn set_active_locked(
        &self,
        gateway_id: Uuid,
        key_id: Uuid,
        models: ModelSelection,
    ) -> Result<(), AppError> {
        let db_active = self.db.get_active_config()?.gateway_id.is_some();
        crate::config_writer::lifecycle::recover(db_active)?;
        let gw_id_str = gateway_id.to_string();
        let key_id_str = key_id.to_string();

        // Fetch the key value from the gateway so the proxy can forward with it.
        let gw = self
            .db
            .get_gateway(&gw_id_str)?
            .ok_or_else(|| AppError::Config(format!("gateway {gateway_id} not found")))?;
        let mut models = models;
        if let Some(main) = models.claude.as_deref() {
            models.claude = Some(crate::model_id::normalize_claude_main_model(main));
        }
        let all_claude = [
            models.claude.as_deref(),
            models.claude_subagent.as_deref(),
            models.claude_small.as_deref(),
        ]
        .into_iter()
        .all(|model| model.is_some_and(crate::model_id::is_claude_family_model));
        let requested_extra_config_id = match &models.claude_extra {
            ClaudeExtraSelection::Inherit => gw.claude_extra_config_id.clone(),
            ClaudeExtraSelection::Disabled => None,
            ClaudeExtraSelection::Preset(id) => Some(id.to_string()),
        };
        let claude_extra_config_id = if all_claude {
            requested_extra_config_id
                .or_else(|| self.db.default_claude_extra_config_id().ok().flatten())
        } else {
            self.db.default_claude_extra_config_id()?
        };
        let claude_extra_config =
            match claude_extra_config_id.as_deref() {
                Some(id) => Some(self.db.get_claude_extra_config(id)?.ok_or_else(|| {
                    AppError::Config(format!("Claude Extra config {id} not found"))
                })?),
                None => None,
            };
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
            claude_subagent_model: models.claude_subagent.clone(),
            claude_small_model: models.claude_small.clone(),
            codex_model: models.codex.clone(),
            codex_subagent_model: models.codex_subagent.clone(),
            gemini_model: models.gemini.clone(),
            claude_extra_config_id: claude_extra_config_id.clone(),
            auto_switch: existing.auto_switch,
            applied_at: Some(chrono::Utc::now().to_rfc3339()),
            last_switched_at: existing.last_switched_at,
        };
        // Finish every fallible external config operation before publishing the
        // new active selection. In particular, an old snapshot may need an
        // atomic on-disk upgrade before `apply_to_targets` can write anything;
        // if that fails, the proxy-visible DB state must remain unchanged.
        let apply_plan = self.build_apply_plan();
        let targets = &apply_plan.ready;
        let inactive_use = existing.gateway_id.is_none();
        let shell_paths = crate::config_writer::shell_rc_paths(targets);
        let mut file_lifecycle = if inactive_use {
            Some(crate::config_writer::lifecycle::prepare_use(
                targets,
                &apply_plan.pending,
                &shell_paths,
            )?)
        } else {
            crate::config_writer::lifecycle::prepare_active_apply(&targets, &shell_paths)?
        };

        // Host-level shell env for Codex CLI, which will not start without
        // OPENAI_API_KEY set. Do this before the per-target writes so a registry
        // failure cannot leave fresh CLI files paired with the old active DB row.
        // WSL targets get their own rc line inside `apply_to_targets`.
        if targets.iter().any(|t| t.installed.codex) {
            crate::config_writer::ensure_openai_api_key_env()?;
        }

        // Apply CLI config files so claude/codex/gemini CLIs use the proxy.
        // Iterate over Windows + every selected WSL distro that has a
        // resolved URL. Distros without resolved_url are skipped here
        // and surface as "Unreachable" in the UI until Refresh succeeds.
        let report = match crate::config_writer::apply_to_targets(
            targets,
            Some(&apply_plan.retained_keys),
            crate::proxy_server::PLACEHOLDER_KEY,
            models.claude.as_deref(),
            models.claude_subagent.as_deref(),
            models.claude_small.as_deref(),
            models.codex.as_deref(),
            models.codex_subagent.as_deref(),
            models.gemini.as_deref(),
            claude_extra_config.as_ref().map(|config| &config.env),
        ) {
            Ok(report) => report,
            Err(error) => {
                if inactive_use {
                    if let Some(manifest) = file_lifecycle.as_mut() {
                        let _ = crate::config_writer::lifecycle::rollback_use(manifest);
                    }
                }
                return Err(error);
            }
        };
        if let Some(manifest) = file_lifecycle.as_mut() {
            crate::config_writer::lifecycle::mark_targets_active(manifest, &report.succeeded)?;
            for (key, error) in &report.failed {
                crate::config_writer::lifecycle::mark_target_failed(key, error)?;
            }
        }

        self.db.set_active_config(&config)?;

        // Persist per-gateway model preferences + key only after the CLI files
        // successfully reflect the same selection.
        self.db.update_gateway_config(
            &gateway_id.to_string(),
            Some(&key_id.to_string()),
            models.claude.as_deref(),
            models.claude_subagent.as_deref(),
            models.claude_small.as_deref(),
            models.codex.as_deref(),
            models.codex_subagent.as_deref(),
            models.gemini.as_deref(),
            claude_extra_config_id.as_deref(),
        )?;

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
            claude_subagent_model: None,
            claude_small_model: None,
            codex_model: None,
            codex_subagent_model: None,
            gemini_model: None,
            claude_extra_config_id: None,
            auto_switch: existing.auto_switch,
            applied_at: None,
            last_switched_at: existing.last_switched_at,
        };
        // Back up the current Relay working files before restoring the exact
        // files captured when this inactive → Use cycle began. The legacy
        // field-level snapshot path remains a fallback for pre-lifecycle users.
        #[cfg(target_os = "windows")]
        let lifecycle_distros: std::collections::HashSet<String> =
            crate::config_writer::lifecycle::load()
                .ok()
                .flatten()
                .into_iter()
                .flat_map(|manifest| manifest.targets)
                .filter_map(|target| target.distro_name)
                .collect();
        if crate::config_writer::lifecycle::manifest_exists() {
            crate::config_writer::lifecycle::disable()?;
        } else if crate::config_writer::snapshot::has_legacy_snapshots()? {
            // Upgrade compatibility: releases before the full-file lifecycle
            // still have authoritative field snapshots. Restore those rather
            // than trapping an active user in a configuration they cannot
            // disable.
            crate::config_writer::clear_targets_from_snapshots()?;
        } else {
            return Err(AppError::Config(
                "No trusted full-file origin manifest or legacy snapshot exists; refusing to disable"
                    .into(),
            ));
        }

        self.db.set_active_config(&config)?;

        // Defensive: state machine may have injected /etc/hosts entries on
        // selected distros that never had an apply run against them (so no
        // snapshot exists for clear_targets_from_snapshots to walk).
        // Sweep every selected distro for our hostname so disable leaves
        // distros clean regardless of apply history.
        #[cfg(target_os = "windows")]
        {
            let hostname = crate::wsl::hosts::relay_hostname();
            let mut cleared = std::collections::HashSet::new();
            if let Ok(distros) = self.db.list_wsl_distros() {
                for d in distros
                    .iter()
                    .filter(|d| d.selected || lifecycle_distros.contains(&d.name))
                {
                    if let Err(e) = crate::wsl::hosts::clear_hosts_entry(&d.name, &hostname) {
                        log::warn!("clear hosts entry on disable for {}: {e}", d.name);
                    }
                    cleared.insert(d.name.clone());
                }
            }
            for distro in lifecycle_distros
                .into_iter()
                .filter(|distro| !cleared.contains(distro))
            {
                if let Err(e) = crate::wsl::hosts::clear_hosts_entry(&distro, &hostname) {
                    log::warn!("clear hosts entry on disable for {distro}: {e}");
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
        self.db
            .set_setting("auto_failover", if on { "true" } else { "false" })?;
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
    pub async fn fetch_models(
        &self,
        gateway_id: Uuid,
        key_id: Uuid,
    ) -> Result<ModelCatalog, AppError> {
        let gw = self
            .db
            .get_gateway(&gateway_id.to_string())?
            .ok_or_else(|| AppError::Config(format!("gateway {gateway_id} not found")))?;
        let key_id = key_id.to_string();
        let keys = crate::gateway::fetch_keys_with_fallback(
            &gw.url,
            gw.session_token.as_deref(),
            &gw.auth_key,
            Some(&key_id),
        )
        .await?;
        let key = keys.iter().find(|key| key.id == key_id).ok_or_else(|| {
            AppError::Config(format!(
                "key {key_id} is not visible on {} — log in again and refresh the key list",
                gw.name
            ))
        })?;

        let model_list = crate::gateway::fetch_models(&gw.url, &key.key).await?;

        Ok(classify_models(
            model_list.data.into_iter().map(|model| model.id),
        ))
    }

    /// Return aggregated usage statistics for the requested time range.
    pub async fn get_usage(
        &self,
        range: TimeRange,
        gateway_id: Option<Uuid>,
    ) -> Result<UsageReport, AppError> {
        let period = range_to_period(range);
        let gw_id_str = gateway_id.map(|id| id.to_string());
        let rows = self.db.get_usage_stats(gw_id_str.as_deref(), period)?;
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
        let logs = self.db.get_traffic_log(gw_id_str.as_deref(), 500, false)?;
        Ok(logs
            .into_iter()
            .map(|l| {
                let at = chrono::DateTime::parse_from_rfc3339(&l.logged_at)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now());
                let gw_id = Uuid::parse_str(&l.gateway_id).unwrap_or_default();
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
        let client_name = self.db.get_setting("client_name")?.unwrap_or_default();
        let auto_failover = self.db.get_setting("auto_failover")?.as_deref() == Some("true");
        let launch_at_login = self.db.get_setting("launch_at_login")?.as_deref() == Some("true");
        Ok(Settings {
            client_name,
            auto_failover,
            launch_at_login,
            managed_clients: self.db.get_managed_clients()?,
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
        if let Some(clients) = u.managed_clients {
            self.set_managed_clients(clients).await?;
        }
        Ok(())
    }

    pub async fn set_managed_clients(
        &self,
        clients: crate::cli_target::ManagedClients,
    ) -> Result<(), AppError> {
        if !clients.any() {
            return Err(AppError::Config(
                "At least one managed client must be selected".into(),
            ));
        }
        let _guard = self.switch_lock.lock().await;
        let active = self.db.get_active_config()?;
        crate::config_writer::lifecycle::recover(active.gateway_id.is_some())?;
        let previous = self.db.get_managed_clients()?;
        if previous == clients {
            return Ok(());
        }
        self.db.set_managed_clients(clients)?;
        if active.gateway_id.is_none() {
            return Ok(());
        }
        if let Err(error) = self.retry_active_config_locked(active.clone()).await {
            self.db.set_managed_clients(previous)?;
            if let Err(rollback) = self.retry_active_config_locked(active).await {
                return Err(AppError::Config(format!(
                    "managed client update failed: {error}; rollback failed: {rollback}"
                )));
            }
            return Err(error);
        }
        Ok(())
    }

    async fn retry_active_config_locked(&self, active: ActiveConfig) -> Result<(), AppError> {
        let gateway_id = active
            .gateway_id
            .as_deref()
            .ok_or_else(|| AppError::Config("No active gateway".into()))?;
        let gateway = self
            .db
            .get_gateway(gateway_id)?
            .ok_or_else(|| AppError::Config("active gateway not found".into()))?;
        let key_id = pick_key_id(&gateway, Some(&active))
            .ok_or_else(|| AppError::Config("active gateway key not found".into()))?;
        self.set_active_locked(
            Uuid::parse_str(gateway_id).map_err(|e| AppError::Config(e.to_string()))?,
            Uuid::parse_str(&key_id).map_err(|e| AppError::Config(e.to_string()))?,
            ModelSelection {
                claude: active.claude_model,
                claude_subagent: active.claude_subagent_model,
                claude_small: active.claude_small_model,
                codex: active.codex_model,
                codex_subagent: active.codex_subagent_model,
                gemini: active.gemini_model,
                claude_extra: ClaudeExtraSelection::Inherit,
            },
        )
        .await
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
        let entries = self.db.get_traffic_log(None, limit as usize, false)?;
        Ok(entries
            .into_iter()
            .filter(|e| e.status >= 400 || e.error_detail.is_some())
            .map(|e| {
                let kind = if e.status == 401 {
                    "auth"
                } else if e.status >= 500 {
                    "proxy"
                } else {
                    "error"
                };
                ErrorRow {
                    timestamp_iso: e.logged_at,
                    gateway_name: e.gateway_name.unwrap_or_default(),
                    kind: kind.to_string(),
                    message: e
                        .error_detail
                        .unwrap_or_else(|| format!("HTTP {}", e.status)),
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
            managed_clients: self.db.get_managed_clients()?,
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
        self.db
            .set_setting("auto_launch", if enabled { "true" } else { "false" })?;
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
    pub async fn update_gateway_simple(
        &self,
        id: Uuid,
        name: String,
        url: String,
    ) -> Result<(), AppError> {
        self.update_gateway(
            id,
            GatewayUpdate {
                name: Some(name),
                url: Some(url),
                auth_key: None,
            },
        )
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
        self.db
            .update_gateway_session(&id_str, false, Some(&session_token))?;
        Ok(())
    }

    /// Get the active key_id and model preferences for a gateway.
    pub async fn get_gateway_config(
        &self,
        gateway_id: Uuid,
    ) -> Result<
        (
            Option<Uuid>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<Uuid>,
        ),
        AppError,
    > {
        let id_str = gateway_id.to_string();
        let gw = self
            .db
            .get_gateway(&id_str)?
            .ok_or_else(|| AppError::Config(format!("gateway {gateway_id} not found")))?;
        let preferred = gw.preferred_key_id.and_then(|k| Uuid::parse_str(&k).ok());
        Ok((
            preferred,
            gw.claude_model,
            gw.claude_subagent_model,
            gw.claude_small_model,
            gw.codex_model,
            gw.codex_subagent_model,
            gw.gemini_model,
            gw.claude_extra_config_id
                .and_then(|id| Uuid::parse_str(&id).ok()),
        ))
    }

    /// Save key + model config for a gateway without activating it.
    pub async fn save_gateway_config(
        &self,
        gateway_id: Uuid,
        key_id: Uuid,
        models: ModelSelection,
    ) -> Result<(), AppError> {
        let gateway = self
            .db
            .get_gateway(&gateway_id.to_string())?
            .ok_or_else(|| AppError::Config(format!("gateway {gateway_id} not found")))?;
        let all_claude = [
            models.claude.as_deref(),
            models.claude_subagent.as_deref(),
            models.claude_small.as_deref(),
        ]
        .into_iter()
        .all(|model| model.is_some_and(crate::model_id::is_claude_family_model));
        let requested_extra_id = match models.claude_extra {
            ClaudeExtraSelection::Inherit => gateway.claude_extra_config_id,
            ClaudeExtraSelection::Disabled => None,
            ClaudeExtraSelection::Preset(id) => Some(id.to_string()),
        };
        let extra_id = if all_claude {
            requested_extra_id.or_else(|| self.db.default_claude_extra_config_id().ok().flatten())
        } else {
            self.db.default_claude_extra_config_id()?
        };
        self.db.update_gateway_config(
            &gateway_id.to_string(),
            Some(&key_id.to_string()),
            models
                .claude
                .as_deref()
                .map(crate::model_id::normalize_claude_main_model)
                .as_deref(),
            models.claude_subagent.as_deref(),
            models.claude_small.as_deref(),
            models.codex.as_deref(),
            models.codex_subagent.as_deref(),
            models.gemini.as_deref(),
            extra_id.as_deref(),
        )?;
        Ok(())
    }

    /// Build the list of CLI targets to write configs for: Windows is
    /// always present; each selected WSL distro is included only when it
    /// has a probed `home` AND a `resolved_url`. Distros missing either
    /// are logged and skipped (the UI surfaces them as Unreachable /
    /// Unknown until the user clicks Refresh).
    pub fn build_apply_plan(&self) -> ApplyPlan {
        use crate::cli_target::{
            CliTarget, InstalledTools, SnapshotMeta, TargetType, WindowsFsBackend, WslBackend,
        };

        let managed = self
            .db
            .get_managed_clients()
            .unwrap_or(crate::cli_target::ManagedClients::ALL);
        let mut ready: Vec<CliTarget> = Vec::new();
        let mut pending = Vec::new();
        let mut retained_keys = std::collections::HashSet::from(["windows".to_string()]);

        ready.push(CliTarget {
            backend: Box::new(WindowsFsBackend::new()),
            base_url: crate::proxy_server::proxy_base_url(),
            installed: managed.intersect(InstalledTools::ALL),
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
            retained_keys.insert(row.name.clone());
            let installed = managed.intersect(InstalledTools {
                claude: row.has_claude,
                codex: row.has_codex,
                gemini: row.has_gemini,
            });
            if !installed.claude && !installed.codex && !installed.gemini {
                continue;
            }
            let (Some(url), Some(home)) = (row.resolved_url.clone(), row.home.clone()) else {
                let reason = if row.home.is_none() {
                    "WSL home has not been probed"
                } else {
                    "WSL relay URL is unreachable"
                };
                log::warn!("WSL distro {} is pending: {reason}", row.name);
                pending.push(PendingWslTarget {
                    name: row.name,
                    home: row.home,
                    installed,
                    reason: reason.into(),
                });
                continue;
            };
            ready.push(CliTarget {
                backend: Box::new(WslBackend {
                    distro: row.name.clone(),
                    home: home.clone(),
                }),
                base_url: url,
                installed,
                label: format!("wsl:{}", row.name),
                snapshot_meta: SnapshotMeta {
                    target_type: TargetType::Wsl,
                    distro_name: Some(row.name),
                    home: Some(home),
                },
            });
        }

        ApplyPlan {
            ready,
            pending,
            retained_keys,
        }
    }

    pub fn build_apply_targets(&self) -> Vec<crate::cli_target::CliTarget> {
        self.build_apply_plan().ready
    }

    pub async fn retry_pending_wsl_apply(&self) -> Result<(), AppError> {
        if !crate::config_writer::lifecycle::has_pending_wsl() {
            return Ok(());
        }
        let active = self.db.get_active_config()?;
        let Some(gateway_id) = active.gateway_id.as_deref() else {
            return Ok(());
        };
        let gateway = self
            .db
            .get_gateway(gateway_id)?
            .ok_or_else(|| AppError::Config("active gateway not found".into()))?;
        let key_id = pick_key_id(&gateway, Some(&active))
            .ok_or_else(|| AppError::Config("active gateway key not found".into()))?;
        let gateway_id =
            Uuid::parse_str(gateway_id).map_err(|error| AppError::Config(error.to_string()))?;
        let key_id =
            Uuid::parse_str(&key_id).map_err(|error| AppError::Config(error.to_string()))?;
        self.set_active(
            gateway_id,
            key_id,
            ModelSelection {
                claude: active.claude_model,
                claude_subagent: active.claude_subagent_model,
                claude_small: active.claude_small_model,
                codex: active.codex_model,
                codex_subagent: active.codex_subagent_model,
                gemini: active.gemini_model,
                claude_extra: ClaudeExtraSelection::Inherit,
            },
        )
        .await
    }

    /// Build the WSL detection state machine. The caller is responsible for
    /// spawning `sm.clone().run()` on the appropriate runtime (tokio for the
    /// headless agent, `tauri::async_runtime` for the GUI). Returns `None`
    /// if `proxy` isn't attached (test code or pre-startup).
    pub fn spawn_wsl_state_machine(&self) -> Option<Arc<crate::wsl::state::StateMachine>> {
        let proxy = self.proxy.as_ref()?.clone();
        Some(crate::wsl::state::StateMachine::new(
            Arc::new(self.clone()),
            proxy,
        ))
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

fn classify_models(ids: impl IntoIterator<Item = String>) -> ModelCatalog {
    let mut claude = Vec::new();
    let mut codex = Vec::new();
    let mut gemini = Vec::new();

    for id in ids {
        let id_lower = id.to_lowercase();
        let family_id = id_lower.rsplit('/').next().unwrap_or(&id_lower);
        if family_id.starts_with("gpt-5.6") {
            claude.push(id.clone());
            codex.push(id);
        } else if family_id.starts_with("claude") {
            claude.push(id);
        } else if id_lower.contains("gpt")
            || id_lower.contains("o1")
            || id_lower.contains("o3")
            || id_lower.contains("o4")
            || id_lower.contains("codex")
        {
            codex.push(id);
        } else if id_lower.contains("gemini") {
            gemini.push(id);
        } else {
            // Unknown provider — add to claude bucket as a fallback.
            claude.push(id);
        }
    }

    ModelCatalog {
        claude,
        codex,
        gemini,
    }
}

fn range_to_period(r: TimeRange) -> &'static str {
    match r {
        TimeRange::Today => "today",
        TimeRange::Week => "week",
        TimeRange::Days7 => "7d",
        TimeRange::Days30 => "30d",
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_models, Service};
    use crate::events::NullSink;
    use crate::wsl::distro::DistroRow;
    use crate::Database;
    use std::sync::Arc;

    #[test]
    fn unresolved_selected_wsl_is_retained_as_pending() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.upsert_wsl_distro(&DistroRow {
            name: "Offline Distro".into(),
            is_default: false,
            selected: true,
            home: None,
            user: None,
            has_claude: true,
            has_codex: true,
            has_gemini: false,
            resolved_url: None,
            probed_at: Some("now".into()),
        })
        .unwrap();
        let service = Service::new(db, Arc::new(NullSink));
        let plan = service.build_apply_plan();
        assert_eq!(plan.ready.len(), 1);
        assert_eq!(plan.pending.len(), 1);
        assert_eq!(plan.pending[0].name, "Offline Distro");
        assert!(plan.retained_keys.contains("Offline Distro"));
    }

    #[test]
    fn gpt_5_6_is_available_to_claude_and_codex() {
        let catalog = classify_models([
            "CLAUDE-opus-4.7".to_string(),
            "GPT-5.6-sol-fast".to_string(),
            "vendor/gpt-5.6-code".to_string(),
            "vendor/claude-sonnet".to_string(),
            "gpt-5.5-codex".to_string(),
            "gemini-3-pro".to_string(),
        ]);

        assert_eq!(
            catalog.claude,
            [
                "CLAUDE-opus-4.7",
                "GPT-5.6-sol-fast",
                "vendor/gpt-5.6-code",
                "vendor/claude-sonnet",
            ]
        );
        assert_eq!(
            catalog.codex,
            ["GPT-5.6-sol-fast", "vendor/gpt-5.6-code", "gpt-5.5-codex"]
        );
        assert_eq!(catalog.gemini, ["gemini-3-pro"]);
    }

    #[test]
    fn unknown_models_keep_the_existing_claude_fallback() {
        let catalog = classify_models(["custom-model".to_string()]);
        assert_eq!(catalog.claude, ["custom-model"]);
        assert!(catalog.codex.is_empty());
        assert!(catalog.gemini.is_empty());
    }
}
