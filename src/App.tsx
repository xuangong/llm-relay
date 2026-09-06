import { useState, useEffect, useCallback, useRef } from "react";
import { Toaster, toast } from "sonner";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { getVersion } from "@tauri-apps/api/app";
import { GatewayList } from "@/components/GatewayList";
import { AddGatewayCard } from "@/components/AddGatewayCard";
import { TrafficLogPanel } from "@/components/TrafficLogPanel";
import { UsagePanel } from "@/components/UsagePanel";
import { DisableRelayDialog } from "@/components/DisableRelayDialog";
import { SettingsSheet } from "@/components/Settings/SettingsSheet";
import { Button } from "@/components/ui/button";
import * as api from "@/lib/api";
import type { GatewayWithHealth, ActiveConfig, ClaudeExtraConfig } from "@/lib/api";
import { extractErrorMessage } from "@/lib/error";
import { useI18n } from "@/lib/i18n";
import { RefreshCw, Loader2, AlertTriangle, ChevronDown, BarChart3, HelpCircle, Menu, ZapOff } from "lucide-react";

function App() {
  const { t } = useI18n();
  const [gateways, setGateways] = useState<GatewayWithHealth[]>([]);
  const [extraConfigs, setExtraConfigs] = useState<ClaudeExtraConfig[]>([]);
  const [activeConfig, setActiveConfig] = useState<ActiveConfig | null>(null);
  const [autoSwitch, setAutoSwitch] = useState(true);
  const [managedClients, setManagedClients] = useState<api.ManagedClients>({
    claude: false,
    codex: true,
    gemini: false,
  });
  const [autostart, setAutostart] = useState(false);
  const [loading, setLoading] = useState(true);
  const [checking, setChecking] = useState(false);
  const [showLogs, setShowLogs] = useState(false);
  const [logErrorCount, setLogErrorCount] = useState(0);
  const [bottomTab, setBottomTab] = useState<"usage" | "logs">("usage");
  const [appVersion, setAppVersion] = useState("");
  const [logFilterGateway, setLogFilterGateway] = useState<string>("all");
  // Muted paths, mirrored here so the error badge stays quiet for them too —
  // otherwise muting a noisy probe would still light up the Errors button. A
  // ref, not state: only the proxy-traffic listener reads it, and keeping it out
  // of that effect's deps avoids tearing down the listener on every mute.
  const suppressedPaths = useRef<string[]>([]);

  const loadSuppressed = useCallback(async () => {
    try {
      const muted = await api.listSuppressedPaths();
      suppressedPaths.current = muted.map((m) => m.path);
    } catch (e) {
      console.error(e);
    }
  }, []);

  useEffect(() => {
    loadSuppressed();
  }, [loadSuppressed]);
  const [clientName, setClientName] = useState("");
  const [disableOpen, setDisableOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);

  const loadGateways = useCallback(async () => {
    try {
      const result = await api.listGateways();
      setGateways(result);
    } catch (err) {
      console.error("Failed to load gateways:", err);
    }
  }, []);

  const loadSettings = useCallback(async () => {
    try {
      const [settings, autostartEnabled] = await Promise.all([
        api.getSettings(),
        api.getAutostart(),
      ]);
      setAutoSwitch(settings.autoSwitch);
      setManagedClients(settings.managedClients);
      setAutostart(autostartEnabled);
    } catch (err) {
      console.error("Failed to load settings:", err);
    }
  }, []);

  const loadClientName = useCallback(async () => {
    try {
      const name = await api.getClientName();
      setClientName(name);
    } catch (err) {
      console.error("Failed to load client name:", err);
    }
  }, []);

  const loadExtraConfigs = useCallback(async () => {
    try {
      setExtraConfigs(await api.listClaudeExtraConfigs());
    } catch (err) {
      console.error("Failed to load Claude Extra configs:", err);
    }
  }, []);

  const loadAll = useCallback(async () => {
    await Promise.all([loadGateways(), loadSettings(), loadClientName(), loadExtraConfigs()]);
    try {
      const config = await api.getActiveConfig();
      setActiveConfig(config);
    } catch {
      // no active config yet
    }
  }, [loadGateways, loadSettings, loadClientName, loadExtraConfigs]);

  useEffect(() => {
    const init = async () => {
      setLoading(true);
      await loadAll();
      setLoading(false);
    };
    init();
    getVersion().then(setAppVersion);
  }, [loadAll]);

  // Listen for backend events
  useEffect(() => {
    const appWindow = getCurrentWebviewWindow();

    const unlisten1 = appWindow.listen<GatewayWithHealth[]>("health-updated", (event) => {
      setGateways(event.payload);
    });

    const unlisten2 = appWindow.listen<{ gatewayId: string; gatewayName?: string }>("gateway-switched", (event) => {
      const { gatewayId, gatewayName } = event.payload;
      console.log("[App] gateway-switched event received:", gatewayId, gatewayName);
      // Immediately update the active gateway without waiting for loadAll
      setActiveConfig((prev) => prev ? { ...prev, gatewayId } : { gatewayId, keyId: null, keyName: null, keyValue: null, claudeModel: null, claudeSubagentModel: null, claudeSmallModel: null, codexModel: null, codexSubagentModel: null, geminiModel: null, claudeExtraConfigId: null, autoSwitch: true, appliedAt: null });
      toast.success(`Switched to ${gatewayName || "new gateway"}`);
      loadAll();
    });

    const unlisten3 = appWindow.listen<{ status: number; path?: string }>("proxy-traffic", (event) => {
      const { status, path } = event.payload;
      if (status >= 400 && !(path && suppressedPaths.current.includes(path))) {
        setLogErrorCount((n) => n + 1);
      }
    });

    return () => {
      unlisten1.then((fn) => fn());
      unlisten2.then((fn) => fn());
      unlisten3.then((fn) => fn());
    };
  }, [loadAll]);

  const handleAutoSwitchChange = async (checked: boolean) => {
    setAutoSwitch(checked);
    try {
      await api.updateSettings(checked);
    } catch (err) {
      console.error("Failed to update settings:", err);
      toast.error(`Failed to update settings: ${extractErrorMessage(err)}`);
      setAutoSwitch(!checked);
    }
  };

  const handleManagedClientsChange = async (next: api.ManagedClients) => {
    const previous = managedClients;
    setManagedClients(next);
    try {
      await api.updateSettings(autoSwitch, next);
      await loadAll();
    } catch (err) {
      toast.error(`Failed to update client mode: ${extractErrorMessage(err)}`);
      setManagedClients(previous);
    }
  };

  const handleAutostartChange = async (checked: boolean) => {
    setAutostart(checked);
    try {
      await api.setAutostart(checked);
    } catch (err) {
      console.error("Failed to update autostart:", err);
      toast.error(`Failed to update autostart: ${extractErrorMessage(err)}`);
      setAutostart(!checked);
    }
  };

  const handleCheckHealth = async () => {
    setChecking(true);
    try {
      const result = await api.checkAllHealth();
      setGateways(result);
    } catch (err) {
      console.error("Failed to check health:", err);
      toast.error(`Failed to check health: ${extractErrorMessage(err)}`);
    } finally {
      setChecking(false);
    }
  };

  const healthySummary = gateways.filter((g) => g.isHealthy).length;
  const totalGateways = gateways.length;

  return (
    <div className="flex flex-col h-screen overflow-hidden bg-background text-foreground">
      <Toaster position="top-right" richColors />

      {/* Header */}
      <header className="border-b border-border/60 bg-card/50 backdrop-blur-sm">
        <div className="flex items-center justify-between px-5 py-2.5">
          <div className="flex items-center gap-3 animate-fade-in">
            <h1 className="text-base font-semibold tracking-tight">{t('header.title')}</h1>
            {appVersion && <span className="text-xs text-muted-foreground font-normal">v{appVersion}</span>}
            {totalGateways > 0 && (
              <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
                <span className={`inline-block w-1.5 h-1.5 rounded-full ${healthySummary > 0 ? 'bg-success' : 'bg-muted-foreground'}`} />
                {t('header.online', { healthy: String(healthySummary), total: String(totalGateways) })}
              </span>
            )}
            {/* Name only, no inline editor — renaming lives in the settings
                drawer, so an unnamed device no longer needs a placeholder
                button here to stay reachable. */}
            {!loading && clientName && (
              <button
                onClick={() => setSettingsOpen(true)}
                className="text-xs text-muted-foreground transition-colors cursor-pointer hover:text-foreground"
                title={t('header.renameClient')}
              >
                {clientName}
              </button>
            )}
          </div>

          <div className="flex items-center gap-2 animate-fade-in" style={{ animationDelay: '0.1s' }}>
            {/* Only surfaced in the state that can surprise you. Failover on is
                the default and needs no reminder; failover off is why a dead
                gateway stays selected, so it says so and links to the switch. */}
            {!loading && !autoSwitch && (
              <button
                onClick={() => setSettingsOpen(true)}
                className="inline-flex items-center gap-1 rounded-full border border-amber-500/30 bg-amber-500/10 px-2 py-0.5 text-[11px] text-amber-600 transition-elegant hover:bg-amber-500/20 dark:text-amber-400"
                title={t('header.autoFailoverOffHint')}
              >
                <ZapOff className="h-3 w-3" />
                {t('header.autoFailoverOff')}
              </button>
            )}

            <Button
              variant="ghost"
              size="icon"
              onClick={() => api.openUrl("https://token.xianliao.de5.net/guide")}
              className="h-7 w-7 transition-elegant hover:bg-secondary"
              title={t('header.howToUse')}
            >
              <HelpCircle className="h-3.5 w-3.5" />
            </Button>

            <Button
              variant="ghost"
              size="icon"
              onClick={handleCheckHealth}
              disabled={checking}
              className="h-7 w-7 transition-elegant hover:bg-secondary"
            >
              {checking ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <RefreshCw className="h-3.5 w-3.5" />
              )}
            </Button>

            {/* Last in the row, hard against the edge the drawer slides in
                from — the hamburger reads as "more, over there". */}
            <Button
              variant="ghost"
              size="icon"
              onClick={() => setSettingsOpen(true)}
              className="h-7 w-7 transition-elegant hover:bg-secondary"
              title={t('settings.title')}
            >
              <Menu className="h-3.5 w-3.5" />
            </Button>
          </div>
        </div>
      </header>

      {/* Main content */}
      <main className="flex-1 overflow-y-auto px-4 py-4 min-h-0">
        {loading ? (
          <div className="flex items-center justify-center py-16">
            <div className="flex flex-col items-center gap-2">
              <Loader2 className="h-5 w-5 animate-spin text-primary" />
              <p className="text-xs text-muted-foreground">{t('common.loading')}</p>
            </div>
          </div>
        ) : (
          <div className="max-w-4xl mx-auto space-y-2">
            <GatewayList
              gateways={gateways}
              activeGatewayId={activeConfig?.gatewayId ?? null}
              activeKeyId={activeConfig?.keyId ?? null}
              activeKeyName={activeConfig?.keyName ?? null}
              extraConfigs={extraConfigs}
              onExtraConfigsChanged={setExtraConfigs}
              managedClients={managedClients}
              activeModels={{
                claude: activeConfig?.claudeModel ?? null,
                claudeSubagent: activeConfig?.claudeSubagentModel ?? null,
                claudeSmall: activeConfig?.claudeSmallModel ?? null,
                codex: activeConfig?.codexModel ?? null,
                codexSubagent: activeConfig?.codexSubagentModel ?? null,
                gemini: activeConfig?.geminiModel ?? null,
              }}
              onRefresh={loadAll}
            />

            <AddGatewayCard
              onAdded={loadAll}
              extraConfigs={extraConfigs}
              onExtraConfigsChanged={setExtraConfigs}
              managedClients={managedClients}
            />
          </div>
        )}
      </main>

      {/* Bottom panel: Usage + Error Logs */}
      <div className={`border-t border-border/60 bg-card/50 transition-all duration-300 ${showLogs ? "h-72" : "h-9"} shrink-0 flex flex-col`}>
        {/* Panel header / tab bar */}
        <div className="flex items-center h-9 shrink-0 border-b border-border/30">
          {/* Usage tab */}
          <button
            className={`flex items-center gap-1.5 px-3 h-full text-xs font-medium transition-colors border-r border-border/30 ${
              showLogs && bottomTab === "usage"
                ? "text-foreground bg-background/60"
                : "text-muted-foreground hover:text-foreground hover:bg-secondary/30"
            }`}
            onClick={() => {
              if (!showLogs) { setShowLogs(true); setBottomTab("usage"); }
              else if (bottomTab === "usage") setShowLogs(false);
              else setBottomTab("usage");
            }}
          >
            <BarChart3 className="h-3.5 w-3.5" />
            {t('usage.title')}
          </button>

          {/* Logs tab */}
          <button
            className={`flex items-center gap-1.5 px-3 h-full text-xs font-medium transition-colors ${
              showLogs && bottomTab === "logs"
                ? "text-foreground bg-background/60"
                : "text-muted-foreground hover:text-foreground hover:bg-secondary/30"
            }`}
            onClick={() => {
              if (!showLogs) { setShowLogs(true); setBottomTab("logs"); setLogErrorCount(0); }
              else if (bottomTab === "logs") setShowLogs(false);
              else { setBottomTab("logs"); setLogErrorCount(0); }
            }}
          >
            <AlertTriangle className="h-3.5 w-3.5" />
            {t('common.errors')}
            {logErrorCount > 0 && (bottomTab !== "logs" || !showLogs) && (
              <span className="inline-flex items-center justify-center h-4 min-w-4 px-1 rounded-full bg-destructive text-[9px] font-bold text-destructive-foreground">
                {logErrorCount > 99 ? "99+" : logErrorCount}
              </span>
            )}
          </button>

          <div className="flex-1" />
          <button
            className="px-3 h-full text-muted-foreground/50 hover:text-muted-foreground transition-colors"
            onClick={() => setShowLogs((v) => !v)}
          >
            <ChevronDown className={`h-3.5 w-3.5 transition-transform ${showLogs ? "rotate-0" : "rotate-180"}`} />
          </button>
        </div>

        {showLogs && (
          <div className="flex-1 overflow-hidden">
            {bottomTab === "usage" ? (
              <UsagePanel />
            ) : (
              <TrafficLogPanel
                filterGateway={logFilterGateway}
                onFilterChange={setLogFilterGateway}
                onSuppressionChange={loadSuppressed}
              />
            )}
          </div>
        )}
      </div>

      <SettingsSheet
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        autoSwitch={autoSwitch}
        onAutoSwitchChange={handleAutoSwitchChange}
        managedClients={managedClients}
        onManagedClientsChange={handleManagedClientsChange}
        autostart={autostart}
        onAutostartChange={handleAutostartChange}
        clientName={clientName}
        onClientNameChange={setClientName}
        canDisable={!!activeConfig?.gatewayId}
        onDisable={() => {
          // Let the drawer finish sliding out before the dialog takes over,
          // rather than stacking two Radix overlays. Matches the 200ms
          // sheetSlideOutRight in index.css.
          setSettingsOpen(false);
          setTimeout(() => setDisableOpen(true), 200);
        }}
      />

      <DisableRelayDialog
        open={disableOpen}
        onOpenChange={setDisableOpen}
        onDisabled={() => {
          setActiveConfig(null);
          loadAll();
        }}
      />

    </div>
  );
}

export default App;
