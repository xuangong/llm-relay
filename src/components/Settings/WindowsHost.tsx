import { useEffect, useState } from "react";
import { toast } from "sonner";
import { getWindowsHostEnabled, setWindowsHostEnabled, listCliLifecycleStatus, type LifecycleTargetStatus } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { extractErrorMessage } from "@/lib/error";
import { isWindows } from "@/components/Settings/WslDistros";
import { EnvironmentStatus } from "@/components/Settings/EnvironmentStatus";

export function WindowsHost() {
  const { t } = useI18n();
  const [selected, setSelected] = useState<boolean | null>(null);
  const [target, setTarget] = useState<LifecycleTargetStatus>();
  const [changing, setChanging] = useState(false);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    if (!isWindows) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const refresh = async () => {
      try {
        const [enabled, status] = await Promise.all([getWindowsHostEnabled(), listCliLifecycleStatus()]);
        if (!disposed) {
          setSelected(enabled);
          setTarget(status.find((row) => row.targetType === "native"));
        }
      } catch (error) { console.error("Windows Host status", error); }
      finally { if (!disposed) timer = setTimeout(refresh, 1500); }
    };
    void refresh();
    return () => { disposed = true; clearTimeout(timer); };
  }, [revision]);
  if (!isWindows) return null;
  return (
    <section className="space-y-3 border-t border-border/60 pt-4">
      <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">Windows Host</h3>
      <label className="flex items-center gap-3 text-xs">
        <input type="checkbox" checked={selected === true} disabled={selected === null || changing}
          onChange={async (event) => {
            const enabled = event.target.checked;
            setChanging(true);
            try { await setWindowsHostEnabled(enabled); }
            catch (error) { toast.error(extractErrorMessage(error)); }
            finally { setChanging(false); setRevision((value) => value + 1); }
          }} />
        <span>{t("wsl.manageHost")}</span>
      </label>
      <p className="text-xs text-muted-foreground">{t("wsl.environmentHint")}</p>
      <EnvironmentStatus target={target} />
      {selected === false && <p className="text-xs text-muted-foreground">{t("wsl.environmentDisabled")}</p>}
    </section>
  );
}
