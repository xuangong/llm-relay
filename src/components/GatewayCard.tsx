import { useState, useEffect, useCallback, useRef } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { toast } from "sonner";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { Card, CardContent } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import {
  GatewayWithHealth,
  ApplyConfigParams,
  HealthLogEntry,
} from "@/lib/api";
import * as api from "@/lib/api";
import { extractErrorMessage } from "@/lib/error";
import { useI18n } from "@/lib/i18n";
import { EditGatewayDialog } from "./EditGatewayDialog";

interface ProxyTrafficEntry {
  path: string;
  status: number;
  latencyMs: number;
  gatewayId: string;
  timestamp: string;
}
import {
  GripVertical,
  Trash2,
  Check,
  Loader2,
  ChevronDown,
  ChevronRight,
  LogIn,
  KeyRound,
} from "lucide-react";

interface GatewayCardProps {
  gateway: GatewayWithHealth;
  isActive: boolean;
  activeKeyId: string | null;
  activeKeyName: string | null;
  activeModels: {
    claude: string | null;
    claudeSmall: string | null;
    codex: string | null;
    gemini: string | null;
  };
  dragHandleProps?: Record<string, unknown>;
  onSelect: () => void;
  onDelete: () => void;
  onApplied: () => void;
}

export function GatewayCard({
  gateway,
  isActive,
  activeKeyId,
  activeKeyName,
  activeModels,
  dragHandleProps,
  onSelect,
  onDelete,
  onApplied,
}: GatewayCardProps) {
  const { t } = useI18n();
  const [expanded, setExpanded] = useState(isActive);
  const [applying, setApplying] = useState(false);
  const [healthLog, setHealthLog] = useState<HealthLogEntry[]>([]);
  const [trafficLog, setTrafficLog] = useState<ProxyTrafficEntry[]>([]);
  const trafficLogRef = useRef<ProxyTrafficEntry[]>([]);
  const [showEditDialog, setShowEditDialog] = useState(false);

  // Tri-state: null = never checked, true = healthy, false = confirmed offline
  const neverChecked = gateway.lastChecked === null;
  const isHealthy = gateway.isHealthy;

  useEffect(() => {
    setExpanded(isActive);
  }, [isActive]);

  const loadHealthLog = useCallback(async () => {
    try {
      const log = await api.getHealthLog(gateway.id);
      setHealthLog(log);
    } catch {
      // silently ignore
    }
  }, [gateway.id]);

  // Reload health log whenever backend emits health-updated
  useEffect(() => {
    if (!expanded) return;
    const appWindow = getCurrentWebviewWindow();
    const unlisten = appWindow.listen("health-updated", () => {
      loadHealthLog();
    });
    return () => { unlisten.then((fn) => fn()); };
  }, [expanded, loadHealthLog]);

  // Listen for proxy-traffic events on the active gateway
  useEffect(() => {
    if (!isActive) return;
    const appWindow = getCurrentWebviewWindow();
    const unlisten = appWindow.listen<ProxyTrafficEntry>("proxy-traffic", (evt) => {
      if (evt.payload.gatewayId !== gateway.id) return;
      const updated = [evt.payload, ...trafficLogRef.current].slice(0, 40);
      trafficLogRef.current = updated;
      setTrafficLog(updated);
    });
    return () => { unlisten.then((fn) => fn()); };
  }, [isActive, gateway.id]);

  useEffect(() => {
    if (!expanded) return;
    loadHealthLog();
  }, [expanded]);

  const handleApply = async () => {
    setApplying(true);
    try {
      const params: ApplyConfigParams = {
        gatewayId: gateway.id,
      };
      await api.applyConfig(params);
      onApplied();
    } catch (err) {
      console.error("Failed to apply config:", err);
      toast.error(`Failed to apply: ${extractErrorMessage(err)}`);
    } finally {
      setApplying(false);
    }
  };

  const handleToggle = () => {
    if (!isActive) {
      onSelect();
    }
    setExpanded(!expanded);
  };

  // Card border style: tri-state
  const cardClass = isActive
    ? "border-primary/60 shadow-[0_0_0_1px_hsl(var(--primary)/0.15)] bg-gradient-to-br from-card to-primary/5"
    : neverChecked
    ? "hover:border-border hover:shadow-elegant bg-card"
    : !isHealthy
    ? "border-destructive/30 bg-destructive/[0.02] hover:border-destructive/40"
    : "hover:border-border hover:shadow-elegant bg-card";

  return (
    <>
      <Card className={`transition-elegant overflow-hidden ${cardClass}`}>
        {/* Header */}
        <div
          className="flex items-center gap-3 px-3 py-2.5 cursor-pointer select-none group"
          onClick={handleToggle}
        >
          <div
            {...dragHandleProps}
            className="cursor-grab active:cursor-grabbing text-muted-foreground/30 hover:text-muted-foreground transition-elegant-fast"
            onClick={(e) => e.stopPropagation()}
          >
            <GripVertical className="h-4 w-4" />
          </div>

          {/* Status dot — tri-state */}
          <div className="flex-shrink-0 relative w-2.5 h-2.5">
            <div className={`w-2.5 h-2.5 rounded-full ${
              neverChecked ? 'bg-muted-foreground/30' : isHealthy ? 'bg-green-500' : 'bg-destructive'
            }`} />
            {!neverChecked && isHealthy && (
              <span className="absolute inset-0 rounded-full bg-green-500 animate-ping opacity-40" />
            )}
          </div>

          {/* Gateway info */}
          <div className="flex-1 min-w-0">
            <div className="flex items-center gap-1.5 min-w-0">
              <span className="font-medium text-sm truncate">{gateway.name}</span>
              {isActive && (
                <span className="shrink-0 inline-flex items-center px-1.5 py-0.5 rounded text-[9px] font-bold uppercase tracking-wider bg-primary text-primary-foreground">
                  {t('gateway.inUse')}
                </span>
              )}
              <span className="text-[11px] text-muted-foreground font-mono truncate">{gateway.url}</span>
              {gateway.userName && (
                <span className="text-[10px] text-muted-foreground/60 truncate">
                  ({gateway.userName})
                </span>
              )}
            </div>
          </div>

          {/* Metrics — tri-state */}
          {neverChecked ? (
            <span className="text-xs text-muted-foreground/40 shrink-0">—</span>
          ) : isHealthy ? (
            <div className="flex items-center gap-1.5 text-xs text-muted-foreground shrink-0">
              <span className="font-medium text-foreground/80">{gateway.latencyMs}ms</span>
              <span className="text-border">·</span>
              <span>{gateway.modelCount}m</span>
            </div>
          ) : (
            <span className="shrink-0 inline-flex items-center px-1.5 py-0.5 rounded text-[9px] font-bold uppercase tracking-wider bg-destructive/15 text-destructive border border-destructive/25">
              {t('gateway.offline')}
            </span>
          )}

          {/* Sign in to edit button */}
          <button
            onClick={(e) => { e.stopPropagation(); setShowEditDialog(true); }}
            className="text-muted-foreground/40 hover:text-primary transition-elegant-fast flex items-center gap-1"
            title={t('gateway.signInToEdit')}
          >
            <LogIn className="h-3.5 w-3.5" />
          </button>

          {/* Expand */}
          <div className="text-muted-foreground/40 group-hover:text-muted-foreground/70 transition-elegant-fast">
            {expanded ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}
          </div>
        </div>

        {/* Expanded content */}
        <AnimatePresence>
          {expanded && (
            <motion.div
              initial={{ height: 0, opacity: 0 }}
              animate={{ height: "auto", opacity: 1 }}
              exit={{ height: 0, opacity: 0 }}
              transition={{ duration: 0.3, ease: [0.4, 0, 0.2, 1] }}
              style={{ overflow: "hidden" }}
            >
              <div className="border-t border-border/40 bg-card/50">
                <CardContent className="px-4 pt-3 pb-3 space-y-3">
                  {/* Current key display (read-only) */}
                  {isActive && activeKeyId && (
                    <div className="space-y-1.5">
                      <label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                        {t('gateway.currentKey')}
                      </label>
                      <div className="flex items-center gap-2 px-3 py-2 rounded-lg border border-border/50 bg-secondary/20">
                        <KeyRound className="h-3.5 w-3.5 text-muted-foreground/50 shrink-0" />
                        <span className="text-xs font-medium truncate">{activeKeyName || activeKeyId}</span>
                        <span className="text-[10px] text-muted-foreground font-mono shrink-0">
                          ...{gateway.authKey.slice(-4)}
                        </span>
                      </div>
                    </div>
                  )}

                  {/* Health history sparkline */}
                  {healthLog.length > 0 && (
                    <HealthSparkline log={healthLog} />
                  )}

                  {/* Proxy traffic monitor (active gateway only) */}
                  {isActive && trafficLog.length > 0 && (
                    <TrafficMonitor log={trafficLog} />
                  )}

                  {/* Model selectors (read-only display of active models) */}
                  {isActive && (activeModels.claude || activeModels.claudeSmall || activeModels.codex || activeModels.gemini) && (
                    <div className="space-y-1.5">
                      <label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                        {t('gateway.models')}
                      </label>
                      <div className="grid grid-cols-2 gap-1.5">
                        {activeModels.claude && (
                          <div className="text-[10px] text-muted-foreground">
                            <span className="font-medium">{t('models.claude')}:</span>{" "}
                            <span className="font-mono">{activeModels.claude}</span>
                          </div>
                        )}
                        {activeModels.claudeSmall && (
                          <div className="text-[10px] text-muted-foreground">
                            <span className="font-medium">{t('models.claudeSmall')}:</span>{" "}
                            <span className="font-mono">{activeModels.claudeSmall}</span>
                          </div>
                        )}
                        {activeModels.codex && (
                          <div className="text-[10px] text-muted-foreground">
                            <span className="font-medium">{t('models.codex')}:</span>{" "}
                            <span className="font-mono">{activeModels.codex}</span>
                          </div>
                        )}
                        {activeModels.gemini && (
                          <div className="text-[10px] text-muted-foreground">
                            <span className="font-medium">{t('models.gemini')}:</span>{" "}
                            <span className="font-mono">{activeModels.gemini}</span>
                          </div>
                        )}
                      </div>
                    </div>
                  )}

                  {/* Action buttons */}
                  <div className="flex items-center justify-between pt-2 border-t border-border/30">
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={(e) => { e.stopPropagation(); onDelete(); }}
                      className="h-7 px-2 text-xs text-destructive hover:text-destructive hover:bg-destructive/10"
                    >
                      <Trash2 className="h-3 w-3 mr-1" />
                      {t('common.remove')}
                    </Button>
                    {!isActive && (
                      <Button
                        size="sm"
                        onClick={handleApply}
                        disabled={applying}
                        className="h-7 px-3 text-xs"
                      >
                        {applying ? <Loader2 className="h-3 w-3 mr-1 animate-spin" /> : <Check className="h-3 w-3 mr-1" />}
                        {t('common.use')}
                      </Button>
                    )}
                  </div>
                </CardContent>
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </Card>

      <EditGatewayDialog
        open={showEditDialog}
        onOpenChange={setShowEditDialog}
        gateway={gateway}
        onUpdated={onApplied}
      />
    </>
  );
}

function HealthSparkline({ log }: { log: HealthLogEntry[] }) {
  const { t } = useI18n();
  if (log.length === 0) return null;

  // Sort oldest → newest, show all available checks
  const sorted = [...log].sort(
    (a, b) => new Date(a.checkedAt).getTime() - new Date(b.checkedAt).getTime()
  );

  const totalChecks = sorted.length;
  const healthyChecks = sorted.filter((e) => e.isHealthy).length;
  const uptimePct = Math.round((healthyChecks / totalChecks) * 100);
  const avgLatency = Math.round(
    sorted.filter((e) => e.isHealthy && e.latencyMs).reduce((s, e) => s + (e.latencyMs ?? 0), 0) /
    Math.max(healthyChecks, 1)
  );

  const maxLatency = Math.max(...sorted.map((e) => e.latencyMs ?? 0), 1);
  const BAR_MAX_PX = 28;

  // Oldest entry time for label
  const oldestTime = new Date(sorted[0].checkedAt);
  const oldestLabel = oldestTime.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

  return (
    <div className="space-y-1.5">
      <div className="flex items-center justify-between">
        <label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
          {t('gateway.healthMonitor')}
        </label>
        <span className="text-[10px] text-muted-foreground">
          {t('gateway.uptimeStats', { pct: String(uptimePct), ms: String(avgLatency), n: String(totalChecks) })}
        </span>
      </div>
      <div className="flex items-end gap-px h-10 bg-secondary/20 rounded px-1 py-1">
        {sorted.map((check, i) => {
          const heightPx = check.isHealthy
            ? Math.max(4, Math.round(((check.latencyMs ?? 0) / maxLatency) * BAR_MAX_PX))
            : BAR_MAX_PX;
          const color = check.isHealthy
            ? "bg-green-500/70 hover:bg-green-500"
            : "bg-destructive/70 hover:bg-destructive";
          const time = new Date(check.checkedAt).toLocaleTimeString([], {
            hour: "2-digit", minute: "2-digit", second: "2-digit",
          });
          return (
            <div
              key={i}
              className={`flex-1 rounded-sm transition-colors cursor-default ${color}`}
              style={{ height: `${heightPx}px` }}
              title={check.isHealthy ? `${time} · ${check.latencyMs}ms` : `${time} · ${t('gateway.down')}`}
            />
          );
        })}
      </div>
      <div className="flex justify-between text-[9px] text-muted-foreground/60 px-1">
        <span>{oldestLabel}</span>
        <span>{t('gateway.now')}</span>
      </div>
    </div>
  );
}

function TrafficMonitor({ log }: { log: ProxyTrafficEntry[] }) {
  const { t } = useI18n();
  if (log.length === 0) return null;

  const recent = log.slice(0, 30);
  const errorCount = recent.filter((e) => e.status >= 400).length;
  const avgLatency = Math.round(
    recent.reduce((s, e) => s + e.latencyMs, 0) / recent.length
  );

  return (
    <div className="space-y-1.5">
      <div className="flex items-center justify-between">
        <label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
          {t('gateway.trafficMonitor')}
        </label>
        <span className="text-[10px] text-muted-foreground">
          {t('gateway.reqs', { n: String(recent.length) })} · avg {avgLatency}ms
          {errorCount > 0 && (
            <span className="text-destructive ml-1">· {t('gateway.errors', { n: String(errorCount) })}</span>
          )}
        </span>
      </div>
      <div className="flex items-center gap-px flex-row-reverse justify-end">
        {recent.map((entry, i) => {
          const isError = entry.status >= 400;
          const is429 = entry.status === 429;
          const color = is429
            ? "bg-amber-500/80"
            : isError
            ? "bg-destructive/80"
            : "bg-green-500/70";
          const label = is429
            ? `429 ${t('gateway.rateLimited')} · ${entry.latencyMs}ms`
            : isError
            ? `${entry.status} ${t('common.error')} · ${entry.latencyMs}ms`
            : `${entry.status} ${t('gateway.ok')} · ${entry.latencyMs}ms`;
          return (
            <div
              key={i}
              className={`w-2 h-2 rounded-full flex-shrink-0 ${color}`}
              title={`${entry.path} · ${label}`}
            />
          );
        })}
      </div>
    </div>
  );
}
