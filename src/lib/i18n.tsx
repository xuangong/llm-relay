import { createContext, useContext, useState, useCallback, type ReactNode } from "react";

// ── Types ────────────────────────────────────────────────────────────────────

export type Lang = "en" | "zh";

type Translations = Record<string, Record<string, string>>;

// ── English translations ─────────────────────────────────────────────────────

const en: Translations = {
  common: {
    loading: "Loading...",
    cancel: "Cancel",
    done: "Done",
    remove: "Remove",
    delete: "Delete",
    back: "Back",
    signIn: "Sign In",
    save: "Save",
    error: "Error",
    errors: "Errors",
    use: "Use",
    next: "Next",
    retry: "Retry",
  },  header: {
    title: "LLM Relay",
    autoFailover: "Auto Failover",
    launchAtLogin: "Launch at Login",
    howToUse: "How to Use",
    online: "{healthy}/{total} online",
    disableRelay: "Disable Relay",
    renameClient: "Click to rename this client",
    autoFailoverOff: "Failover off",
    autoFailoverOffHint: "Auto failover is off — a failing gateway will stay selected. Click to change.",
  },
  settings: {
    title: "Settings",
    autoFailoverHint: "Switch to a healthy gateway automatically when the active one starts failing.",
    deviceName: "Device Name",
    deviceNamePlaceholder: "e.g. work-laptop",
    deviceNameHint: "Shown on the gateway dashboard to tell your machines apart.",
    language: "Language",
    managedClients: "Client configurations",
    managedClientsHint: "Choose which CLI configuration files LLM Relay manages.",
    clientClaude: "Claude Code",
    clientCodex: "Codex",
    clientGemini: "Gemini",
    clientsAtLeastOne: "Select at least one client configuration.",
    disableHint: "Stops routing CLI traffic through the relay and restores the config you had before enabling it.",
  },
  wsl: {
    title: "WSL2 Distros",
    refresh: "Refresh",
    refreshing: "Refreshing…",
    noneDetected: "No WSL2 distros detected.",
    installHint: "Install one via Microsoft Store or {cmd}, then Refresh.",
    default: "(default)",
    homeUnknown: "(home unknown)",
    unreachable: "Unreachable — ensure curl or wget is installed in this distro, then Refresh.",
    notProbed: "Not yet probed.",
  },
  disable: {
    title: "Disable LLM Relay",
    intro: "This will stop routing your CLI traffic through the relay and restore the original config you had before enabling it.",
    nothingToRestore: "No Relay origin files are available to restore.",
    confirm: "Disable",
    cancel: "Cancel",
    success: "Relay disabled. Original CLI config restored.",
    successCleared: "Relay disabled. Relay-written settings cleared.",
    failed: "Failed to disable: {error}",
    present: "present",
    absent: "absent",
  },
  addDialog: {
    title: "Add Gateway",
    urlStep: "Enter the gateway URL to sign in.",
    deviceStep: "Enter the code on the gateway website to authorize this app.",
    keysStep: "Select the API key to use for this gateway.",
    gatewayUrl: "Gateway URL",
    urlPlaceholder: "https://gateway.example.com",
    codeCopied: "Code copied to clipboard. Enter it on the gateway website.",
    waitingAuth: "Waiting for authorization...",
    openGateway: "Open Gateway",
    loadingKeys: "Loading keys...",
    noKeys: "No API keys found.",
    noKeysHint: "Create an API key on the gateway dashboard first.",
    selectKey: "Select an API Key",
    signedInAs: "Signed in as",
    addGateway: "Add Gateway",
  },
  editDialog: {
    title: "Edit Gateway",
    description: "Sign in to change your API key or gateway settings.",
    selectKey: "Select API Key",
    updateGateway: "Update Gateway",
    modelsStep: "Select the models to use with this gateway.",
  },  gateway: {
    inUse: "in use",
    offline: "offline",
    reapply: "Re-apply",
    authToken: "Auth Token",
    apiKey: "API Key",
    noKeys: "No keys available.",
    models: "Models",
    healthMonitor: "Health Monitor",
    trafficMonitor: "Traffic Monitor",
    uptimeStats: "{pct}% uptime · avg {ms}ms · {n} checks",
    down: "DOWN",
    now: "now",
    reqs: "{n} reqs",
    errors: "{n} errors",
    rateLimited: "Rate Limited",
    ok: "OK",
    noModels: "No models available",
    signedInAs: "Signed in as",
    gatewayAuthToken: "Gateway auth token",
    signInToEdit: "Sign in to edit",
    checking: "checking...",
    currentKey: "Current Key",
    pinToTop: "Pin to top",
    renameHint: "Double-click to rename",
    openInBrowser: "Open in browser",
  },
  usage: {
    title: "Token Usage",
    today: "Today",
    thisWeek: "This Week",
    sevenDays: "7 Days",
    thirtyDays: "30 Days",
    allGateways: "All gateways",
    noData: "No usage data for this period",
    models: "{n} models",
    input: "in",
    output: "out",
    cache: "cache",
    total: "total",
    req: "req",
    tokens: "tokens",
  },
  traffic: {
    title: "Anomalous Traffic",
    last24h: "(last 24h)",
    allGateways: "All gateways",
    entries: "{n} entries",
    noTraffic: "No anomalous traffic in the last 24h",
    time: "Time",
    status: "Status",
    latency: "Latency",
    gateway: "Gateway",
    path: "Path",
    detail: "Detail",
    errorDetail: "Error Detail:",
    suppress: "Mute errors for this path",
    unsuppress: "Unmute this path",
    suppressed: "Muted",
    showSuppressed: "{n} muted — click to show",
    hideSuppressed: "{n} muted — click to hide",
    today: "Today",
    yesterday: "Yesterday",
  },
  models: {
    search: "Search models…",
    noMatches: "No matching models",
    claude: "Claude",
    claudeSubagent: "Claude Subagent",
    claudeHaiku: "Claude Haiku",
    codex: "Codex",
    codexSubagent: "Codex Subagent",
    gemini: "Gemini",
    claudeRegion: "Claude Code",
    codexRegion: "Codex",
    geminiRegion: "Gemini",
  },
  extra: {
    none: "No Extra config",
    manage: "Manage",
    title: "Claude Extra configs",
    description: "Create reusable environment-variable sets. Relay-owned routing and authentication keys are reserved.",
    new: "New config",
    name: "Name",
    key: "Environment key",
    value: "Value",
    addEntry: "Add entry",
    duplicateKey: "Environment keys must be unique",
    invalid: "Enter a name and at least one environment entry",
    saved: "Extra config saved",
  },
};

