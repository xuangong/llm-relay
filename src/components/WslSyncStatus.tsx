import { useEffect, useState } from "react";
import { AlertTriangle, Loader2 } from "lucide-react";
import { listCliLifecycleStatus, type LifecycleTargetStatus } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { isWindows } from "@/components/Settings/WslDistros";

export function WslSyncStatus({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { t } = useI18n();
  const [pending, setPending] = useState<LifecycleTargetStatus[]>([]);
  const [unavailable, setUnavailable] = useState(false);
  const [released, setReleased] = useState<LifecycleTargetStatus[]>([]);

  useEffect(() => {
    if (!isWindows) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const refresh = async () => {
      try {
        const targets = await listCliLifecycleStatus();
        if (disposed) return;
        setPending(targets.filter((target) => target.targetType === "wsl" && target.pending));
        setReleased(targets.filter((target) => target.releasedClients?.some((client) => client.officialLogin || client.cleanupPending)));
        setUnavailable(false);
      } catch {
        // Keep the last pending state: a failed read does not mean sync finished.
        if (!disposed) setUnavailable(true);
      } finally {
        // Serialize reads so slow responses cannot overwrite newer status.
        if (!disposed) timer = setTimeout(refresh, 1500);
      }
    };
    void refresh();
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, []);

  if (!isWindows || (!pending.length && !released.length && !unavailable)) return null;
  const failed = pending.some((target) => target.files.some((file) => file.error));
  const label = t(unavailable ? "wsl.statusUnavailable" : failed ? "wsl.syncFailedShort" : pending.length ? "wsl.syncPendingShort" : "wsl.loginDetected");
  const details = pending.map((target) =>
    `${target.distroName ?? target.label}${target.pendingReason ? `: ${target.pendingReason}` : ""}`
  ).concat(released.flatMap((target) => target.releasedClients.map((client) => `${target.distroName ?? "Windows Host"}: ${client.provider} · ${t(client.cleanupPending ? "wsl.loginCleanupPending" : "wsl.loginReleased")}`))).join("\n");

  return (
    <Button variant="ghost" size="sm" onClick={onOpenSettings}
      className={`h-7 max-w-full gap-1.5 text-xs ${failed || unavailable || released.length ? "text-amber-500" : "text-primary"}`}
      title={[label, details, t("wsl.openSyncDetails")].filter(Boolean).join("\n")}
      aria-label={`${label}. ${t("wsl.openSyncDetails")}`}>
      {failed || unavailable || !pending.length
        ? <AlertTriangle className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
        : <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
      <span className="truncate">{label}{pending.length > 0 && ` (${pending.length})`}</span>
    </Button>
  );
}
