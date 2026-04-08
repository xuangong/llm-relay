use crate::error::AppError;
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
    pub sort_order: i32,
    pub created_at: String,
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

impl Database {
    pub fn init(config_dir: &Path) -> Result<Self, AppError> {
        let db_path = config_dir.join("config.db");
        let conn = Connection::open(db_path)?;

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

        Ok(Database {
            conn: Mutex::new(conn),
        })
    }

    // ─── Gateway CRUD ───

    pub fn add_gateway(&self, gw: &Gateway) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO gateways (id, name, url, auth_key, is_admin, session_token, sort_order, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                gw.id,
                gw.name,
                gw.url,
                gw.auth_key,
                gw.is_admin as i32,
                gw.session_token,
                gw.sort_order,
                gw.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn list_gateways(&self) -> Result<Vec<Gateway>, AppError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, url, auth_key, is_admin, session_token, sort_order, created_at
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
                sort_order: row.get(6)?,
                created_at: row.get(7)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn get_gateway(&self, id: &str) -> Result<Option<Gateway>, AppError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, url, auth_key, is_admin, session_token, sort_order, created_at
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
                sort_order: row.get(6)?,
                created_at: row.get(7)?,
            })
        });
        match result {
            Ok(gw) => Ok(Some(gw)),
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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE gateways SET name = ?1, url = ?2, auth_key = ?3 WHERE id = ?4",
            params![name, url, auth_key, id],
        )?;
        Ok(())
    }

    pub fn update_gateway_session(
        &self,
        id: &str,
        is_admin: bool,
        session_token: Option<&str>,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE gateways SET is_admin = ?1, session_token = ?2 WHERE id = ?3",
            params![is_admin as i32, session_token, id],
        )?;
        Ok(())
    }

    pub fn delete_gateway(&self, id: &str) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM health_cache WHERE gateway_id = ?1", params![id])?;
        conn.execute("DELETE FROM gateways WHERE id = ?1", params![id])?;
        // Clear active config if it was using this gateway
        conn.execute(
            "UPDATE active_config SET gateway_id = NULL, key_id = NULL, key_name = NULL, key_value = NULL
             WHERE gateway_id = ?1",
            params![id],
        )?;
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

    // ─── Active Config ───

    pub fn get_active_config(&self) -> Result<ActiveConfig, AppError> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT gateway_id, key_id, key_name, key_value,
                    claude_model, claude_small_model, codex_model, gemini_model,
                    auto_switch, applied_at
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
                })
            },
        )?;
        Ok(result)
    }

    pub fn set_active_config(&self, config: &ActiveConfig) -> Result<(), AppError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE active_config SET
                gateway_id = ?1, key_id = ?2, key_name = ?3, key_value = ?4,
                claude_model = ?5, claude_small_model = ?6, codex_model = ?7, gemini_model = ?8,
                auto_switch = ?9, applied_at = ?10
             WHERE id = 1",
            params![
                config.gateway_id,
                config.key_id,
                config.key_name,
                config.key_value,
                config.claude_model,
                config.claude_small_model,
                config.codex_model,
                config.gemini_model,
                config.auto_switch as i32,
                config.applied_at,
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
}