// ── Chinese translations ─────────────────────────────────────────────────────

const zh: Translations = {
  common: {
    loading: "加载中...",
    cancel: "取消",
    done: "完成",
    remove: "移除",
    back: "返回",
    signIn: "登录",
    save: "保存",
    error: "错误",
    errors: "错误",
    use: "使用",
    next: "下一步",
    retry: "重试",
  },  header: {
    title: "LLM Relay",
    autoFailover: "自动故障转移",
    launchAtLogin: "开机启动",
    howToUse: "使用指南",
    online: "{healthy}/{total} 在线",
    disableRelay: "停用中继",
    renameClient: "点击重命名此客户端",
    autoFailoverOff: "未自动切换",
    autoFailoverOffHint: "自动故障转移已关闭——网关故障后不会自动切换。点击修改。",
  },
  settings: {
    title: "设置",
    autoFailoverHint: "当前网关开始故障时，自动切换到健康的网关。",
    deviceName: "设备名称",
    deviceNamePlaceholder: "例如 work-laptop",
    deviceNameHint: "显示在网关控制台，用于区分你的多台机器。",
    language: "语言",
    managedClients: "客户端配置",
    managedClientsHint: "选择 LLM Relay 要管理哪些 CLI 配置文件。",
    clientClaude: "Claude Code",
    clientCodex: "Codex",
    clientGemini: "Gemini",
    clientsAtLeastOne: "请至少选择一个客户端配置。",
    disableHint: "停止把 CLI 流量转发到中继，并恢复启用前的原始配置。",
  },
  wsl: {
    title: "WSL2 发行版",
    refresh: "刷新",
    refreshing: "刷新中…",
    noneDetected: "未检测到 WSL2 发行版。",
    installHint: "可通过 Microsoft Store 或 {cmd} 安装，然后点刷新。",
    default: "（默认）",
    homeUnknown: "（home 未知）",
    unreachable: "无法连通——请确认该发行版内安装了 curl 或 wget，然后点刷新。",
    notProbed: "尚未探测。",
  },
  disable: {
    title: "停用 LLM Relay",
    intro: "将停止把 CLI 流量转发到中继，并恢复启用前的原始配置。",
    nothingToRestore: "没有可用于恢复的 Relay origin 文件。",
    confirm: "停用",
    cancel: "取消",
    success: "已停用中继，原始 CLI 配置已恢复。",
    successCleared: "已停用中继，清除中继写入的字段。",
    failed: "停用失败：{error}",
    present: "存在",
    absent: "不存在",
  },
  addDialog: {
    title: "添加网关",
    urlStep: "输入网关地址以登录。",
    deviceStep: "在网关网站上输入授权码来授权此应用。",
    keysStep: "选择此网关要使用的 API 密钥。",
    gatewayUrl: "网关地址",
    urlPlaceholder: "https://gateway.example.com",
    codeCopied: "授权码已复制到剪贴板，请在网关网站上输入。",
    waitingAuth: "等待授权中...",
    openGateway: "打开网关",
    loadingKeys: "加载密钥中...",
    noKeys: "未找到 API 密钥。",
    noKeysHint: "请先在网关控制台创建一个 API 密钥。",
    selectKey: "选择 API 密钥",
    signedInAs: "已登录为",
    addGateway: "添加网关",
  },
  editDialog: {
    title: "编辑网关",
    description: "登录以更改 API 密钥或网关设置。",
    selectKey: "选择 API 密钥",
    updateGateway: "更新网关",
    modelsStep: "选择此网关要使用的模型。",
  },  gateway: {
    inUse: "使用中",
    offline: "离线",
    reapply: "重新应用",
    authToken: "授权令牌",
    apiKey: "API 密钥",
    noKeys: "无可用密钥。",
    models: "模型",
    healthMonitor: "健康监控",
    trafficMonitor: "流量监控",
    uptimeStats: "{pct}% 可用 · 均值 {ms}ms · {n} 次检查",
    down: "宕机",
    now: "现在",
    reqs: "{n} 请求",
    errors: "{n} 错误",
    rateLimited: "频率限制",
    ok: "正常",
    noModels: "无可选模型",
    signedInAs: "已登录为",
    gatewayAuthToken: "网关授权令牌",
    signInToEdit: "登录以编辑",
    checking: "检查中...",
    currentKey: "当前密钥",
    pinToTop: "置顶",
    renameHint: "双击重命名",
    openInBrowser: "在浏览器中打开",
  },
  usage: {
    title: "用量统计",
    today: "今天",
    thisWeek: "本周",
    sevenDays: "7 天",
    thirtyDays: "30 天",
    allGateways: "全部网关",
    noData: "该时段暂无用量数据",
    models: "{n} 个模型",
    input: "输入",
    output: "输出",
    cache: "缓存",
    total: "总计",
    req: "次",
    tokens: "令牌",
  },
  traffic: {
    title: "异常流量",
    last24h: "（近 24 小时）",
    allGateways: "全部网关",
    entries: "{n} 条记录",
    noTraffic: "近 24 小时内无异常流量",
    time: "时间",
    status: "状态",
    latency: "延迟",
    gateway: "网关",
    path: "路径",
    detail: "详情",
    errorDetail: "错误详情：",
    suppress: "屏蔽此路径的错误",
    unsuppress: "取消屏蔽此路径",
    suppressed: "已屏蔽",
    showSuppressed: "已屏蔽 {n} 条 — 点击显示",
    hideSuppressed: "已屏蔽 {n} 条 — 点击隐藏",
    today: "今天",
    yesterday: "昨天",
  },
  models: {
    search: "搜索模型…",
    noMatches: "没有匹配的模型",
    claude: "Claude 主模型",
    claudeSubagent: "Claude 子代理",
    claudeHaiku: "Claude Haiku",
    codex: "Codex",
    codexSubagent: "Codex 子代理",
    gemini: "Gemini",
    claudeRegion: "Claude Code",
    codexRegion: "Codex",
    geminiRegion: "Gemini",
  },
  extra: {
    none: "无 Extra 配置",
    manage: "管理",
    title: "Claude Extra 配置",
    description: "创建可复用的环境变量方案。Relay 管理的路由和认证键不可覆盖。",
    new: "新增配置",
    name: "名称",
    key: "环境变量名",
    value: "值",
    addEntry: "增加配置项",
    duplicateKey: "环境变量名不能重复",
    invalid: "请输入名称和至少一个配置项",
    saved: "Extra 配置已保存",
  },
};

