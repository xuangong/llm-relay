import { useEffect, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCw, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/lib/i18n";

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

export const isWindows = (() => {
  if (typeof navigator === "undefined") return false;
  const ua = navigator.userAgent || "";
  const plat = (navigator as { platform?: string }).platform || "";
  return /Windows/i.test(ua) || /Win/i.test(plat);
})();

/// Lives in the settings drawer rather than the main list: most people pick
/// their distros once during setup and never look again, and on macOS the
/// section doesn't exist at all.
export function WslDistros() {
  const { t } = useI18n();
  const [distros, setDistros] = useState<WslDistroInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);

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

  // Split rather than dangerouslySetInnerHTML so the command keeps its <code>
  // styling without the translation carrying markup.
  const [hintBefore, hintAfter] = t("wsl.installHint").split("{cmd}");

  return (
    <section className="space-y-3 border-t border-border/60 pt-4">
      <header className="flex items-center justify-between gap-2">
        <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          {t("wsl.title")}
        </h3>
        {/* Same shape as the header's health refresh — ghost icon button,
            spinner in place of the arrows while it runs. */}
        <Button
          variant="ghost"
          size="icon"
          onClick={handleRefresh}
          disabled={refreshing || loading}
          className="h-7 w-7 shrink-0 transition-elegant hover:bg-secondary"
          title={refreshing ? t("wsl.refreshing") : t("wsl.refresh")}
        >
          {refreshing ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <RefreshCw className="h-3.5 w-3.5" />
          )}
        </Button>
      </header>

      {loading ? (
        <p className="text-xs text-muted-foreground">{t("common.loading")}</p>
      ) : distros.length === 0 ? (
        <div className="space-y-1 text-xs text-muted-foreground">
          <p>{t("wsl.noneDetected")}</p>
          <p>
            {hintBefore}
            <code className="rounded bg-secondary px-1">wsl --install</code>
            {hintAfter}
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
                    <span className="ml-1 text-muted-foreground">
                      {t("wsl.default")}
                    </span>
                  )}
                </div>
                <div className="text-muted-foreground">
                  {d.home ?? t("wsl.homeUnknown")} ·{" "}
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
  const { t } = useI18n();
  switch (status) {
    case "ready":
      return <div className="text-emerald-500">→ {url}</div>;
    case "unreachable":
      return <div className="text-amber-500">{t("wsl.unreachable")}</div>;
    case "unknown":
      return <div className="text-muted-foreground">{t("wsl.notProbed")}</div>;
  }
}
