import { useEffect, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown } from "lucide-react";

type WslDistroStatus = "ready" | "unreachable" | "unknown";

interface WslDistroInfo {
  name: string;
  isDefault: boolean;
  selected: boolean;
  home: string | null;
  hasClaude: boolean;
  hasCodex: boolean;
  hasGemini: boolean;
  resolvedUrl: string | null;
  status: WslDistroStatus;
}

const isWindows = (() => {
  if (typeof navigator === "undefined") return false;
  const ua = navigator.userAgent || "";
  const plat = (navigator as { platform?: string }).platform || "";
  return /Windows/i.test(ua) || /Win/i.test(plat);
})();

/// Most people pick their distros once and never look again, so the section
/// stays out of the way until asked for. Remembered across launches: someone
/// who opens it is usually mid-way through setting something up.
const OPEN_KEY = "wslDistrosOpen";

export function WslDistros() {
  const [distros, setDistros] = useState<WslDistroInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [open, setOpen] = useState(
    () => localStorage.getItem(OPEN_KEY) === "1"
  );

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const list = await invoke<WslDistroInfo[]>("list_wsl_distros");
      setDistros(list);
    } catch (e) {
      console.error("list_wsl_distros failed", e);
      setDistros([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!isWindows) return;
    void load();
  }, [load]);

  const handleRefresh = async () => {
    setRefreshing(true);
    try {
      await invoke("refresh_wsl_distros");
      setTimeout(() => {
        void load();
        setRefreshing(false);
      }, 1500);
    } catch (e) {
      console.error("refresh_wsl_distros failed", e);
      setRefreshing(false);
    }
  };

  const handleToggle = async (name: string, selected: boolean) => {
    try {
      await invoke("toggle_wsl_distro", { name, selected });
      setDistros((d) => d.map((x) => (x.name === name ? { ...x, selected } : x)));
    } catch (e) {
      console.error("toggle_wsl_distro failed", e);
    }
  };

  if (!isWindows) return null;

  const toggleOpen = () => {
    setOpen((prev) => {
      localStorage.setItem(OPEN_KEY, prev ? "0" : "1");
      return !prev;
    });
  };

  // Shown while collapsed, so the section still answers "is this set up?"
  // without being expanded. `list_wsl_distros` is a plain DB read, so the data
  // is loaded on mount either way.
  const selectedCount = distros.filter((d) => d.selected).length;
  const summary = loading
    ? "…"
    : distros.length === 0
      ? "none detected"
      : `${selectedCount}/${distros.length} selected`;

  return (
    <section
      className={`space-y-3 rounded-lg border border-border/60 bg-card/30 px-4 ${
        open ? "py-4" : "py-2.5"
      }`}
    >
      <header className="flex items-center justify-between gap-2">
        <button
          type="button"
          onClick={toggleOpen}
          aria-expanded={open}
          className="flex min-w-0 flex-1 items-center gap-1.5 text-left"
        >
          <ChevronDown
            className={`h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform ${
              open ? "" : "-rotate-90"
            }`}
          />
          <h3 className="text-sm font-semibold">WSL2 Distros</h3>
          {!open && (
            <span className="truncate text-xs text-muted-foreground">
              {summary}
            </span>
          )}
        </button>
        {open && (
          <button
            className="shrink-0 rounded border border-border/60 bg-secondary/60 px-2 py-1 text-xs hover:bg-secondary disabled:opacity-50"
            onClick={handleRefresh}
            disabled={refreshing || loading}
          >
            {refreshing ? "Refreshing…" : "🔄 Refresh"}
          </button>
        )}
      </header>

      {!open ? null : loading ? (
        <p className="text-xs text-muted-foreground">Loading…</p>
      ) : distros.length === 0 ? (
        <div className="space-y-1 text-xs text-muted-foreground">
          <p>No WSL2 distros detected.</p>
          <p>
            Install one via Microsoft Store or{" "}
            <code className="rounded bg-secondary px-1">wsl --install</code>, then
            Refresh.
          </p>
        </div>
      ) : (
        <ul className="space-y-2">
          {distros.map((d) => (
            <li key={d.name} className="flex items-start gap-3">
              <input
                type="checkbox"
                className="mt-1"
                checked={d.selected}
                onChange={(e) => handleToggle(d.name, e.target.checked)}
              />
              <div className="flex-1 text-xs">
                <div>
                  <span className="font-medium">{d.name}</span>
                  {d.isDefault && (
                    <span className="ml-1 text-muted-foreground">(default)</span>
                  )}
                </div>
                <div className="text-muted-foreground">
                  {d.home ?? "(home unknown)"} ·{" "}
                  <span className={d.hasClaude ? "" : "opacity-40"}>
                    claude {d.hasClaude ? "✓" : "✗"}
                  </span>{" "}
                  <span className={d.hasCodex ? "" : "opacity-40"}>
                    codex {d.hasCodex ? "✓" : "✗"}
                  </span>{" "}
                  <span className={d.hasGemini ? "" : "opacity-40"}>
                    gemini {d.hasGemini ? "✓" : "✗"}
                  </span>
                </div>
                <StatusLine status={d.status} url={d.resolvedUrl} />
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function StatusLine({
  status,
  url,
}: {
  status: WslDistroStatus;
  url: string | null;
}) {
  switch (status) {
    case "ready":
      return <div className="text-emerald-500">→ {url}</div>;
    case "unreachable":
      return (
        <div className="text-amber-500">
          Unreachable — ensure curl or wget is installed in this distro, then
          Refresh.
        </div>
      );
    case "unknown":
      return <div className="text-muted-foreground">Not yet probed.</div>;
  }
}