// ── Dictionary lookup ────────────────────────────────────────────────────────

const dicts: Record<Lang, Translations> = { en, zh };

function lookup(lang: Lang, key: string): string {
  const parts = key.split(".");
  if (parts.length !== 2) return key;
  const [ns, k] = parts;
  return dicts[lang]?.[ns]?.[k] ?? dicts.en?.[ns]?.[k] ?? key;
}

// ── Context & Provider ───────────────────────────────────────────────────────

interface I18nContextValue {
  t: (key: string, vars?: Record<string, string | number>) => string;
  lang: Lang;
  toggleLang: () => void;
}

const I18nContext = createContext<I18nContextValue | null>(null);

function detectLang(): Lang {
  const stored = localStorage.getItem("lang");
  if (stored === "en" || stored === "zh") return stored;
  return navigator.language.startsWith("zh") ? "zh" : "en";
}

export function I18nProvider({ children }: { children: ReactNode }) {
  const [lang, setLang] = useState<Lang>(detectLang);

  const toggleLang = useCallback(() => {
    setLang((prev) => {
      const next = prev === "en" ? "zh" : "en";
      localStorage.setItem("lang", next);
      return next;
    });
  }, []);

  const t = useCallback(
    (key: string, vars?: Record<string, string | number>): string => {
      let result = lookup(lang, key);
      if (vars) {
        for (const [k, v] of Object.entries(vars)) {
          result = result.replace(`{${k}}`, String(v));
        }
      }
      return result;
    },
    [lang]
  );

  return (
    <I18nContext.Provider value={{ t, lang, toggleLang }}>
      {children}
    </I18nContext.Provider>
  );
}

// ── Hook ─────────────────────────────────────────────────────────────────────

export function useI18n(): I18nContextValue {
  const ctx = useContext(I18nContext);
  if (!ctx) throw new Error("useI18n must be used within I18nProvider");
  return ctx;
}
