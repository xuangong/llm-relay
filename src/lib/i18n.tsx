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
    back: "Back",
    signIn: "Sign In",
    save: "Save",
    error: "Error",
    errors: "Errors",
    use: "Use",
  },
  header: {
    title: "LLM Relay",
    autoFailover: "Auto Failover",
    launchAtLogin: "Launch at Login",
    howToUse: "How to Use",
    online: "{healthy}/{total} online",
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
  gateway: {
    inUse: "in use",
    configDrift: "Config modified externally",
    reApply: "Re-apply",
    offline: "offline",
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
    today: "Today",
    yesterday: "Yesterday",
  },
  models: {
    claude: "Claude",
    claudeSmall: "Claude Small",
    codex: "Codex",
    gemini: "Gemini",
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
  },
  header: {
    title: "LLM Relay",
    autoFailover: "自动故障转移",
    launchAtLogin: "开机启动",
    howToUse: "使用指南",
    online: "{healthy}/{total} 在线",
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
  gateway: {
    inUse: "使用中",
    configDrift: "配置已被外部修改",
    reApply: "重新应用",
    offline: "离线",
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
    today: "今天",
    yesterday: "昨天",
  },
  models: {
    claude: "Claude",
    claudeSmall: "Claude Small",
    codex: "Codex",
    gemini: "Gemini",
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
