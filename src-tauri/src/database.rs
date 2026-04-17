use crate::error::AppError;
use crate::keystore;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

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
    pub claude_small_model: Option<String>,
    pub codex_model: Option<String>,
    pub gemini_model: Option<String>,
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
    pub claude_small_model: Option<String>,
    pub codex_model: Option<String>,
    pub gemini_model: Option<String>,
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

impl Database {
    pub fn init(config_dir: &Path) -> Result<Self, AppError> {
        let db_path = config_dir.join("config.db");
        let conn = Connection::open(db_path)?;

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
            conn.execute_batch(
                "ALTER TABLE active_config ADD COLUMN last_switched_at TEXT;",
            )?;
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
                    .query_row("SELECT key_value FROM active_config WHERE id = 1", [], |row| row.get(0))
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

            let active: Option<(Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)> = conn
                .query_row(
                    "SELECT gateway_id, claude_model, claude_small_model, codex_model, gemini_model
                     FROM active_config WHERE id = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
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

        Ok(Database {
            conn: Mutex::new(conn),
        })
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
                                   claude_model, claude_small_model, codex_model, gemini_model)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
                gw.claude_small_model,
                gw.codex_model,
                gw.gemini_model,
            ],
        )?;
        Ok(())
    }

    pub fn list_gateways(&self) -> Result<Vec<Gateway>, AppError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, url, auth_key, is_admin, session_token, user_id, user_name, sort_order, created_at,
                    claude_model, claude_small_model, codex_model, gemini_model
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
                claude_small_model: row.get(11)?,
                codex_model: row.get(12)?,
                gemini_model: row.get(13)?,
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
                    claude_model, claude_small_model, codex_model, gemini_model
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
                claude_small_model: row.get(11)?,
                codex_model: row.get(12)?,
                gemini_model: row.get(13)?,
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
        conn.execute("DELETE FROM health_cache WHERE gateway_id = ?1", params![id])?;
        conn.execute("DELETE FROM health_check_log WHERE gateway_id = ?1", params![id])?;
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

    /// Persist per-gateway model preferences. Only overwrites fields that are
    /// provided; passing None for a field leaves the stored value untouched.
    pub fn update_gateway_models(
        &self,
        id: &str,
        claude: Option<&str>,
        claude_small: Option<&str>,
        codex: Option<&str>,
        gemini: Option<&str>,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE gateways SET
                claude_model       = COALESCE(?1, claude_model),
                claude_small_model = COALESCE(?2, claude_small_model),
                codex_model        = COALESCE(?3, codex_model),
                gemini_model       = COALESCE(?4, gemini_model)
             WHERE id = ?5",
            params![claude, claude_small, codex, gemini, id],
        )?;
        Ok(())
    }

    // ─── Active Config ───

    pub fn get_active_config(&self) -> Result<ActiveConfig, AppError> {
        let conn = self.conn.lock().unwrap();
        let mut result = conn.query_row(
            "SELECT gateway_id, key_id, key_name, key_value,
                    claude_model, claude_small_model, codex_model, gemini_model,
                    auto_switch, applied_at, last_switched_at
             FROM active_config WHERE id = 1",
            [],
            |row| {
                Ok(ActiveConfig {
                    gateway_id: row.get(0)?,
                    key_id: row.get(1)?,
                    key_name: row.get(2)?,
                    key_value: row.get(3)?,
                    claude_model: row.get(4)?,
                    claude_small_model: row.get(5)?,
                    codex_model: row.get(6)?,
                    gemini_model: row.get(7)?,
                    auto_switch: row.get::<_, i32>(8)? != 0,
                    applied_at: row.get(9)?,
                    last_switched_at: row.get(10)?,
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
        // Store key_value in keychain
        if let Some(ref kv) = config.key_value {
            keystore::set_secret(&keystore::active_key_value(), kv);
        } else {
            keystore::delete_secret(&keystore::active_key_value());
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE active_config SET
                gateway_id = ?1, key_id = ?2, key_name = ?3, key_value = ?4,
                claude_model = ?5, claude_small_model = ?6, codex_model = ?7, gemini_model = ?8,
                auto_switch = ?9, applied_at = ?10, last_switched_at = ?11
             WHERE id = 1",
            params![
                config.gateway_id,
                config.key_id,
                config.key_name,
                Option::<String>::None,  // empty — real value in keychain
                config.claude_model,
                config.claude_small_model,
                config.codex_model,
                config.gemini_model,
                config.auto_switch as i32,
                config.applied_at,
                config.last_switched_at,
            ],
        )?;
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

    pub fn add_health_log(&self, gateway_id: &str, is_healthy: bool, latency_ms: Option<i64>) -> Result<(), AppError> {
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

    pub fn get_health_log(&self, gateway_id: &str, limit: usize) -> Result<Vec<HealthLogEntry>, AppError> {
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
    pub fn get_traffic_log(
        &self,
        gateway_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<TrafficLogEntry>, AppError> {
        let conn = self.conn.lock().unwrap();
        let entries = if let Some(gid) = gateway_id {
            let mut stmt = conn.prepare(
                "SELECT t.id, t.gateway_id, g.name, t.path, t.status, t.latency_ms, t.error_detail, t.logged_at
                 FROM traffic_log t LEFT JOIN gateways g ON g.id = t.gateway_id
                 WHERE t.gateway_id = ?1
                 ORDER BY t.logged_at DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![gid, limit as i64], |row| {
                Ok(TrafficLogEntry {
                    id: row.get(0)?,
                    gateway_id: row.get(1)?,
                    gateway_name: row.get(2)?,
                    path: row.get(3)?,
                    status: row.get::<_, i32>(4)? as u16,
                    latency_ms: row.get::<_, i64>(5)? as u64,
                    error_detail: row.get(6)?,
                    logged_at: row.get(7)?,
                })
            })?;
            rows.filter_map(|r| r.ok()).collect()
        } else {
            let mut stmt = conn.prepare(
                "SELECT t.id, t.gateway_id, g.name, t.path, t.status, t.latency_ms, t.error_detail, t.logged_at
                 FROM traffic_log t LEFT JOIN gateways g ON g.id = t.gateway_id
                 ORDER BY t.logged_at DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit as i64], |row| {
                Ok(TrafficLogEntry {
                    id: row.get(0)?,
                    gateway_id: row.get(1)?,
                    gateway_name: row.get(2)?,
                    path: row.get(3)?,
                    status: row.get::<_, i32>(4)? as u16,
                    latency_ms: row.get::<_, i64>(5)? as u64,
                    error_detail: row.get(6)?,
                    logged_at: row.get(7)?,
                })
            })?;
            rows.filter_map(|r| r.ok()).collect()
        };
        Ok(entries)
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
            params![gateway_id, model, hour, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens],
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
            "today"  => "strftime('%Y-%m-%dT%H', 'now', 'start of day')".to_string(),
            "week"   => "strftime('%Y-%m-%dT%H', date('now', 'weekday 1', '-7 days'))".to_string(),
            "7d"     => "strftime('%Y-%m-%dT%H', datetime('now', '-7 days'))".to_string(),
            "30d"    => "strftime('%Y-%m-%dT%H', datetime('now', '-30 days'))".to_string(),
            _        => "strftime('%Y-%m-%dT%H', 'now', 'start of day')".to_string(),
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
            gw_filter = if gateway_id.is_some() { " AND gateway_id = ?1" } else { "" },
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
}
