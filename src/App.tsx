import { useState, useEffect, useCallback } from "react";
import { Toaster, toast } from "sonner";
import { listen } from "@tauri-apps/api/event";
import { GatewayList } from "@/components/GatewayList";
import { AddGatewayDialog } from "@/components/AddGatewayDialog";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import * as api from "@/lib/api";
import type { GatewayWithHealth, ActiveConfig } from "@/lib/api";
import { Plus, RefreshCw, Loader2 } from "lucide-react";

function App() {
  const [gateways, setGateways] = useState<GatewayWithHealth[]>([]);
  const [activeConfig, setActiveConfig] = useState<ActiveConfig | null>(null);
  const [autoSwitch, setAutoSwitch] = useState(true);
  const [showAddDialog, setShowAddDialog] = useState(false);
  const [loading, setLoading] = useState(true);
  const [checking, setChecking] = useState(false);

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
      const settings = await api.getSettings();
      setAutoSwitch(settings.autoSwitch);
    } catch (err) {
      console.error("Failed to load settings:", err);
    }
  }, []);

  const loadAll = useCallback(async () => {
    await Promise.all([loadGateways(), loadSettings()]);
    try {
      const config = await api.getActiveConfig();
      setActiveConfig(config);
    } catch {
      // no active config yet
    }
  }, [loadGateways, loadSettings]);

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
    const unlisten1 = listen<GatewayWithHealth[]>("health-updated", (event) => {
      setGateways(event.payload);
    });

    const unlisten2 = listen("gateway-switched", (event) => {
      const data = event.payload as { gatewayName?: string };
      toast.success(`Switched to ${data.gatewayName || "new gateway"}`);
      loadAll();
    });

    return () => {
      unlisten1.then((fn) => fn());
      unlisten2.then((fn) => fn());
    };
  }, [loadAll]);

  const handleAutoSwitchChange = async (checked: boolean) => {
    setAutoSwitch(checked);
    try {
      await api.updateSettings(checked);
    } catch (err) {
      console.error("Failed to update settings:", err);
      setAutoSwitch(!checked);
    }
  };

  const handleCheckHealth = async () => {
    setChecking(true);
    try {
      const result = await api.checkAllHealth();
      setGateways(result);
    } catch (err) {
      console.error("Failed to check health:", err);
    } finally {
      setChecking(false);
    }
  };

  const healthySummary = gateways.filter((g) => g.isHealthy).length;
  const totalGateways = gateways.length;

  return (
    <div className="flex flex-col h-screen overflow-hidden bg-background text-foreground">
      <Toaster position="top-right" richColors />

      {/* Title bar drag region */}
      <div data-tauri-drag-region className="h-8 flex-shrink-0" />

      {/* Header */}
      <div className="flex items-center justify-between px-6 pb-4">
        <div>
          <h1 className="text-xl font-semibold">LLM Relay</h1>
          {totalGateways > 0 && (
            <p className="text-xs text-muted-foreground mt-0.5">
              {healthySummary}/{totalGateways} online
            </p>
          )}
        </div>
        <div className="flex items-center gap-4">
          <div className="flex items-center gap-2">
            <Switch
              id="auto-switch"
              checked={autoSwitch}
              onCheckedChange={handleAutoSwitchChange}
            />
            <Label htmlFor="auto-switch" className="text-sm cursor-pointer">
              Auto
            </Label>
          </div>

          <Button
            variant="ghost"
            size="icon"
            onClick={handleCheckHealth}
            disabled={checking}
          >
            {checking ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <RefreshCw className="h-4 w-4" />
            )}
          </Button>
        </div>
      </div>

      {/* Main content */}
      <div className="flex-1 overflow-y-auto px-6 pb-6">
        {loading ? (
          <div className="flex items-center justify-center py-12">
            <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
          </div>
        ) : (
          <>
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

            <div className="mt-4">
              <Button
                variant="outline"
                className="w-full border-dashed"
                onClick={() => setShowAddDialog(true)}
              >
                <Plus className="h-4 w-4 mr-2" />
                Add Gateway
              </Button>
            </div>
          </>
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
