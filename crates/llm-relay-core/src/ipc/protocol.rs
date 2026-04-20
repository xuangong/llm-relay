//! IPC protocol between agent (server) and TUI/clients.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientFrame {
    pub request_id: u64,
    pub payload: Request,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerFrame {
    Response { request_id: u64, payload: Response },
    Event(Event),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Ping,
    GetSnapshot,
    Subscribe { topics: Vec<Topic> },
    Unsubscribe { topics: Vec<Topic> },
    AddGateway(GatewayInput),
    UpdateGateway { id: Uuid, fields: GatewayUpdate },
    DeleteGateway { id: Uuid },
    SetActive { gateway_id: Uuid, key_id: Uuid, models: ModelSelection },
    ClearActive,
    SetAutoFailover(bool),
    Reorder(Vec<Uuid>),
    GetUsage { range: TimeRange, gateway_id: Option<Uuid> },
    GetTrafficLog { gateway_id: Option<Uuid> },
    StartLogin { gateway_id: Uuid },
    CancelLogin { gateway_id: Uuid },
    FetchKeys { gateway_id: Uuid },
    FetchModels { gateway_id: Uuid, key_id: Uuid },
    GetSettings,
    UpdateSettings(SettingsUpdate),
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Pong,
    Snapshot(Snapshot),
    Ok,
    Error { message: String },
    LoginInitiated {
        device_code: String,
        user_code: String,
        verification_url: String,
        expires_at: DateTime<Utc>,
        interval_secs: u64,
    },
    Keys(Vec<KeyInfo>),
    Models(ModelCatalog),
    Settings(Settings),
    Usage(UsageReport),
    TrafficLog(Vec<TrafficEntry>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    HealthChanged { gateway_id: Uuid, status: HealthStatus },
    ActiveChanged { gateway_id: Option<Uuid> },
    TrafficError(TrafficEntry),
    UsageDelta { gateway_id: Uuid, model: String, input: u64, output: u64, cache: u64 },
    LoginCompleted { gateway_id: Uuid, user_name: Option<String> },
    LoginFailed { gateway_id: Uuid, reason: String },
    LoginExpired { gateway_id: Uuid },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Topic { Health, Active, Traffic, Usage, Login }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayInput {
    pub name: String,
    pub url: String,
    pub auth_key: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayUpdate {
    pub name: Option<String>,
    pub url: Option<String>,
    pub auth_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub gateways: Vec<GatewayView>,
    pub active: Option<ActiveView>,
    pub auto_failover: bool,
    pub agent_pid: u32,
    pub agent_started_at: DateTime<Utc>,
    pub proxy_port: u16,
    pub keystore_kind: KeystoreKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayView {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub sort_order: i32,
    pub health: Option<HealthStatus>,
    pub last_check_at: Option<DateTime<Utc>>,
    pub model_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveView {
    pub gateway_id: Uuid,
    pub key_id: Uuid,
    pub key_name: String,
    pub models: ModelSelection,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelSelection {
    pub claude: Option<String>,
    pub claude_small: Option<String>,
    pub codex: Option<String>,
    pub gemini: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCatalog {
    pub claude: Vec<String>,
    pub codex: Vec<String>,
    pub gemini: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus { Healthy, Degraded, Down, Unknown }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KeystoreKind { System, EncryptedFile }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimeRange { Today, Week, Days7, Days30 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageReport {
    pub range: TimeRange,
    pub rows: Vec<UsageRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRow {
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficEntry {
    pub at: DateTime<Utc>,
    pub gateway_id: Uuid,
    pub gateway_name: String,
    pub status: u16,
    pub path: String,
    pub latency_ms: u32,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyInfo {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    pub client_name: String,
    pub auto_failover: bool,
    pub launch_at_login: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SettingsUpdate {
    pub client_name: Option<String>,
    pub launch_at_login: Option<bool>,
}
