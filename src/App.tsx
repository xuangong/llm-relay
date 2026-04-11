import { useState, useEffect, useCallback, useRef } from "react";
import { Toaster, toast } from "sonner";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { GatewayList } from "@/components/GatewayList";
import { AddGatewayDialog } from "@/components/AddGatewayDialog";
import { TrafficLogPanel } from "@/components/TrafficLogPanel";
import { UsagePanel } from "@/components/UsagePanel";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import * as api from "@/lib/api";
import type { GatewayWithHealth, ActiveConfig } from "@/lib/api";
import { extractErrorMessage } from "@/lib/error";
import { Plus, RefreshCw, Loader2, AlertTriangle, ChevronDown, BarChart3 } from "lucide-react";

function App() {
  const [gateways, setGateways] = useState<GatewayWithHealth[]>([]);
  const [activeConfig, setActiveConfig] = useState<ActiveConfig | null>(null);
  const [autoSwitch, setAutoSwitch] = useState(true);
  const [autostart, setAutostart] = useState(false);
  const [showAddDialog, setShowAddDialog] = useState(false);
  const [loading, setLoading] = useState(true);
  const [checking, setChecking] = useState(false);
  const [showLogs, setShowLogs] = useState(false);
  const [logErrorCount, setLogErrorCount] = useState(0);
  const [bottomTab, setBottomTab] = useState<"usage" | "logs">("usage");
  const [clientName, setClientName] = useState("");
  const [editingClientName, setEditingClientName] = useState(false);
  const [clientNameDraft, setClientNameDraft] = useState("");
  const clientNameInputRef = useRef<HTMLInputElement>(null);

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

  const loadAll = useCallback(async () => {
    await Promise.all([loadGateways(), loadSettings(), loadClientName()]);
    try {
      const config = await api.getActiveConfig();
      setActiveConfig(config);
    } catch {
      // no active config yet
    }
  }, [loadGateways, loadSettings, loadClientName]);

  useEffect(() => {
    const init = async () => {
      setLoading(true);
      await loadAll();
      setLoading(false);
    };
    init();
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
      setActiveConfig((prev) => prev ? { ...prev, gatewayId } : { gatewayId, keyId: null, keyName: null, keyValue: null, claudeModel: null, claudeSmallModel: null, codexModel: null, geminiModel: null, autoSwitch: true, appliedAt: null });
      toast.success(`Switched to ${gatewayName || "new gateway"}`);
      loadAll();
    });

    const unlisten3 = appWindow.listen<{ status: number }>("proxy-traffic", (event) => {
      if (event.payload.status >= 400) {
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

  const handleSaveClientName = async () => {
    const name = clientNameDraft.trim();
    if (!name) return;
    try {
      await api.setClientName(name);
      setClientName(name);
      setEditingClientName(false);
    } catch (err) {
      toast.error(`Failed to save client name: ${extractErrorMessage(err)}`);
    }
  };

  const handleEditClientName = () => {
    setClientNameDraft(clientName);
    setEditingClientName(true);
    setTimeout(() => clientNameInputRef.current?.focus(), 50);
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
            <h1 className="text-base font-semibold tracking-tight">LLM Relay</h1>
            {totalGateways > 0 && (
              <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
                <span className={`inline-block w-1.5 h-1.5 rounded-full ${healthySummary > 0 ? 'bg-success' : 'bg-muted-foreground'}`} />
                {healthySummary}/{totalGateways} online
              </span>
            )}
            {/* Client name display/editor */}
            {!loading && (
              editingClientName ? (
                <form
                  className="flex items-center gap-1"
                  onSubmit={(e) => { e.preventDefault(); handleSaveClientName(); }}
                >
                  <input
                    ref={clientNameInputRef}
                    value={clientNameDraft}
                    onChange={(e) => setClientNameDraft(e.target.value)}
                    onBlur={handleSaveClientName}
                    onKeyDown={(e) => { if (e.key === "Escape") setEditingClientName(false); }}
                    className="text-xs h-5 px-1.5 rounded border border-border bg-background text-foreground outline-none focus:border-primary/50 w-28"
                    maxLength={32}
                  />
                </form>
              ) : (
                <button
                  onClick={handleEditClientName}
                  className="text-xs text-muted-foreground hover:text-foreground transition-colors cursor-pointer"
                  title="Click to rename this client"
                >
                  {clientName}
                </button>
              )
            )}
          </div>

          <div className="flex items-center gap-2 animate-fade-in" style={{ animationDelay: '0.1s' }}>
            <div className="flex items-center gap-1.5 px-2.5 py-1 rounded-full bg-secondary/60 transition-elegant hover:bg-secondary">
              <Label htmlFor="auto-switch" className="text-xs font-medium cursor-pointer">
                Auto Failover
              </Label>
              <Switch
                id="auto-switch"
                checked={autoSwitch}
                onCheckedChange={handleAutoSwitchChange}
                className="scale-75"
              />
            </div>

            <div className="flex items-center gap-1.5 px-2.5 py-1 rounded-full bg-secondary/60 transition-elegant hover:bg-secondary">
              <Label htmlFor="autostart" className="text-xs font-medium cursor-pointer">
                Launch at Login
              </Label>
              <Switch
                id="autostart"
                checked={autostart}
                onCheckedChange={handleAutostartChange}
                className="scale-75"
              />
            </div>

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
          </div>
        </div>
      </header>

      {/* Main content */}
      <main className="flex-1 overflow-y-auto px-4 py-4 min-h-0">
        {loading ? (
          <div className="flex items-center justify-center py-16">
            <div className="flex flex-col items-center gap-2">
              <Loader2 className="h-5 w-5 animate-spin text-primary" />
              <p className="text-xs text-muted-foreground">Loading...</p>
            </div>
          </div>
        ) : (
          <div className="max-w-4xl mx-auto space-y-2">
            <GatewayList
              gateways={gateways}
              activeGatewayId={activeConfig?.gatewayId ?? null}
              activeKeyId={activeConfig?.keyId ?? null}
              activeModels={{
                claude: activeConfig?.claudeModel ?? null,
                claudeSmall: activeConfig?.claudeSmallModel ?? null,
                codex: activeConfig?.codexModel ?? null,
                gemini: activeConfig?.geminiModel ?? null,
              }}
              onRefresh={loadAll}
            />

            <Button
              variant="outline"
              className="w-full border-dashed hover:border-primary/50 hover:bg-primary/5 transition-elegant h-9"
              onClick={() => setShowAddDialog(true)}
            >
              <Plus className="h-3.5 w-3.5 mr-1.5" />
              <span className="text-sm font-medium">Add Gateway</span>
            </Button>
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
            Usage
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
            Errors
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
            {bottomTab === "usage" ? <UsagePanel /> : <TrafficLogPanel />}
          </div>
        )}
      </div>

      <AddGatewayDialog
        open={showAddDialog}
        onOpenChange={setShowAddDialog}
        onAdded={loadAll}
      />
    </div>
  );
}

export default App;
