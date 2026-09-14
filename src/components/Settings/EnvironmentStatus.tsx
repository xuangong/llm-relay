import type { LifecycleTargetStatus } from "@/lib/api";
import { useI18n } from "@/lib/i18n";

export function EnvironmentStatus({ target }: { target?: LifecycleTargetStatus }) {
  const { t } = useI18n();
  const released = target?.releasedClients ?? [];
  if (!released.length) return null;
  const official = released.filter((client) => client.officialLogin);
  const pending = released.some((client) => client.cleanupPending);
  return (
    <div className="mt-2 text-xs text-amber-500" role="status">
      {official.length > 0 && <p>{t("wsl.environmentLogin", { clients: official.map((client) => client.provider).join(", ") })}</p>}
      {pending && <p>{t("wsl.loginCleanupPending")}</p>}
      {released.filter((client) => client.error).map((client) => <p key={client.provider} className="mt-1 break-words select-text">{client.error}</p>)}
    </div>
  );
}
