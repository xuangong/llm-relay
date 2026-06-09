import { invoke } from "@tauri-apps/api/core";

// Types matching Rust structs

export interface Gateway {
  id: string;
  name: string;
  url: string;
  authKey: string;
  isAdmin: boolean;
  sessionToken: string | null;
  userId: string | null;
  userName: string | null;
  sortOrder: number;
  createdAt: string;
  claudeModel: string | null;
  claudeSmallModel: string | null;
  codexModel: string | null;
  geminiModel: string | null;
}

export interface GatewayWithHealth {
  id: string;
  name: string;
  url: string;
  authKey: string;
  isAdmin: boolean;
  sessionToken: string | null;
  userId: string | null;
  userName: string | null;
  sortOrder: number;
  createdAt: string;
  claudeModel: string | null;
  claudeSmallModel: string | null;
  codexModel: string | null;
  geminiModel: string | null;
  isHealthy: boolean;
  latencyMs: number | null;
  modelCount: number | null;
  lastChecked: string | null;
}

export interface LoginResult {
  ok: boolean;
  isAdmin: boolean;
  isUser: boolean;
  userId?: string;
  userName?: string;
  sessionToken?: string;
  keyId?: string;
  keyName?: string;
}

export interface ApiKey {
  id: string;
  name: string;
  key: string;
  createdAt: string | null;
  lastUsedAt: string | null;
  ownerId: string | null;
  ownerName: string | null;
}

export interface ModelInfo {
  id: string;
  object: string;
  owned_by: string;
}

export interface ModelList {
  data: ModelInfo[];
}

export interface ActiveConfig {
  gatewayId: string | null;
  keyId: string | null;
  keyName: string | null;
  keyValue: string | null;
  claudeModel: string | null;
  claudeSmallModel: string | null;
  codexModel: string | null;
  geminiModel: string | null;
  autoSwitch: boolean;
  appliedAt: string | null;
}

export interface AppSettings {
  autoSwitch: boolean;
}

export interface CurrentCliConfig {
  claude: Record<string, unknown> | null;
  codexAuth: Record<string, unknown> | null;
  codexConfig: string | null;
  gemini: Record<string, string> | null;
}

export interface HealthLogEntry {
  isHealthy: boolean;
  latencyMs: number | null;
  checkedAt: string;
}

export interface TrafficLogEntry {
  id: number;
  gatewayId: string;
  gatewayName: string | null;
  path: string;
  status: number;
  latencyMs: number;
  errorDetail: string | null;
  loggedAt: string;
}

export interface UsageSummary {
  model: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  requests: number;
}

// ─── Gateway CRUD ───

export interface AddGatewayParams {
  name: string;
  url: string;
  authKey: string;
  sessionToken?: string;
  userId?: string;
  userName?: string;
}

export const addGateway = (params: AddGatewayParams) =>
  invoke<Gateway>("add_gateway", params as unknown as Record<string, unknown>);

export const listGateways = () =>
  invoke<GatewayWithHealth[]>("list_gateways");

export const updateGateway = (id: string, name: string, url: string, authKey: string) =>
  invoke<void>("update_gateway", { id, name, url, authKey });

export const deleteGateway = (id: string) =>
  invoke<void>("delete_gateway", { id });

export const reorderGateways = (ids: string[]) =>
  invoke<void>("reorder_gateways", { ids });

// ─── Gateway API ───

export const loginGateway = (url: string, key: string) =>
  invoke<LoginResult>("login_gateway", { url, key });

export const fetchKeys = (gatewayId: string) =>
  invoke<ApiKey[]>("fetch_keys", { gatewayId });

export const fetchModels = (gatewayId: string, keyValue?: string) =>
  invoke<ModelList>("fetch_models", { gatewayId, keyValue });

// ─── Health ───

export const checkAllHealth = () =>
  invoke<GatewayWithHealth[]>("check_all_health");

export const getHealthLog = (gatewayId: string) =>
  invoke<HealthLogEntry[]>("get_health_log", { gatewayId });

export const getTrafficLogs = (gatewayId?: string, limit?: number) =>
  invoke<TrafficLogEntry[]>("get_traffic_logs", { gatewayId, limit });

export type UsagePeriod = "today" | "week" | "7d" | "30d";

export const getUsageStats = (period: UsagePeriod, gatewayId?: string) =>
  invoke<UsageSummary[]>("get_usage_stats", { period, gatewayId });

// ─── Config ───

export interface ApplyConfigParams {
  gatewayId: string;
  keyId?: string;
  keyName?: string;
  keyValue?: string;
  claudeModel?: string;
  claudeSmallModel?: string;
  codexModel?: string;
  geminiModel?: string;
}

export const applyConfig = (params: ApplyConfigParams) =>
  invoke<void>("apply_config", params as unknown as Record<string, unknown>);

export const readCurrentConfig = () =>
  invoke<CurrentCliConfig>("read_current_config");

export const clearConfig = () =>
  invoke<void>("clear_config");

export interface ClaudeSnapshot {
  anthropicBaseUrl: string | null;
  anthropicModel: string | null;
  anthropicSmallFastModel: string | null;
  anthropicAuthToken: string | null;
}

export interface CodexSnapshot {
  openaiApiKey: string | null;
  model: string | null;
  modelProvider: string | null;
  copilotGatewayProviderToml: string | null;
}

export interface GeminiSnapshot {
  geminiApiKey: string | null;
  googleGeminiBaseUrl: string | null;
  geminiApiBaseUrl: string | null;
  selectedAuthType: string | null;
}

export interface CliConfigSnapshot {
  capturedAt: string;
  claude: ClaudeSnapshot;
  codex: CodexSnapshot;
  gemini: GeminiSnapshot;
}

export const getConfigSnapshot = () =>
  invoke<CliConfigSnapshot | null>("get_config_snapshot");

// ─── Settings ───

export const getActiveConfig = () =>
  invoke<ActiveConfig>("get_active_config_cmd");

export const getSettings = () =>
  invoke<AppSettings>("get_settings");

export const updateSettings = (autoSwitch: boolean) =>
  invoke<void>("update_settings", { autoSwitch });

// ─── Client Identity ───

export const getClientName = () =>
  invoke<string>("get_client_name");

export const setClientName = (name: string) =>
  invoke<void>("set_client_name", { name });

export const getAutostart = () =>
  invoke<boolean>("get_autostart");

export const setAutostart = (enabled: boolean) =>
  invoke<void>("set_autostart", { enabled });

// ─── Tray ───

export const updateTrayMenu = () =>
  invoke<void>("update_tray_menu");

export async function testHeartbeat(): Promise<string> {
  return invoke("test_heartbeat");
}

// ─── Device Authorization Flow ───

export interface DeviceCodeResponse {
  deviceCode: string;
  userCode: string;
  expiresIn: number;
  interval: number;
}

export interface DevicePollResponse {
  status: "pending" | "complete" | "expired";
  sessionToken?: string;
  userId?: string;
  userName?: string;
}

export const startDeviceLogin = (url: string) =>
  invoke<DeviceCodeResponse>("start_device_login", { url });

export const pollDeviceLogin = (url: string, deviceCode: string) =>
  invoke<DevicePollResponse>("poll_device_login", { url, deviceCode });

export const fetchKeysWithToken = (url: string, token: string) =>
  invoke<ApiKey[]>("fetch_keys_with_token", { url, token });

export const openUrl = (url: string) =>
  invoke<void>("open_url", { url });
