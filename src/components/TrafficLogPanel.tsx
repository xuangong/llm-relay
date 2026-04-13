import { useState, useEffect, useCallback } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { RefreshCw, AlertTriangle } from "lucide-react";
import { TrafficLogEntry } from "@/lib/api";
import * as api from "@/lib/api";

interface TrafficLogPanelProps {
  filterGateway: string;
  onFilterChange: (value: string) => void;
}

export function TrafficLogPanel({ filterGateway, onFilterChange }: TrafficLogPanelProps) {
  const [logs, setLogs] = useState<TrafficLogEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [gateways, setGateways] = useState<{ id: string; name: string }[]>([]);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const gid = filterGateway === "all" ? undefined : filterGateway;
      const [entries, gws] = await Promise.all([
        api.getTrafficLogs(gid, 200),
        api.listGateways(),
      ]);
      setLogs(entries);
      setGateways(gws.map((g) => ({ id: g.id, name: g.name })));
    } catch (e) {
      console.error(e);
    } finally {
      setLoading(false);
    }
  }, [filterGateway]);

  useEffect(() => {
    load();
  }, [load]);

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
    if (d.toDateString() === now.toDateString()) return "Today";
    const yesterday = new Date(now);
    yesterday.setDate(yesterday.getDate() - 1);
    if (d.toDateString() === yesterday.toDateString()) return "Yesterday";
    return d.toLocaleDateString([], { month: "short", day: "numeric" });
  };

  // Group by date for visual separation
  let lastDate = "";

  return (
    <div className="flex flex-col h-full">
      {/* Toolbar */}
      <div className="flex items-center gap-2 px-4 py-2 border-b border-border/40 bg-card/50 shrink-0">
        <AlertTriangle className="h-3.5 w-3.5 text-amber-500" />
        <span className="text-xs font-semibold text-foreground/80">Anomalous Traffic</span>
        <span className="text-xs text-muted-foreground">(last 24h)</span>

        {gateways.length > 1 && (
          <select
            value={filterGateway}
            onChange={(e) => onFilterChange(e.target.value)}
            className="ml-2 text-xs bg-background border border-border/60 rounded px-2 py-0.5 text-foreground"
          >
            <option value="all">All gateways</option>
            {gateways.map((g) => (
              <option key={g.id} value={g.id}>{g.name}</option>
            ))}
          </select>
        )}

        <div className="flex-1" />
        {logs.length > 0 && (
          <span className="text-xs text-muted-foreground">{logs.length} entries</span>
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
            <p className="text-xs">No anomalous traffic in the last 24h</p>
          </div>
        ) : (
          <table className="w-full text-xs">
            <thead className="sticky top-0 bg-card border-b border-border/40 z-10">
              <tr className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                <th className="text-left px-4 py-1.5 w-24">Time</th>
                <th className="text-left px-2 py-1.5 w-16">Status</th>
                <th className="text-left px-2 py-1.5 w-20">Latency</th>
                {filterGateway === "all" && (
                  <th className="text-left px-2 py-1.5 w-28">Gateway</th>
                )}
                <th className="text-left px-2 py-1.5">Path</th>
                <th className="text-left px-2 py-1.5">Detail</th>
              </tr>
            </thead>
            <tbody>
              {logs.map((entry) => {
                const dateLabel = formatDate(entry.loggedAt);
                const showDate = dateLabel !== lastDate;
                lastDate = dateLabel;
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
                      className={`border-b border-border/20 hover:bg-secondary/20 ${statusBg(entry.status)}`}
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
                      <td className="px-2 py-1.5 font-mono text-foreground/70 truncate max-w-[12rem]">
                        {entry.path}
                      </td>
                      <td className="px-2 py-1.5 text-muted-foreground/60 truncate max-w-[16rem]">
                        {entry.errorDetail ?? "—"}
                      </td>
                    </tr>
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
