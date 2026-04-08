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

export const fetchModels = (gatewayId: string) =>
  invoke<ModelList>("fetch_models", { gatewayId });

// ─── Health ───

export const checkAllHealth = () =>
  invoke<GatewayWithHealth[]>("check_all_health");

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

// ─── Tray ───

export const updateTrayMenu = () =>
  invoke<void>("update_tray_menu");
