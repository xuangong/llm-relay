use crate::keystore;
use crate::AppError;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

pub const DEFAULT_CLAUDE_EXTRA_CONFIG_ID: &str = "00000000-0000-0000-0000-000000000001";
pub const MINIMAL_CLAUDE_EXTRA_CONFIG_ID: &str = "00000000-0000-0000-0000-000000000002";

pub struct Database {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gateway {
    pub id: String,
    pub name: String,
    pub url: String,
    pub auth_key: String,
    pub is_admin: bool,
    pub session_token: Option<String>,
    pub user_id: Option<String>,
    pub user_name: Option<String>,
    pub sort_order: i32,
    pub created_at: String,
    // Per-gateway model preferences (persisted so each gateway remembers its own choices)
    pub claude_model: Option<String>,
    pub claude_subagent_model: Option<String>,
    pub claude_small_model: Option<String>,
    pub codex_model: Option<String>,
    pub codex_subagent_model: Option<String>,
    pub gemini_model: Option<String>,
    pub preferred_key_id: Option<String>,
    pub claude_extra_config_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeExtraConfig {
    pub id: String,
    pub name: String,
    pub env: BTreeMap<String, String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayWithHealth {
    #[serde(flatten)]
    pub gateway: Gateway,
    pub is_healthy: bool,
    pub latency_ms: Option<i64>,
    pub model_count: Option<i32>,
    pub last_checked: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveConfig {
    pub gateway_id: Option<String>,
    pub key_id: Option<String>,
    pub key_name: Option<String>,
    pub key_value: Option<String>,
    pub claude_model: Option<String>,
    pub claude_subagent_model: Option<String>,
    pub claude_small_model: Option<String>,
    pub codex_model: Option<String>,
    pub codex_subagent_model: Option<String>,
    pub gemini_model: Option<String>,
    pub claude_extra_config_id: Option<String>,
    pub auto_switch: bool,
    pub applied_at: Option<String>,
    pub last_switched_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCache {
    pub gateway_id: String,
    pub is_healthy: bool,
    pub latency_ms: Option<i64>,
    pub model_count: Option<i32>,
    pub last_checked: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthLogEntry {
    pub is_healthy: bool,
    pub latency_ms: Option<i64>,
    pub checked_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficLogEntry {
    pub id: i64,
    pub gateway_id: String,
    pub gateway_name: Option<String>,
    pub path: String,
    pub status: u16,
    pub latency_ms: u64,
    pub error_detail: Option<String>,
    pub logged_at: String,
    /// Whether this row's path is muted. Carried on the row (rather than just
    /// filtered out) so a caller that asks to see muted rows can render them as
    /// muted instead of leaving the user wondering where they went.
    pub suppressed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuppressedPath {
    pub path: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub requests: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummaryByGateway {
    pub gateway_id: String,
    pub gateway_name: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub requests: i64,
}

impl Database {
    pub fn init(config_dir: &Path) -> Result<Self, AppError> {
        let db_path = config_dir.join("config.db");
        let existed = db_path.exists();
        let conn = Connection::open(db_path)?;
        Self::apply_schema_and_migrations(&conn)?;
        Self::initialize_managed_clients(&conn, existed)?;
        Ok(Database {
            conn: Mutex::new(conn),
        })
    }

    /// Test-only constructor: in-memory SQLite with full schema and
    /// `user_version` pinned at the latest. Migrations are skipped because
    /// they touch the OS keystore (v6/v7) and would force every test to
    /// call `keystore::init()` first. Tests use fresh schemas, never the
    /// migration path, so this is safe.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, AppError> {
        let conn = Connection::open_in_memory()?;
        // Base schema (idempotent) — copy of the block in
        // apply_schema_and_migrations to avoid pulling in keystore work.
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS gateways (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                url TEXT NOT NULL,
                auth_key TEXT NOT NULL,
                is_admin INTEGER DEFAULT 0,
                session_token TEXT,
                sort_order INTEGER DEFAULT 0,
                created_at TEXT NOT NULL,
                user_id TEXT,
                user_name TEXT,
                claude_model TEXT,
                claude_subagent_model TEXT,
                claude_small_model TEXT,
                codex_model TEXT,
                codex_subagent_model TEXT,
                gemini_model TEXT,
                preferred_key_id TEXT,
                claude_extra_config_id TEXT
            );

            CREATE TABLE IF NOT EXISTS active_config (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                gateway_id TEXT,
                key_id TEXT,
                key_name TEXT,
                key_value TEXT,
                claude_model TEXT,
                claude_subagent_model TEXT,
                claude_small_model TEXT,
                codex_model TEXT,
                codex_subagent_model TEXT,
                gemini_model TEXT,
                claude_extra_config_id TEXT,
                auto_switch INTEGER DEFAULT 1,
                applied_at TEXT,
                last_switched_at TEXT
            );

            CREATE TABLE IF NOT EXISTS health_cache (
                gateway_id TEXT PRIMARY KEY,
                is_healthy INTEGER DEFAULT 0,
                latency_ms INTEGER,
                model_count INTEGER,
                last_checked TEXT
            );

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT
            );

            CREATE TABLE IF NOT EXISTS claude_extra_configs (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL COLLATE NOCASE UNIQUE,
                env_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS health_check_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                gateway_id TEXT NOT NULL,
                is_healthy INTEGER NOT NULL,
                latency_ms INTEGER,
                checked_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS traffic_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                gateway_id TEXT NOT NULL,
                path TEXT NOT NULL,
                status INTEGER NOT NULL,
                latency_ms INTEGER NOT NULL,
                error_detail TEXT,
                logged_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS usage_log (
                gateway_id TEXT NOT NULL,
                model TEXT NOT NULL,
                hour TEXT NOT NULL,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                requests INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (gateway_id, model, hour)
            );

            CREATE TABLE IF NOT EXISTS wsl_distros (
                name          TEXT PRIMARY KEY,
                is_default    INTEGER NOT NULL DEFAULT 0,
                selected      INTEGER NOT NULL DEFAULT 0,
                home          TEXT,
                user          TEXT,
                has_claude    INTEGER NOT NULL DEFAULT 0,
                has_codex     INTEGER NOT NULL DEFAULT 0,
                has_gemini    INTEGER NOT NULL DEFAULT 0,
                resolved_url  TEXT,
                probed_at     TEXT
            );

            CREATE TABLE IF NOT EXISTS suppressed_paths (
                path       TEXT PRIMARY KEY,
                created_at TEXT NOT NULL
            );

            INSERT OR IGNORE INTO active_config (id, auto_switch) VALUES (1, 1);
            PRAGMA user_version = 14;
            ",
        )?;
        Self::seed_claude_extra_configs(&conn)?;
        Self::initialize_managed_clients(&conn, false)?;
        Ok(Database {
            conn: Mutex::new(conn),
        })
    }

    fn apply_schema_and_migrations(conn: &Connection) -> Result<(), AppError> {
        // Base schema (idempotent)
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS gateways (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                url TEXT NOT NULL,
                auth_key TEXT NOT NULL,
                is_admin INTEGER DEFAULT 0,
                session_token TEXT,
                sort_order INTEGER DEFAULT 0,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS active_config (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                gateway_id TEXT,
                key_id TEXT,
                key_name TEXT,
                key_value TEXT,
                claude_model TEXT,
                claude_small_model TEXT,
                codex_model TEXT,
                gemini_model TEXT,
                auto_switch INTEGER DEFAULT 1,
                applied_at TEXT
            );

            CREATE TABLE IF NOT EXISTS health_cache (
                gateway_id TEXT PRIMARY KEY,
                is_healthy INTEGER DEFAULT 0,
                latency_ms INTEGER,
                model_count INTEGER,
                last_checked TEXT
            );

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT
            );

            INSERT OR IGNORE INTO active_config (id, auto_switch) VALUES (1, 1);
            ",
        )?;

        // Run migrations
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or(0);

        if version < 1 {
            conn.execute_batch("ALTER TABLE active_config ADD COLUMN last_switched_at TEXT;")?;
            conn.execute_batch("PRAGMA user_version = 1")?;
        }

        if version < 2 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS health_check_log (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    gateway_id TEXT NOT NULL,
                    is_healthy INTEGER NOT NULL,
                    latency_ms INTEGER,
                    checked_at TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_health_log_gateway ON health_check_log (gateway_id, checked_at DESC);
                ",
            )?;
            conn.execute_batch("PRAGMA user_version = 2")?;
        }

        if version < 3 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS traffic_log (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    gateway_id TEXT NOT NULL,
                    path TEXT NOT NULL,
                    status INTEGER NOT NULL,
                    latency_ms INTEGER NOT NULL,
                    error_detail TEXT,
                    logged_at TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_traffic_log_time ON traffic_log (logged_at DESC);
                CREATE INDEX IF NOT EXISTS idx_traffic_log_gw ON traffic_log (gateway_id, logged_at DESC);
                ",
            )?;
            conn.execute_batch("PRAGMA user_version = 3")?;
        }

        if version < 4 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS usage_log (
                    gateway_id TEXT NOT NULL,
                    model TEXT NOT NULL,
                    hour TEXT NOT NULL,
                    input_tokens INTEGER NOT NULL DEFAULT 0,
                    output_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                    requests INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (gateway_id, model, hour)
                );
                CREATE INDEX IF NOT EXISTS idx_usage_log_hour ON usage_log (hour DESC);
                ",
            )?;
            conn.execute_batch("PRAGMA user_version = 4")?;
        }

        if version < 5 {
            conn.execute_batch(
                "ALTER TABLE gateways ADD COLUMN user_id TEXT;
                 ALTER TABLE gateways ADD COLUMN user_name TEXT;
                ",
            )?;
            conn.execute_batch("PRAGMA user_version = 5")?;
        }

        if version < 6 {
            // Migrate existing plaintext secrets from DB to OS keychain
            {
                let mut stmt = conn.prepare(
                    "SELECT id, auth_key, session_token FROM gateways WHERE auth_key != '' AND auth_key IS NOT NULL",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })?;
                for row in rows.flatten() {
                    let (id, auth_key, session_token) = row;
                    if !auth_key.is_empty() {
                        keystore::set_secret(&keystore::gw_auth_key(&id), &auth_key);
                    }
                    if let Some(ref token) = session_token {
                        if !token.is_empty() {
                            keystore::set_secret(&keystore::gw_session_token(&id), token);
                        }
                    }
                }
            }
            // Migrate active key_value
            {
                let kv: Option<String> = conn
                    .query_row(
                        "SELECT key_value FROM active_config WHERE id = 1",
                        [],
                        |row| row.get(0),
                    )
                    .ok()
                    .flatten();
                if let Some(ref v) = kv {
                    if !v.is_empty() {
                        keystore::set_secret(&keystore::active_key_value(), v);
                    }
                }
            }
            // Clear plaintext from DB
            conn.execute_batch(
                "UPDATE gateways SET auth_key = '', session_token = NULL;
                 UPDATE active_config SET key_value = NULL;
                ",
            )?;
            conn.execute_batch("PRAGMA user_version = 6")?;
        }

        // Migrate legacy per-key keychain entries into single unified entry.
        // Only run once — guarded by user_version 7.
        if version < 7 {
            let mut stmt = conn.prepare("SELECT id FROM gateways")?;
            let ids: Vec<String> = stmt
                .query_map([], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();
            keystore::migrate_legacy_entries(&ids);
            conn.execute_batch("PRAGMA user_version = 7")?;
        }

        // v8: per-gateway model preferences. Back-fill each gateway from the
        // (previously global) active_config row so existing users keep their
        // model selections on whichever gateway was last active.
        if version < 8 {
            conn.execute_batch(
                "ALTER TABLE gateways ADD COLUMN claude_model TEXT;
                 ALTER TABLE gateways ADD COLUMN claude_small_model TEXT;
                 ALTER TABLE gateways ADD COLUMN codex_model TEXT;
                 ALTER TABLE gateways ADD COLUMN gemini_model TEXT;",
            )?;

            let active: Option<(
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
            )> = conn
                .query_row(
                    "SELECT gateway_id, claude_model, claude_small_model, codex_model, gemini_model
                     FROM active_config WHERE id = 1",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .ok();
            if let Some((Some(gw_id), c, cs, cx, g)) = active {
                conn.execute(
                    "UPDATE gateways SET claude_model = ?1, claude_small_model = ?2,
                                         codex_model = ?3, gemini_model = ?4
                     WHERE id = ?5",
                    params![c, cs, cx, g, gw_id],
                )?;
            }
            conn.execute_batch("PRAGMA user_version = 8")?;
        }

        // v9: per-gateway preferred_key_id so config can be saved without activating.
        if version < 9 {
            conn.execute_batch("ALTER TABLE gateways ADD COLUMN preferred_key_id TEXT;")?;
            // Back-fill from active_config if the gateway matches.
            let active: Option<(Option<String>, Option<String>)> = conn
                .query_row(
                    "SELECT gateway_id, key_id FROM active_config WHERE id = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .ok();
            if let Some((Some(gw_id), Some(key_id))) = active {
                conn.execute(
                    "UPDATE gateways SET preferred_key_id = ?1 WHERE id = ?2",
                    params![key_id, gw_id],
                )?;
            }
            conn.execute_batch("PRAGMA user_version = 9")?;
        }

        if version < 10 {
            // WSL2 distro cache. Table exists on all platforms (harmless when
            // unused) so SQLite migrations are platform-agnostic.
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS wsl_distros (
                    name          TEXT PRIMARY KEY,
                    is_default    INTEGER NOT NULL DEFAULT 0,
                    selected      INTEGER NOT NULL DEFAULT 0,
                    home          TEXT,
                    user          TEXT,
                    has_claude    INTEGER NOT NULL DEFAULT 0,
                    has_codex     INTEGER NOT NULL DEFAULT 0,
                    has_gemini    INTEGER NOT NULL DEFAULT 0,
                    resolved_url  TEXT,
                    probed_at     TEXT
                );",
            )?;
            conn.execute_batch("PRAGMA user_version = 10")?;
        }

        if version < 11 {
            // Paths whose errors the user has muted in the traffic log. Keyed by
            // path alone: a probe like `/api/hello` is noise on every gateway,
            // not just the one that happened to log it first.
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS suppressed_paths (
                    path       TEXT PRIMARY KEY,
                    created_at TEXT NOT NULL
                );",
            )?;
            conn.execute_batch("PRAGMA user_version = 11")?;
        }

        if version < 12 {
            conn.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE gateways ADD COLUMN claude_subagent_model TEXT;
                 ALTER TABLE active_config ADD COLUMN claude_subagent_model TEXT;
                 PRAGMA user_version = 12;
                 COMMIT;",
            )?;
        }

        if version < 13 {
            conn.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE gateways ADD COLUMN codex_subagent_model TEXT;
                 ALTER TABLE active_config ADD COLUMN codex_subagent_model TEXT;
                 PRAGMA user_version = 13;
                 COMMIT;",
            )?;
        }

