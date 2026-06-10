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
    SetAutoFailover { enabled: bool },
    Reorder { ids: Vec<Uuid> },
    GetUsage { range: TimeRange, gateway_id: Option<Uuid> },
    GetTrafficLog { gateway_id: Option<Uuid> },
    StartLogin { gateway_id: Uuid },
    CancelLogin { gateway_id: Uuid },
    FetchKeys { gateway_id: Uuid },
    FetchModels { gateway_id: Uuid, key_id: Uuid },
    GetSettings,
    UpdateSettings(SettingsUpdate),
    Shutdown,
    ListGateways,
    /// TUI: fetch per-gateway usage rows for a given range.
    GetUsageRows { range: UsageRange },
    /// TUI: fetch recent error rows.
    GetErrors { limit: u32 },
    /// TUI: fetch agent/system settings summary.
    GetTuiSettings,
    /// TUI: toggle the auto-launch-on-boot preference.
    SetAutoLaunch { enabled: bool },
    /// TUI: add a gateway with just name+url (auth_key defaults to empty).
    AddGatewaySimple { name: String, url: String },
    /// TUI: update a gateway's name and url.
    UpdateGatewaySimple { id: Uuid, name: String, url: String },
    /// TUI: get the active key_id + model preferences for a gateway.
    GetGatewayConfig { gateway_id: Uuid },
    /// TUI: save key + model config for a gateway without activating it.
    SaveGatewayConfig { gateway_id: Uuid, key_id: Uuid, models: ModelSelection },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Pong,
    Snapshot(Snapshot),
    Ok,
    Error { message: String },
    LoginInitiated {
        gateway_id: Uuid,
        user_code: String,
        verification_uri: String,
        expires_in_secs: u64,
    },
    LoginCancelled { gateway_id: Uuid },
    Keys { keys: Vec<KeyInfo> },
    Models { catalog: ModelCatalog },
    Settings(Settings),
    Usage(UsageReport),
    TrafficLog { entries: Vec<TrafficEntry> },
    GatewayList { gateways: Vec<GatewaySummary> },
    /// TUI: per-gateway usage rows (see `UsageRowDetail`).
    UsageRows { rows: Vec<UsageRowDetail> },
    /// TUI: recent error rows.
    ErrorRows { rows: Vec<ErrorRow> },
    /// TUI: agent/system settings snapshot.
    TuiSettings(TuiSettings),
    /// TUI: acknowledgement for a settings mutation (e.g. SetAutoLaunch).
    SettingsAck,
    /// TUI: gateway created, returns new id.
    GatewayCreated { id: Uuid },
    /// TUI: gateway updated.
    GatewayUpdated { id: Uuid },
    /// TUI: active key + model config for a gateway.
    GatewayConfig {
        active_key_id: Option<Uuid>,
        claude: Option<String>,
        claude_small: Option<String>,
        codex: Option<String>,
        gemini: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    HealthChanged { gateway_id: Uuid, status: HealthStatus },
    ActiveChanged { gateway_id: Option<Uuid> },
    TrafficError(TrafficEntry),
    UsageDelta { gateway_id: Uuid, model: String, input: u64, output: u64, cache: u64 },
    LoginCompleted { gateway_id: Uuid, session_token: String, user_id: Option<String>, user_name: Option<String> },
    LoginFailed { gateway_id: Uuid, message: String },
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
pub enum KeystoreKind { System, EncryptedFile, Env }

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

/// Range selector used by the TUI Usage tab (distinct from `TimeRange` used by GUI).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageRange { Today, Last7Days, Last30Days, AllTime }

impl Default for UsageRange {
    fn default() -> Self { UsageRange::Today }
}

/// Per-gateway/model usage row returned for the TUI Usage tab.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRowDetail {
    pub gateway_id: uuid::Uuid,
    pub gateway_name: String,
    pub model: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
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
    /// The actual API key value, needed for proxy forwarding.
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewaySummary {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub starred: bool,
    pub healthy: Option<bool>,
    pub latency_ms: Option<i64>,
    #[serde(default)]
    pub needs_login: bool,
    #[serde(default)]
    pub active_key_name: Option<String>,
    #[serde(default)]
    pub claude_model: Option<String>,
    #[serde(default)]
    pub claude_small_model: Option<String>,
    #[serde(default)]
    pub codex_model: Option<String>,
    #[serde(default)]
    pub gemini_model: Option<String>,
    #[serde(default)]
    pub user_name: Option<String>,
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

/// A recent error entry shown in the TUI Errors tab.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorRow {
    /// ISO-8601 timestamp string.
    pub timestamp_iso: String,
    pub gateway_name: String,
    /// Error kind: "health" | "proxy" | "auth"
    pub kind: String,
    pub message: String,
}

/// Agent/system settings snapshot shown in the TUI Settings tab.
/// Separate from `Settings` (used by the GUI) to avoid collisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiSettings {
    /// "system" or "encrypted-file"
    pub keystore_kind: String,
    pub agent_pid: u32,
    pub socket_path: String,
    pub proxy_port: u16,
    pub log_path: String,
    pub auto_launch: bool,
    pub auto_failover: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WslDistroInfo {
    pub name: String,
    pub is_default: bool,
    pub selected: bool,
    pub home: Option<String>,
    pub has_claude: bool,
    pub has_codex: bool,
    pub has_gemini: bool,
    pub resolved_url: Option<String>,
    pub status: WslDistroStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WslDistroStatus {
    /// Has resolved URL, ready to apply.
    Ready,
    /// Probe ran but no candidate URL succeeded.
    Unreachable,
    /// Not yet probed (e.g., just discovered, before first state-machine tick).
    Unknown,
}
