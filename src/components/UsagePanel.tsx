import { useState, useEffect, useCallback } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { BarChart3, RefreshCw } from "lucide-react";
import { UsageSummary, UsagePeriod } from "@/lib/api";
import * as api from "@/lib/api";

const PERIODS: { key: UsagePeriod; label: string }[] = [
  { key: "today", label: "Today" },
  { key: "week",  label: "This Week" },
  { key: "7d",    label: "7 Days" },
  { key: "30d",   label: "30 Days" },
];

function fmt(n: number): string {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
  if (n >= 1_000)     return (n / 1_000).toFixed(1) + "K";
  return n.toString();
}

export function UsagePanel() {
  const [period, setPeriod] = useState<UsagePeriod>("today");
  const [rows, setRows] = useState<UsageSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [filterGateway, setFilterGateway] = useState<string>("all");
  const [gateways, setGateways] = useState<{ id: string; name: string }[]>([]);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const gid = filterGateway === "all" ? undefined : filterGateway;
      const [data, gws] = await Promise.all([
        api.getUsageStats(period, gid),
        api.listGateways(),
      ]);
      setRows(data);
      setGateways(gws.map((g) => ({ id: g.id, name: g.name })));
    } catch (e) {
      console.error(e);
    } finally {
      setLoading(false);
    }
  }, [period, filterGateway]);

  useEffect(() => { load(); }, [load]);

  // Refresh after each successful request (proxy-traffic fires on every request)
  useEffect(() => {
    const appWindow = getCurrentWebviewWindow();
    const unlisten = appWindow.listen<{ status: number }>("proxy-traffic", (evt) => {
      if (evt.payload.status < 400) load();
    });
    return () => { unlisten.then((fn) => fn()); };
  }, [load]);

  const totals = rows.reduce(
    (acc, r) => ({
      inputTokens:         acc.inputTokens + r.inputTokens,
      outputTokens:        acc.outputTokens + r.outputTokens,
      cacheReadTokens:     acc.cacheReadTokens + r.cacheReadTokens,
      cacheCreationTokens: acc.cacheCreationTokens + r.cacheCreationTokens,
      requests:            acc.requests + r.requests,
    }),
    { inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheCreationTokens: 0, requests: 0 }
  );

  const maxTotal = rows.reduce((m, r) => Math.max(m, r.inputTokens + r.outputTokens), 1);

  return (
    <div className="flex flex-col h-full">
      {/* Toolbar */}
      <div className="flex items-center gap-2 px-4 py-2 border-b border-border/40 bg-card/50 shrink-0">
        <BarChart3 className="h-3.5 w-3.5 text-primary/70" />
        <span className="text-xs font-semibold text-foreground/80">Token Usage</span>

        {/* Period tabs */}
        <div className="flex items-center gap-0.5 ml-2 bg-secondary/50 rounded-md p-0.5">
          {PERIODS.map((p) => (
            <button
              key={p.key}
              onClick={() => setPeriod(p.key)}
              className={`px-2 py-0.5 rounded text-[11px] font-medium transition-colors ${
                period === p.key
                  ? "bg-background text-foreground shadow-sm"
                  : "text-muted-foreground hover:text-foreground"
              }`}
            >
              {p.label}
            </button>
          ))}
        </div>

        {gateways.length > 1 && (
          <select
            value={filterGateway}
            onChange={(e) => setFilterGateway(e.target.value)}
            className="ml-2 text-xs bg-background border border-border/60 rounded px-2 py-0.5 text-foreground"
          >
            <option value="all">All gateways</option>
            {gateways.map((g) => (
              <option key={g.id} value={g.id}>{g.name}</option>
            ))}
          </select>
        )}

        <div className="flex-1" />
        {rows.length > 0 && (
          <span className="text-xs text-muted-foreground">
            {totals.requests} req · {fmt(totals.inputTokens + totals.outputTokens)} tokens
          </span>
        )}
        <button
          onClick={load}
          disabled={loading}
          className="text-muted-foreground/60 hover:text-primary transition-colors"
        >
          <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
        </button>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto">
        {rows.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full gap-2 text-muted-foreground/40">
            <BarChart3 className="h-8 w-8" />
            <p className="text-xs">No usage data for this period</p>
          </div>
        ) : (
          <div className="px-4 py-3 space-y-3">
            {rows.map((row) => {
              const total = row.inputTokens + row.outputTokens;
              const barPct = Math.round((total / maxTotal) * 100);
              const cacheTotal = row.cacheReadTokens + row.cacheCreationTokens;
              return (
                <div key={row.model} className="space-y-1">
                  <div className="flex items-baseline justify-between gap-2">
                    <span className="text-xs font-mono text-foreground/80 truncate max-w-[16rem]">
                      {row.model}
                    </span>
                    <span className="text-[11px] text-muted-foreground shrink-0">
                      {row.requests} req
                    </span>
                  </div>
                  {/* Bar */}
                  <div className="h-1.5 bg-secondary/60 rounded-full overflow-hidden">
                    <div
                      className="h-full bg-primary/60 rounded-full"
                      style={{ width: `${barPct}%` }}
                    />
                  </div>
                  {/* Token breakdown */}
                  <div className="flex items-center gap-3 text-[10px] text-muted-foreground">
                    <span>
                      <span className="text-blue-400/80">↑</span>{" "}
                      {fmt(row.inputTokens)} in
                    </span>
                    <span>
                      <span className="text-green-400/80">↓</span>{" "}
                      {fmt(row.outputTokens)} out
                    </span>
                    {cacheTotal > 0 && (
                      <span className="text-muted-foreground/50">
                        {fmt(cacheTotal)} cache
                      </span>
                    )}
                    <span className="ml-auto font-medium text-foreground/60">
                      {fmt(total)} total
                    </span>
                  </div>
                </div>
              );
            })}

            {/* Totals row */}
            {rows.length > 1 && (
              <div className="pt-2 border-t border-border/30 flex items-center justify-between text-[11px]">
                <span className="text-muted-foreground font-medium">{rows.length} models</span>
                <div className="flex items-center gap-3 text-muted-foreground">
                  <span>{fmt(totals.inputTokens)} in</span>
                  <span>{fmt(totals.outputTokens)} out</span>
                  <span className="font-semibold text-foreground/70">
                    {fmt(totals.inputTokens + totals.outputTokens)} total
                  </span>
                </div>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
