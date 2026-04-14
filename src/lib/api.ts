import { invoke } from "@tauri-apps/api/core";

// Types matching Rust structs

export interface Gateway {
  id: string;
  name: string;
  url: string;
  authKey: string;
  isAdmin: boolean;
  sessionToken: string | null;
  sortOrder: number;
  createdAt: string;
}

export interface GatewayWithHealth {
  id: string;
  name: string;
  url: string;
  authKey: string;
  isAdmin: boolean;
  sessionToken: string | null;
  sortOrder: number;
  createdAt: string;
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

export const addGateway = (name: string, url: string, authKey: string) =>
  invoke<Gateway>("add_gateway", { name, url, authKey });

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
