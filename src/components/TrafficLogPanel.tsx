import { useState, useEffect, useCallback } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { RefreshCw, AlertTriangle, ChevronDown, Bell, BellOff } from "lucide-react";
import { TrafficLogEntry } from "@/lib/api";
import * as api from "@/lib/api";
import { useI18n } from "@/lib/i18n";

interface TrafficLogPanelProps {
  filterGateway: string;
  onFilterChange: (value: string) => void;
  /** Lets the shell refresh anything keyed off the mute list (e.g. the error badge). */
  onSuppressionChange?: () => void;
}

export function TrafficLogPanel({ filterGateway, onFilterChange, onSuppressionChange }: TrafficLogPanelProps) {
  const { t } = useI18n();
  const [logs, setLogs] = useState<TrafficLogEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [gateways, setGateways] = useState<{ id: string; name: string }[]>([]);
  const [expandedRow, setExpandedRow] = useState<number | null>(null);
  const [showSuppressed, setShowSuppressed] = useState(false);
  const [suppressedCount, setSuppressedCount] = useState(0);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const gid = filterGateway === "all" ? undefined : filterGateway;
      const [entries, gws, muted] = await Promise.all([
        api.getTrafficLogs(gid, 200, showSuppressed),
        api.listGateways(),
        api.listSuppressedPaths(),
      ]);
      setLogs(entries);
      setGateways(gws.map((g) => ({ id: g.id, name: g.name })));
      setSuppressedCount(muted.length);
    } catch (e) {
      console.error(e);
    } finally {
      setLoading(false);
    }
  }, [filterGateway, showSuppressed]);

  useEffect(() => {
    load();
  }, [load]);

  const toggleSuppress = async (path: string, isSuppressed: boolean) => {
    try {
      if (isSuppressed) await api.unsuppressPath(path);
      else await api.suppressPath(path);
      await load();
      onSuppressionChange?.();
    } catch (e) {
      console.error(e);
    }
  };

  // Auto-refresh when proxy-traffic fires a new anomalous entry
  useEffect(() => {
    const appWindow = getCurrentWebviewWindow();
    const unlisten = appWindow.listen("proxy-traffic", (evt: { payload: { status: number } }) => {
      if (evt.payload.status >= 400) {
        load();
      }
    });
    return () => { unlisten.then((fn) => fn()); };
  }, [load]);

  const statusColor = (s: number) => {
    if (s === 429) return "text-amber-500";
    if (s >= 500) return "text-destructive";
    if (s >= 400) return "text-orange-400";
    return "text-muted-foreground";
  };

  const statusBg = (s: number) => {
    if (s === 429) return "bg-amber-500/10 border-amber-500/20";
    if (s >= 500) return "bg-destructive/8 border-destructive/20";
    if (s >= 400) return "bg-orange-400/8 border-orange-400/20";
    return "";
  };

  const formatTime = (iso: string) => {
    const d = new Date(iso);
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
  };

  const formatDate = (iso: string) => {
    const d = new Date(iso);
    const now = new Date();
    if (d.toDateString() === now.toDateString()) return t('traffic.today');
    const yesterday = new Date(now);
    yesterday.setDate(yesterday.getDate() - 1);
    if (d.toDateString() === yesterday.toDateString()) return t('traffic.yesterday');
    return d.toLocaleDateString([], { month: "short", day: "numeric" });
  };

  // Group by date for visual separation
  let lastDate = "";

  return (
    <div className="flex flex-col h-full">
      {/* Toolbar */}
      <div className="flex items-center gap-2 px-4 py-2 border-b border-border/40 bg-card/50 shrink-0">
        <AlertTriangle className="h-3.5 w-3.5 text-amber-500" />
        <span className="text-xs font-semibold text-foreground/80">{t('traffic.title')}</span>
        <span className="text-xs text-muted-foreground">{t('traffic.last24h')}</span>

        {gateways.length > 1 && (
          <select
            value={filterGateway}
            onChange={(e) => onFilterChange(e.target.value)}
            className="ml-2 text-xs bg-background border border-border/60 rounded px-2 py-0.5 text-foreground"
          >
            <option value="all">{t('traffic.allGateways')}</option>
            {gateways.map((g) => (
              <option key={g.id} value={g.id}>{g.name}</option>
            ))}
          </select>
        )}

        <div className="flex-1" />
        {suppressedCount > 0 && (
          <button
            onClick={() => setShowSuppressed((v) => !v)}
            title={t(showSuppressed ? 'traffic.hideSuppressed' : 'traffic.showSuppressed', { n: String(suppressedCount) })}
            className={`inline-flex items-center gap-1 text-[10px] font-medium px-1.5 py-0.5 rounded border transition-colors ${
              showSuppressed
                ? "bg-amber-500/15 border-amber-500/30 text-amber-500"
                : "border-border/60 text-muted-foreground/70 hover:text-foreground hover:border-border"
            }`}
          >
            <BellOff className="h-3 w-3" />
            {suppressedCount}
          </button>
        )}
        {logs.length > 0 && (
          <span className="text-xs text-muted-foreground">{t('traffic.entries', { n: String(logs.length) })}</span>
        )}
        <button
          onClick={load}
          disabled={loading}
          className="text-muted-foreground/60 hover:text-primary transition-colors"
        >
          <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
        </button>
      </div>

      {/* Log entries */}
      <div className="flex-1 overflow-y-auto">
        {logs.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full gap-2 text-muted-foreground/40">
            <AlertTriangle className="h-8 w-8" />
            <p className="text-xs">{t('traffic.noTraffic')}</p>
          </div>
        ) : (
          <table className="w-full text-xs">
            <thead className="sticky top-0 bg-card border-b border-border/40 z-10">
              <tr className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                <th className="text-left px-4 py-1.5 w-24">{t('traffic.time')}</th>
                <th className="text-left px-2 py-1.5 w-16">{t('traffic.status')}</th>
                <th className="text-left px-2 py-1.5 w-20">{t('traffic.latency')}</th>
                {filterGateway === "all" && (
                  <th className="text-left px-2 py-1.5 w-28">{t('traffic.gateway')}</th>
                )}
                <th className="text-left px-2 py-1.5">{t('traffic.path')}</th>
                <th className="text-left px-2 py-1.5">{t('traffic.detail')}</th>
              </tr>
            </thead>
            <tbody>
              {logs.map((entry) => {
                const dateLabel = formatDate(entry.loggedAt);
                const showDate = dateLabel !== lastDate;
                lastDate = dateLabel;
                const isExpanded = expandedRow === entry.id;
                const hasDetail = entry.errorDetail && entry.errorDetail.length > 0;

                return (
                  <>
                    {showDate && (
                      <tr key={`date-${entry.id}`}>
                        <td colSpan={filterGateway === "all" ? 6 : 5}
                          className="px-4 pt-3 pb-1 text-[10px] font-semibold text-muted-foreground/50 uppercase tracking-wider bg-secondary/10">
                          {dateLabel}
                        </td>
                      </tr>
                    )}
                    <tr
                      key={entry.id}
                      className={`group border-b border-border/20 hover:bg-secondary/20 transition-colors ${statusBg(entry.status)} ${entry.suppressed ? 'opacity-50' : ''} ${hasDetail ? 'cursor-pointer' : ''}`}
                      onClick={() => hasDetail && setExpandedRow(isExpanded ? null : entry.id)}
                    >
                      <td className="px-4 py-1.5 font-mono text-muted-foreground whitespace-nowrap">
                        {formatTime(entry.loggedAt)}
                      </td>
                      <td className={`px-2 py-1.5 font-mono font-semibold ${statusColor(entry.status)}`}>
                        {entry.status}
                      </td>
                      <td className="px-2 py-1.5 text-muted-foreground font-mono">
                        {entry.latencyMs}ms
                      </td>
                      {filterGateway === "all" && (
                        <td className="px-2 py-1.5 text-muted-foreground/70 truncate max-w-[7rem]">
                          {entry.gatewayName ?? entry.gatewayId.slice(0, 8)}
                        </td>
                      )}
                      <td className="px-2 py-1.5 font-mono text-foreground/70 max-w-[12rem]">
                        <div className="flex items-center gap-1">
                          <span className={`truncate ${entry.suppressed ? 'line-through' : ''}`}>
                            {entry.path}
                          </span>
                          <button
                            title={entry.suppressed ? t('traffic.unsuppress') : t('traffic.suppress')}
                            onClick={(e) => {
                              e.stopPropagation();
                              toggleSuppress(entry.path, entry.suppressed);
                            }}
                            className={`shrink-0 transition-opacity ${
                              entry.suppressed
                                ? "text-amber-500 hover:text-amber-400"
                                : "opacity-0 group-hover:opacity-100 focus:opacity-100 text-muted-foreground/50 hover:text-amber-500"
                            }`}
                          >
                            {entry.suppressed
                              ? <Bell className="h-3 w-3" />
                              : <BellOff className="h-3 w-3" />}
                          </button>
                        </div>
                      </td>
                      <td className="px-2 py-1.5 text-muted-foreground/60 flex items-center gap-1">
                        {entry.suppressed && (
                          <span className="shrink-0 text-[9px] uppercase tracking-wider px-1 rounded bg-amber-500/15 text-amber-500 border border-amber-500/25">
                            {t('traffic.suppressed')}
                          </span>
                        )}
                        {hasDetail ? (
                          <>
                            <span className={`flex-1 ${isExpanded ? '' : 'truncate max-w-[14rem]'}`}>
                              {entry.errorDetail}
                            </span>
                            <ChevronDown className={`h-3 w-3 shrink-0 transition-transform ${isExpanded ? 'rotate-180' : ''}`} />
                          </>
                        ) : (
                          <span>—</span>
                        )}
                      </td>
                    </tr>
                    {isExpanded && hasDetail && (
                      <tr key={`detail-${entry.id}`} className={statusBg(entry.status)}>
                        <td colSpan={filterGateway === "all" ? 6 : 5} className="px-4 py-2 bg-secondary/5">
                          <div className="text-xs space-y-1">
                            <div className="font-semibold text-foreground/80">{t('traffic.errorDetail')}</div>
                            <div className="font-mono text-muted-foreground whitespace-pre-wrap break-all bg-background/50 p-2 rounded border border-border/40">
                              {entry.errorDetail}
                            </div>
                          </div>
                        </td>
                      </tr>
                    )}
                  </>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