        if version < 14 {
            conn.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE claude_extra_configs (
                     id TEXT PRIMARY KEY,
                     name TEXT NOT NULL COLLATE NOCASE UNIQUE,
                     env_json TEXT NOT NULL,
                     created_at TEXT NOT NULL,
                     updated_at TEXT NOT NULL
                 );
                 ALTER TABLE gateways ADD COLUMN claude_extra_config_id TEXT;
                 ALTER TABLE active_config ADD COLUMN claude_extra_config_id TEXT;
                 PRAGMA user_version = 14;
                 COMMIT;",
            )?;
        }

        if version < 14 {
            Self::seed_claude_extra_configs(conn)?;
        }
        Ok(())
    }

    fn initialize_managed_clients(
        conn: &Connection,
        existing_database: bool,
    ) -> Result<(), AppError> {
        let value = if existing_database {
            r#"{"claude":true,"codex":true,"gemini":true}"#
        } else {
            r#"{"claude":false,"codex":true,"gemini":false}"#
        };
        conn.execute(
            "INSERT OR IGNORE INTO settings (key, value) VALUES ('managed_clients', ?1)",
            params![value],
        )?;
        Ok(())
    }

    pub fn get_managed_clients(&self) -> Result<crate::cli_target::ManagedClients, AppError> {
        let value = self
            .get_setting("managed_clients")?
            .ok_or_else(|| AppError::Config("managed_clients is not initialized".into()))?;
        let clients: crate::cli_target::ManagedClients = serde_json::from_str(&value)?;
        if !clients.any() {
            return Err(AppError::Config(
                "At least one managed client must be selected".into(),
            ));
        }
        Ok(clients)
    }

    pub fn set_managed_clients(
        &self,
        clients: crate::cli_target::ManagedClients,
    ) -> Result<(), AppError> {
        if !clients.any() {
            return Err(AppError::Config(
                "At least one managed client must be selected".into(),
            ));
        }
        self.set_setting("managed_clients", &serde_json::to_string(&clients)?)
    }

    fn seed_claude_extra_configs(conn: &Connection) -> Result<(), AppError> {
        let full = serde_json::json!({
            "CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS": "2",
            "CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH": "1",
            "CLAUDE_CODE_FORK_SUBAGENT": "0",
            "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS": "1",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
            "DISABLE_AUTOUPDATER": "1",
            "DISABLE_NON_ESSENTIAL_MODEL_CALLS": "1",
        });
        let minimal = serde_json::json!({
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
            "DISABLE_AUTOUPDATER": "1",
        });
        conn.execute(
            "INSERT OR IGNORE INTO claude_extra_configs
             (id, name, env_json, created_at, updated_at) VALUES (?1, '配置项一', ?2, ?3, ?3)",
            params![
                DEFAULT_CLAUDE_EXTRA_CONFIG_ID,
                serde_json::to_string(&full)?,
                chrono::Utc::now().to_rfc3339(),
            ],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO claude_extra_configs
             (id, name, env_json, created_at, updated_at) VALUES (?1, '配置项二', ?2, ?3, ?3)",
            params![
                MINIMAL_CLAUDE_EXTRA_CONFIG_ID,
                serde_json::to_string(&minimal)?,
                chrono::Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    // ─── Gateway CRUD ───

    pub fn add_gateway(&self, gw: &Gateway) -> Result<(), AppError> {
        // Store secrets in OS keychain
        keystore::set_secret(&keystore::gw_auth_key(&gw.id), &gw.auth_key);
        if let Some(ref token) = gw.session_token {
            keystore::set_secret(&keystore::gw_session_token(&gw.id), token);
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO gateways (id, name, url, auth_key, is_admin, session_token, user_id, user_name, sort_order, created_at,
                                   claude_model, claude_subagent_model, claude_small_model, codex_model, codex_subagent_model, gemini_model, preferred_key_id, claude_extra_config_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                gw.id,
                gw.name,
                gw.url,
                "",  // empty — real value in keychain
                gw.is_admin as i32,
                Option::<String>::None,  // empty — real value in keychain
                gw.user_id,
                gw.user_name,
                gw.sort_order,
                gw.created_at,
                gw.claude_model,
                gw.claude_subagent_model,
                gw.claude_small_model,
                gw.codex_model,
                gw.codex_subagent_model,
                gw.gemini_model,
                gw.preferred_key_id,
                gw.claude_extra_config_id,
            ],
        )?;
        Ok(())
    }

    pub fn list_gateways(&self) -> Result<Vec<Gateway>, AppError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, url, auth_key, is_admin, session_token, user_id, user_name, sort_order, created_at,
                    claude_model, claude_subagent_model, claude_small_model, codex_model, codex_subagent_model, gemini_model, preferred_key_id, claude_extra_config_id
             FROM gateways ORDER BY sort_order ASC, created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Gateway {
                id: row.get(0)?,
                name: row.get(1)?,
                url: row.get(2)?,
                auth_key: row.get(3)?,
                is_admin: row.get::<_, i32>(4)? != 0,
                session_token: row.get(5)?,
                user_id: row.get(6)?,
                user_name: row.get(7)?,
                sort_order: row.get(8)?,
                created_at: row.get(9)?,
                claude_model: row.get(10)?,
                claude_subagent_model: row.get(11)?,
                claude_small_model: row.get(12)?,
                codex_model: row.get(13)?,
                codex_subagent_model: row.get(14)?,
                gemini_model: row.get(15)?,
                preferred_key_id: row.get(16)?,
                claude_extra_config_id: row.get(17)?,
            })
        })?;
        let mut gateways: Vec<Gateway> = rows.filter_map(|r| r.ok()).collect();
        // Fill secrets from keychain
        for gw in &mut gateways {
            if let Some(key) = keystore::get_secret(&keystore::gw_auth_key(&gw.id)) {
                gw.auth_key = key;
            }
            if let Some(token) = keystore::get_secret(&keystore::gw_session_token(&gw.id)) {
                gw.session_token = Some(token);
            }
        }
        Ok(gateways)
    }

    pub fn get_gateway(&self, id: &str) -> Result<Option<Gateway>, AppError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, url, auth_key, is_admin, session_token, user_id, user_name, sort_order, created_at,
                    claude_model, claude_subagent_model, claude_small_model, codex_model, codex_subagent_model, gemini_model, preferred_key_id, claude_extra_config_id
             FROM gateways WHERE id = ?1",
        )?;
        let result = stmt.query_row(params![id], |row| {
            Ok(Gateway {
                id: row.get(0)?,
                name: row.get(1)?,
                url: row.get(2)?,
                auth_key: row.get(3)?,
                is_admin: row.get::<_, i32>(4)? != 0,
                session_token: row.get(5)?,
                user_id: row.get(6)?,
                user_name: row.get(7)?,
                sort_order: row.get(8)?,
                created_at: row.get(9)?,
                claude_model: row.get(10)?,
                claude_subagent_model: row.get(11)?,
                claude_small_model: row.get(12)?,
                codex_model: row.get(13)?,
                codex_subagent_model: row.get(14)?,
                gemini_model: row.get(15)?,
                preferred_key_id: row.get(16)?,
                claude_extra_config_id: row.get(17)?,
            })
        });
        match result {
            Ok(mut gw) => {
                if let Some(key) = keystore::get_secret(&keystore::gw_auth_key(&gw.id)) {
                    gw.auth_key = key;
                }
                if let Some(token) = keystore::get_secret(&keystore::gw_session_token(&gw.id)) {
                    gw.session_token = Some(token);
                }
                Ok(Some(gw))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn update_gateway(
        &self,
        id: &str,
        name: &str,
        url: &str,
        auth_key: &str,
    ) -> Result<(), AppError> {
        keystore::set_secret(&keystore::gw_auth_key(id), auth_key);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE gateways SET name = ?1, url = ?2, auth_key = ?3 WHERE id = ?4",
            params![name, url, "", id],
        )?;
        Ok(())
    }

    pub fn update_gateway_session(
        &self,
        id: &str,
        is_admin: bool,
        session_token: Option<&str>,
    ) -> Result<(), AppError> {
        if let Some(token) = session_token {
            keystore::set_secret(&keystore::gw_session_token(id), token);
        } else {
            keystore::delete_secret(&keystore::gw_session_token(id));
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE gateways SET is_admin = ?1, session_token = ?2 WHERE id = ?3",
            params![is_admin as i32, Option::<String>::None, id],
        )?;
        Ok(())
    }

    pub fn delete_gateway(&self, id: &str) -> Result<(), AppError> {
        // Remove secrets from keychain
        keystore::delete_secret(&keystore::gw_auth_key(id));
        keystore::delete_secret(&keystore::gw_session_token(id));
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM health_cache WHERE gateway_id = ?1",
            params![id],
        )?;
        conn.execute(
            "DELETE FROM health_check_log WHERE gateway_id = ?1",
            params![id],
        )?;
        conn.execute("DELETE FROM gateways WHERE id = ?1", params![id])?;
        conn.execute(
            "UPDATE active_config SET gateway_id = NULL, key_id = NULL, key_name = NULL, key_value = NULL
             WHERE gateway_id = ?1",
            params![id],
        )?;
        // Also clear active key_value from keychain if this gateway was active
        keystore::delete_secret(&keystore::active_key_value());
        Ok(())
    }

    pub fn reorder_gateways(&self, ids: &[String]) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        for (i, id) in ids.iter().enumerate() {
            conn.execute(
                "UPDATE gateways SET sort_order = ?1 WHERE id = ?2",
                params![i as i32, id],
            )?;
        }
        Ok(())
    }

    /// Persist per-gateway model preferences and preferred key.
    pub fn update_gateway_config(
        &self,
        id: &str,
        preferred_key_id: Option<&str>,
        claude: Option<&str>,
        claude_subagent: Option<&str>,
        claude_small: Option<&str>,
        codex: Option<&str>,
        codex_subagent: Option<&str>,
        gemini: Option<&str>,
        claude_extra_config_id: Option<&str>,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE gateways SET
                preferred_key_id      = COALESCE(?1, preferred_key_id),
                claude_model          = ?2,
                claude_subagent_model = ?3,
                claude_small_model    = ?4,
                codex_model           = ?5,
                codex_subagent_model  = ?6,
                gemini_model          = ?7,
                claude_extra_config_id = ?8
             WHERE id = ?9",
            params![
                preferred_key_id,
                claude,
                claude_subagent,
                claude_small,
                codex,
                codex_subagent,
                gemini,
                claude_extra_config_id,
                id,
            ],
        )?;
        Ok(())
    }

    // ─── Claude Extra Configs ───

    pub fn list_claude_extra_configs(&self) -> Result<Vec<ClaudeExtraConfig>, AppError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, env_json, created_at, updated_at
             FROM claude_extra_configs ORDER BY created_at, name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |row| {
            let env_json: String = row.get(2)?;
            let env = serde_json::from_str(&env_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok(ClaudeExtraConfig {
                id: row.get(0)?,
                name: row.get(1)?,
                env,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_claude_extra_config(&self, id: &str) -> Result<Option<ClaudeExtraConfig>, AppError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT id, name, env_json, created_at, updated_at
             FROM claude_extra_configs WHERE id = ?1",
            params![id],
            |row| {
                let env_json: String = row.get(2)?;
                let env = serde_json::from_str(&env_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok(ClaudeExtraConfig {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    env,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        );
        match result {
            Ok(config) => Ok(Some(config)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn create_claude_extra_config(
        &self,
        name: &str,
        env: &BTreeMap<String, String>,
    ) -> Result<ClaudeExtraConfig, AppError> {
        let now = chrono::Utc::now().to_rfc3339();
        let config = ClaudeExtraConfig {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            env: env.clone(),
            created_at: now.clone(),
            updated_at: now,
        };
        let env_json = serde_json::to_string(&config.env)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO claude_extra_configs (id, name, env_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                config.id,
                config.name,
                env_json,
                config.created_at,
                config.updated_at
            ],
        )?;
        Ok(config)
    }

    pub fn update_claude_extra_config(
        &self,
        id: &str,
        name: &str,
        env: &BTreeMap<String, String>,
    ) -> Result<ClaudeExtraConfig, AppError> {
        let updated_at = chrono::Utc::now().to_rfc3339();
        let env_json = serde_json::to_string(env)?;
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE claude_extra_configs SET name = ?1, env_json = ?2, updated_at = ?3
             WHERE id = ?4",
            params![name, env_json, updated_at, id],
        )?;
        if changed == 0 {
            return Err(AppError::Config(format!(
                "Claude Extra config {id} not found"
            )));
        }
        drop(conn);
        self.get_claude_extra_config(id)?.ok_or_else(|| {
            AppError::Config(format!("Claude Extra config {id} not found after update"))
        })
    }

    pub fn delete_claude_extra_config(&self, id: &str) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        let references: i64 = conn.query_row(
            "SELECT
                 (SELECT COUNT(*) FROM gateways WHERE claude_extra_config_id = ?1) +
                 (SELECT COUNT(*) FROM active_config WHERE claude_extra_config_id = ?1)",
            params![id],
            |row| row.get(0),
        )?;
        if references > 0 {
            return Err(AppError::Config(
                "This Claude Extra config is still selected by a gateway".into(),
            ));
        }
        let changed = conn.execute(
            "DELETE FROM claude_extra_configs WHERE id = ?1",
            params![id],
        )?;
        if changed == 0 {
            return Err(AppError::Config(format!(
                "Claude Extra config {id} not found"
            )));
        }
        Ok(())
    }

    pub fn default_claude_extra_config_id(&self) -> Result<Option<String>, AppError> {
        Ok(self
            .get_claude_extra_config(DEFAULT_CLAUDE_EXTRA_CONFIG_ID)?
            .map(|config| config.id))
    }

    // ─── Active Config ───

    pub fn get_active_config(&self) -> Result<ActiveConfig, AppError> {
        let conn = self.conn.lock().unwrap();
        let mut result = conn.query_row(
            "SELECT gateway_id, key_id, key_name, key_value,
                    claude_model, claude_subagent_model, claude_small_model,
                    codex_model, codex_subagent_model, gemini_model,
                    claude_extra_config_id, auto_switch, applied_at, last_switched_at
             FROM active_config WHERE id = 1",
            [],
            |row| {
                Ok(ActiveConfig {
                    gateway_id: row.get(0)?,
                    key_id: row.get(1)?,
                    key_name: row.get(2)?,
                    key_value: row.get(3)?,
                    claude_model: row.get(4)?,
                    claude_subagent_model: row.get(5)?,
                    claude_small_model: row.get(6)?,
                    codex_model: row.get(7)?,
                    codex_subagent_model: row.get(8)?,
                    gemini_model: row.get(9)?,
                    claude_extra_config_id: row.get(10)?,
                    auto_switch: row.get::<_, i32>(11)? != 0,
                    applied_at: row.get(12)?,
                    last_switched_at: row.get(13)?,
                })
            },
        )?;
        // Fill key_value from keychain
        if let Some(kv) = keystore::get_secret(&keystore::active_key_value()) {
            result.key_value = Some(kv);
        }
        Ok(result)
    }

    pub fn set_active_config(&self, config: &ActiveConfig) -> Result<(), AppError> {
        // Publish the fallible SQLite row first. Callers coordinate readers with
        // the switch lock, and keychain mutations do not return errors, so a SQL
        // failure must leave the previous active secret untouched.
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE active_config SET
                gateway_id = ?1, key_id = ?2, key_name = ?3, key_value = ?4,
                claude_model = ?5, claude_subagent_model = ?6, claude_small_model = ?7,
                codex_model = ?8, codex_subagent_model = ?9, gemini_model = ?10,
                claude_extra_config_id = ?11,
                auto_switch = ?12, applied_at = ?13, last_switched_at = ?14
             WHERE id = 1",
            params![
                config.gateway_id,
                config.key_id,
                config.key_name,
                Option::<String>::None, // empty — real value in keychain
                config.claude_model,
                config.claude_subagent_model,
                config.claude_small_model,
                config.codex_model,
                config.codex_subagent_model,
                config.gemini_model,
                config.claude_extra_config_id,
                config.auto_switch as i32,
                config.applied_at,
                config.last_switched_at,
            ],
        )?;
        drop(conn);

        if let Some(ref kv) = config.key_value {
            keystore::set_secret(&keystore::active_key_value(), kv);
        } else {
            keystore::delete_secret(&keystore::active_key_value());
        }
        Ok(())
    }

    // ─── Health Cache ───

    pub fn update_health(&self, health: &HealthCache) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO health_cache (gateway_id, is_healthy, latency_ms, model_count, last_checked)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                health.gateway_id,
                health.is_healthy as i32,
                health.latency_ms,
                health.model_count,
                health.last_checked,
            ],
        )?;
        Ok(())
    }

    pub fn get_health(&self, gateway_id: &str) -> Result<Option<HealthCache>, AppError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT gateway_id, is_healthy, latency_ms, model_count, last_checked
             FROM health_cache WHERE gateway_id = ?1",
            params![gateway_id],
            |row| {
                Ok(HealthCache {
                    gateway_id: row.get(0)?,
                    is_healthy: row.get::<_, i32>(1)? != 0,
                    latency_ms: row.get(2)?,
                    model_count: row.get(3)?,
                    last_checked: row.get(4)?,
                })
            },
        );
        match result {
            Ok(h) => Ok(Some(h)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn list_gateways_with_health(&self) -> Result<Vec<GatewayWithHealth>, AppError> {
        let gateways = self.list_gateways()?;
        let mut result = Vec::new();
        for gw in gateways {
            let health = self.get_health(&gw.id)?;
            result.push(GatewayWithHealth {
                is_healthy: health.as_ref().map(|h| h.is_healthy).unwrap_or(false),
                latency_ms: health.as_ref().and_then(|h| h.latency_ms),
                model_count: health.as_ref().and_then(|h| h.model_count),
                last_checked: health.as_ref().and_then(|h| h.last_checked.clone()),
                gateway: gw,
            });
        }
        Ok(result)
    }

    // ─── Health Log ───

    pub fn add_health_log(
        &self,
        gateway_id: &str,
        is_healthy: bool,
        latency_ms: Option<i64>,
    ) -> Result<(), AppError> {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO health_check_log (gateway_id, is_healthy, latency_ms, checked_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![gateway_id, is_healthy as i32, latency_ms, now],
        )?;
        // Prune to last 1440 entries per gateway (24h at 1-min intervals)
        conn.execute(
            "DELETE FROM health_check_log WHERE gateway_id = ?1
             AND id NOT IN (
                 SELECT id FROM health_check_log WHERE gateway_id = ?1
                 ORDER BY checked_at DESC LIMIT 1440
             )",
            params![gateway_id],
        )?;
        Ok(())
    }

    pub fn get_health_log(
        &self,
        gateway_id: &str,
        limit: usize,
    ) -> Result<Vec<HealthLogEntry>, AppError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT is_healthy, latency_ms, checked_at
             FROM health_check_log WHERE gateway_id = ?1
             ORDER BY checked_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![gateway_id, limit as i64], |row| {
            Ok(HealthLogEntry {
                is_healthy: row.get::<_, i32>(0)? != 0,
                latency_ms: row.get(1)?,
                checked_at: row.get(2)?,
            })
        })?;
        let mut entries: Vec<HealthLogEntry> = rows.filter_map(|r| r.ok()).collect();
        entries.reverse(); // oldest first for chart rendering
        Ok(entries)
    }

    // ─── Settings ───

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, AppError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |row| row.get(0),
        );
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    // ─── Traffic Log ───

    pub fn add_traffic_log(
        &self,
        gateway_id: &str,
        path: &str,
        status: u16,
        latency_ms: u64,
        error_detail: Option<&str>,
    ) -> Result<(), AppError> {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO traffic_log (gateway_id, path, status, latency_ms, error_detail, logged_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![gateway_id, path, status as i32, latency_ms as i64, error_detail, now],
        )?;
        // Purge entries older than 24 hours
        conn.execute(
            "DELETE FROM traffic_log WHERE logged_at < datetime('now', '-1 day')",
            [],
        )?;
        Ok(())
    }

    /// Returns recent anomalous traffic entries (newest first).
    /// `gateway_id = None` returns across all gateways.
    ///
    /// Muted paths are filtered in SQL rather than by the caller so that `limit`
    /// counts rows the user will actually see — otherwise a chatty suppressed
    /// path (a health probe firing every few seconds) would crowd real errors
    /// out of the window.
    pub fn get_traffic_log(
        &self,
        gateway_id: Option<&str>,
        limit: usize,
        include_suppressed: bool,
    ) -> Result<Vec<TrafficLogEntry>, AppError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT t.id, t.gateway_id, g.name, t.path, t.status, t.latency_ms, t.error_detail,
                    t.logged_at,
                    EXISTS (SELECT 1 FROM suppressed_paths s WHERE s.path = t.path)
             FROM traffic_log t LEFT JOIN gateways g ON g.id = t.gateway_id
             WHERE (?1 IS NULL OR t.gateway_id = ?1)
               AND (?2 OR NOT EXISTS (SELECT 1 FROM suppressed_paths s WHERE s.path = t.path))
             ORDER BY t.logged_at DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![gateway_id, include_suppressed, limit as i64],
            |row| {
                Ok(TrafficLogEntry {
                    id: row.get(0)?,
                    gateway_id: row.get(1)?,
                    gateway_name: row.get(2)?,
                    path: row.get(3)?,
                    status: row.get::<_, i32>(4)? as u16,
                    latency_ms: row.get::<_, i64>(5)? as u64,
                    error_detail: row.get(6)?,
                    logged_at: row.get(7)?,
                    suppressed: row.get(8)?,
                })
            },
        )?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    // ─── Suppressed Paths ───

    pub fn list_suppressed_paths(&self) -> Result<Vec<SuppressedPath>, AppError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT path, created_at FROM suppressed_paths ORDER BY created_at DESC")?;
        let rows = stmt.query_map([], |row| {
            Ok(SuppressedPath {
                path: row.get(0)?,
                created_at: row.get(1)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Mute a path. Idempotent — re-muting keeps the original timestamp.
    pub fn suppress_path(&self, path: &str) -> Result<(), AppError> {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO suppressed_paths (path, created_at) VALUES (?1, ?2)",
            params![path, now],
        )?;
        Ok(())
    }

    /// Unmute a path. Past rows for it are still in `traffic_log` (subject to
    /// the 24h purge), so they reappear immediately.
    pub fn unsuppress_path(&self, path: &str) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM suppressed_paths WHERE path = ?1",
            params![path],
        )?;
        Ok(())
    }

    // ─── Usage Log ───

    pub fn record_usage(
        &self,
        gateway_id: &str,
        model: &str,
        input_tokens: i64,
        output_tokens: i64,
        cache_read_tokens: i64,
        cache_creation_tokens: i64,
    ) -> Result<(), AppError> {
        if input_tokens == 0 && output_tokens == 0 {
            return Ok(());
        }
        // Current UTC hour as "YYYY-MM-DDTHH"
        let hour = chrono::Utc::now().format("%Y-%m-%dT%H").to_string();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO usage_log (gateway_id, model, hour, input_tokens, output_tokens,
                                    cache_read_tokens, cache_creation_tokens, requests)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)
             ON CONFLICT (gateway_id, model, hour) DO UPDATE SET
                input_tokens = input_tokens + excluded.input_tokens,
                output_tokens = output_tokens + excluded.output_tokens,
                cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens,
                cache_creation_tokens = cache_creation_tokens + excluded.cache_creation_tokens,
                requests = requests + 1",
            params![
                gateway_id,
                model,
                hour,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens
            ],
        )?;
        Ok(())
    }

    /// Aggregate usage grouped by model for a given time period.
    /// period: "today" | "week" | "7d" | "30d"
    /// gateway_id: None = all gateways
    pub fn get_usage_stats(
        &self,
        gateway_id: Option<&str>,
        period: &str,
    ) -> Result<Vec<UsageSummary>, AppError> {
        let since = match period {
            "today" => "strftime('%Y-%m-%dT%H', 'now', 'start of day')".to_string(),
            "week" => "strftime('%Y-%m-%dT%H', date('now', 'weekday 1', '-7 days'))".to_string(),
            "7d" => "strftime('%Y-%m-%dT%H', datetime('now', '-7 days'))".to_string(),
            "30d" => "strftime('%Y-%m-%dT%H', datetime('now', '-30 days'))".to_string(),
            _ => "strftime('%Y-%m-%dT%H', 'now', 'start of day')".to_string(),
        };

        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT model,
                    SUM(input_tokens), SUM(output_tokens),
                    SUM(cache_read_tokens), SUM(cache_creation_tokens),
                    SUM(requests)
             FROM usage_log
             WHERE hour >= ({since}){gw_filter}
             GROUP BY model
             ORDER BY SUM(input_tokens) + SUM(output_tokens) DESC",
            since = since,
            gw_filter = if gateway_id.is_some() {
                " AND gateway_id = ?1"
            } else {
                ""
            },
        );

        let entries = if let Some(gid) = gateway_id {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![gid], |row| {
                Ok(UsageSummary {
                    model: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    cache_read_tokens: row.get(3)?,
                    cache_creation_tokens: row.get(4)?,
                    requests: row.get(5)?,
                })
            })?;
            rows.filter_map(|r| r.ok()).collect()
        } else {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok(UsageSummary {
                    model: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    cache_read_tokens: row.get(3)?,
                    cache_creation_tokens: row.get(4)?,
                    requests: row.get(5)?,
                })
            })?;
            rows.filter_map(|r| r.ok()).collect()
        };
        Ok(entries)
    }

    /// Aggregate usage grouped by (gateway, model) for the TUI Usage tab.
    pub fn get_usage_stats_by_gateway(
        &self,
        period: &str,
    ) -> Result<Vec<UsageSummaryByGateway>, AppError> {
        let since = match period {
            "today" => "strftime('%Y-%m-%dT%H', 'now', 'start of day')".to_string(),
            "7d" => "strftime('%Y-%m-%dT%H', datetime('now', '-7 days'))".to_string(),
            "30d" => "strftime('%Y-%m-%dT%H', datetime('now', '-30 days'))".to_string(),
            "all" => "'0000'".to_string(),
            _ => "strftime('%Y-%m-%dT%H', 'now', 'start of day')".to_string(),
        };

        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT u.gateway_id, COALESCE(g.name, u.gateway_id), u.model,
                    SUM(u.input_tokens), SUM(u.output_tokens), SUM(u.requests)
             FROM usage_log u
             LEFT JOIN gateways g ON g.id = u.gateway_id
             WHERE u.hour >= ({since})
             GROUP BY u.gateway_id, u.model
             ORDER BY SUM(u.input_tokens) + SUM(u.output_tokens) DESC",
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| {
            Ok(UsageSummaryByGateway {
                gateway_id: row.get(0)?,
                gateway_name: row.get(1)?,
                model: row.get(2)?,
                input_tokens: row.get(3)?,
                output_tokens: row.get(4)?,
                requests: row.get(5)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    // ─── WSL Distros ───

    pub fn list_wsl_distros(&self) -> Result<Vec<crate::wsl::distro::DistroRow>, AppError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT name, is_default, selected, home, user, has_claude, has_codex,
                    has_gemini, resolved_url, probed_at
             FROM wsl_distros ORDER BY name",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(crate::wsl::distro::DistroRow {
                    name: r.get(0)?,
                    is_default: r.get::<_, i64>(1)? != 0,
                    selected: r.get::<_, i64>(2)? != 0,
                    home: r.get(3)?,
                    user: r.get(4)?,
                    has_claude: r.get::<_, i64>(5)? != 0,
                    has_codex: r.get::<_, i64>(6)? != 0,
                    has_gemini: r.get::<_, i64>(7)? != 0,
                    resolved_url: r.get(8)?,
                    probed_at: r.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn upsert_wsl_distro(&self, row: &crate::wsl::distro::DistroRow) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO wsl_distros
             (name, is_default, selected, home, user, has_claude, has_codex, has_gemini, resolved_url, probed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(name) DO UPDATE SET
                is_default=excluded.is_default,
                home=excluded.home,
                user=excluded.user,
                has_claude=excluded.has_claude,
                has_codex=excluded.has_codex,
                has_gemini=excluded.has_gemini,
                resolved_url=excluded.resolved_url,
                probed_at=excluded.probed_at",
            params![
                row.name,
                row.is_default as i64,
                row.selected as i64,
                row.home,
                row.user,
                row.has_claude as i64,
                row.has_codex as i64,
                row.has_gemini as i64,
                row.resolved_url,
                row.probed_at,
            ],
        )?;
        Ok(())
    }

    pub fn set_wsl_distro_selected(&self, name: &str, selected: bool) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE wsl_distros SET selected = ?1 WHERE name = ?2",
            params![selected as i64, name],
        )?;
        Ok(())
    }

    pub fn delete_wsl_distro(&self, name: &str) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM wsl_distros WHERE name = ?1", params![name])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v12_migration_adds_subagent_columns_without_changing_small_model() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE gateways (
                 id TEXT PRIMARY KEY,
                 claude_small_model TEXT
             );
             CREATE TABLE active_config (
                 id INTEGER PRIMARY KEY,
                 auto_switch INTEGER DEFAULT 1,
                 claude_small_model TEXT
             );
             INSERT INTO gateways (id, claude_small_model) VALUES ('gw', 'claude-haiku');
             INSERT INTO active_config (id, claude_small_model) VALUES (1, 'claude-haiku');
             PRAGMA user_version = 11;",
        )
        .unwrap();

        Database::apply_schema_and_migrations(&conn).unwrap();

        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        let gateway_columns: Vec<String> = conn
            .prepare("PRAGMA table_info(gateways)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let active_columns: Vec<String> = conn
            .prepare("PRAGMA table_info(active_config)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let old_small: String = conn
            .query_row(
                "SELECT claude_small_model FROM gateways WHERE id = 'gw'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(version, 14);
        assert!(gateway_columns
            .iter()
            .any(|name| name == "claude_subagent_model"));
        assert!(active_columns
            .iter()
            .any(|name| name == "claude_subagent_model"));
        assert!(gateway_columns
            .iter()
            .any(|name| name == "codex_subagent_model"));
        assert!(active_columns
            .iter()
            .any(|name| name == "codex_subagent_model"));
        assert_eq!(old_small, "claude-haiku");
    }

    #[test]
    fn fresh_database_defaults_to_codex_only_and_preserves_changes() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(
            db.get_managed_clients().unwrap(),
            crate::cli_target::ManagedClients::CODEX_ONLY
        );
        db.set_managed_clients(crate::cli_target::ManagedClients::ALL)
            .unwrap();
        Database::initialize_managed_clients(&db.conn.lock().unwrap(), false).unwrap();
        assert_eq!(
            db.get_managed_clients().unwrap(),
            crate::cli_target::ManagedClients::ALL
        );
    }

    #[test]
    fn existing_database_without_setting_defaults_to_all() {
        let db = Database::open_in_memory().unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM settings WHERE key = 'managed_clients'", [])
            .unwrap();
        Database::initialize_managed_clients(&db.conn.lock().unwrap(), true).unwrap();
        assert_eq!(
            db.get_managed_clients().unwrap(),
            crate::cli_target::ManagedClients::ALL
        );
    }

    #[test]
    fn empty_managed_client_selection_is_rejected() {
        let db = Database::open_in_memory().unwrap();
        assert!(db
            .set_managed_clients(crate::cli_target::ManagedClients {
                claude: false,
                codex: false,
                gemini: false,
            })
            .is_err());
    }

    #[test]
    fn claude_extra_configs_seed_once_and_support_crud() {
        let db = Database::open_in_memory().unwrap();
        let seeded = db.list_claude_extra_configs().unwrap();
        assert_eq!(seeded.len(), 2);
        assert_eq!(seeded[0].name, "配置项一");
        assert_eq!(
            seeded[0].env.get("CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"),
            Some(&"2".to_string())
        );

        let env = BTreeMap::from([("TEST_FLAG".to_string(), "1".to_string())]);
        let created = db.create_claude_extra_config("Custom", &env).unwrap();
        assert_eq!(created.env, env);
        let updated_env = BTreeMap::from([("TEST_FLAG".to_string(), "0".to_string())]);
        let updated = db
            .update_claude_extra_config(&created.id, "Renamed", &updated_env)
            .unwrap();
        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.env, updated_env);
        db.delete_claude_extra_config(&created.id).unwrap();
        assert!(db.get_claude_extra_config(&created.id).unwrap().is_none());

        Database::seed_claude_extra_configs(&db.conn.lock().unwrap()).unwrap();
        assert_eq!(db.list_claude_extra_configs().unwrap().len(), 2);
    }

    #[test]
    fn deleting_selected_claude_extra_config_is_rejected() {
        let db = Database::open_in_memory().unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO gateways (id, name, url, auth_key, created_at, claude_extra_config_id)
                 VALUES ('gw', 'Gateway', 'http://example.invalid', '', 'now', ?1)",
                params![DEFAULT_CLAUDE_EXTRA_CONFIG_ID],
            )
            .unwrap();
        }
        assert!(db
            .delete_claude_extra_config(DEFAULT_CLAUDE_EXTRA_CONFIG_ID)
            .is_err());
    }

    #[test]
    fn gateway_and_active_config_round_trip_subagent_model() {
        let db = Database::open_in_memory().unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO gateways (
                     id, name, url, auth_key, created_at, claude_small_model
                 ) VALUES ('gw', 'Gateway', 'http://example.invalid', '', 'now', 'old-haiku')",
                [],
            )
            .unwrap();
        }

        db.update_gateway_config(
            "gw",
            None,
            Some("claude-opus"),
            Some("claude-sonnet"),
            Some("claude-haiku"),
            None,
            Some("gpt-5.6-sol-fast"),
            None,
            None,
        )
        .unwrap();
        let conn = db.conn.lock().unwrap();
        let (subagent, haiku, codex_subagent): (Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT claude_subagent_model, claude_small_model, codex_subagent_model FROM gateways WHERE id = 'gw'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(subagent.as_deref(), Some("claude-sonnet"));
        assert_eq!(haiku.as_deref(), Some("claude-haiku"));
        assert_eq!(codex_subagent.as_deref(), Some("gpt-5.6-sol-fast"));
        drop(conn);

        db.update_gateway_config("gw", None, None, None, None, None, None, None, None)
            .unwrap();
        let conn = db.conn.lock().unwrap();
        let cleared: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT claude_subagent_model, claude_small_model FROM gateways WHERE id = 'gw'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(cleared, (None, None));

        conn.execute(
            "UPDATE active_config SET claude_subagent_model = 'gpt-5.6-sol-fast' WHERE id = 1",
            [],
        )
        .unwrap();
        let active_subagent: Option<String> = conn
            .query_row(
                "SELECT claude_subagent_model FROM active_config WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active_subagent.as_deref(), Some("gpt-5.6-sol-fast"));
    }

    /// `add_traffic_log` needs no gateway row — the list query left-joins, so a
    /// dangling gateway_id just yields a NULL name.
    fn log(db: &Database, path: &str, status: u16) {
        db.add_traffic_log("gw-1", path, status, 100, Some("boom"))
            .expect("insert");
    }

    #[test]
    fn muting_a_path_hides_only_that_path() {
        let db = Database::open_in_memory().unwrap();
        log(&db, "/api/hello", 404);
        log(&db, "/v1/messages", 500);

        db.suppress_path("/api/hello").unwrap();

        let visible = db.get_traffic_log(None, 100, false).unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].path, "/v1/messages");
        assert!(!visible[0].suppressed);
    }

    /// The toolbar toggle asks for muted rows so it can render them struck
    /// through. They must come back *flagged*, not silently mixed in with the
    /// real errors.
    #[test]
    fn muted_rows_come_back_flagged_when_asked_for() {
        let db = Database::open_in_memory().unwrap();
        log(&db, "/api/hello", 404);
        log(&db, "/v1/messages", 500);
        db.suppress_path("/api/hello").unwrap();

        let all = db.get_traffic_log(None, 100, true).unwrap();
        assert_eq!(all.len(), 2);
        let hello = all.iter().find(|e| e.path == "/api/hello").unwrap();
        let msgs = all.iter().find(|e| e.path == "/v1/messages").unwrap();
        assert!(hello.suppressed);
        assert!(!msgs.suppressed);
    }

    /// The whole point of the feature being reversible: past rows are still in
    /// `traffic_log`, so unmuting brings them straight back.
    #[test]
    fn unmuting_restores_the_existing_rows() {
        let db = Database::open_in_memory().unwrap();
        log(&db, "/api/hello", 404);
        db.suppress_path("/api/hello").unwrap();
        assert!(db.get_traffic_log(None, 100, false).unwrap().is_empty());

        db.unsuppress_path("/api/hello").unwrap();

        let visible = db.get_traffic_log(None, 100, false).unwrap();
        assert_eq!(visible.len(), 1);
        assert!(!visible[0].suppressed);
    }

    /// Clicking mute twice (two rows, same path) must not create a second entry
    /// or reset when it was first muted.
    #[test]
    fn muting_twice_is_idempotent() {
        let db = Database::open_in_memory().unwrap();
        db.suppress_path("/api/hello").unwrap();
        let first = db.list_suppressed_paths().unwrap();
        db.suppress_path("/api/hello").unwrap();
        let second = db.list_suppressed_paths().unwrap();

        assert_eq!(second.len(), 1);
        assert_eq!(first[0].created_at, second[0].created_at);
    }

    /// Why the filter lives in SQL rather than in the caller: a chatty muted
    /// path must not eat the row budget and push real errors out of view.
    #[test]
    fn the_limit_counts_only_visible_rows() {
        let db = Database::open_in_memory().unwrap();
        for _ in 0..20 {
            log(&db, "/api/hello", 404);
        }
        log(&db, "/v1/messages", 500);
        db.suppress_path("/api/hello").unwrap();

        let visible = db.get_traffic_log(None, 5, false).unwrap();
        assert_eq!(visible.len(), 1, "the real error must survive the limit");
        assert_eq!(visible[0].path, "/v1/messages");
    }

    #[test]
    fn muting_applies_across_gateways() {
        let db = Database::open_in_memory().unwrap();
        db.add_traffic_log("gw-1", "/api/hello", 404, 10, None)
            .unwrap();
        db.add_traffic_log("gw-2", "/api/hello", 404, 10, None)
            .unwrap();
        db.suppress_path("/api/hello").unwrap();

        assert!(db
            .get_traffic_log(Some("gw-1"), 100, false)
            .unwrap()
            .is_empty());
        assert!(db
            .get_traffic_log(Some("gw-2"), 100, false)
            .unwrap()
            .is_empty());
    }
}
