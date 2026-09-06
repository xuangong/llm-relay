import { useEffect, useState } from "react";
import { toast } from "sonner";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Loader2 } from "lucide-react";
import * as api from "@/lib/api";
import type { LifecycleTargetStatus } from "@/lib/api";
import { extractErrorMessage } from "@/lib/error";
import { useI18n } from "@/lib/i18n";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onDisabled: () => void;
}

export function DisableRelayDialog({ open, onOpenChange, onDisabled }: Props) {
  const { t } = useI18n();
  const [targets, setTargets] = useState<LifecycleTargetStatus[]>([]);
  const [loading, setLoading] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  useEffect(() => {
    if (!open) return;
    setLoading(true);
    api
      .listCliLifecycleStatus()
      .then(setTargets)
      .catch((err) => {
        console.error("Failed to list CLI lifecycle status:", err);
        setTargets([]);
      })
      .finally(() => setLoading(false));
  }, [open]);

  const handleConfirm = async () => {
    setSubmitting(true);
    try {
      await api.clearConfig();
      toast.success(t("disable.success"));
      onDisabled();
      onOpenChange(false);
    } catch (err) {
      toast.error(t("disable.failed", { error: extractErrorMessage(err) }));
      try {
        setTargets(await api.listCliLifecycleStatus());
      } catch {}
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-xl">
        <DialogHeader>
          <DialogTitle>{t("disable.title")}</DialogTitle>
          <DialogDescription>{t("disable.intro")}</DialogDescription>
        </DialogHeader>

        {loading ? (
          <div className="flex items-center justify-center py-6">
            <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
          </div>
        ) : targets.length > 0 ? (
          <div className="max-h-72 space-y-3 overflow-y-auto">
            {targets.map((target) => (
              <div key={`${target.targetType}:${target.distroName ?? "native"}`} className="overflow-hidden rounded-md border border-border/60">
                <div className="flex items-center justify-between bg-muted/40 px-3 py-1.5 text-[11px] font-semibold">
                  <span>{target.label}</span>
                  <span className="font-normal text-muted-foreground">{target.phase}</span>
                </div>
                {target.pending && target.pendingReason && (
                  <p className="border-t border-border/40 px-3 py-2 text-[11px] text-amber-600 dark:text-amber-400">
                    {target.pendingReason}
                  </p>
                )}
                <div className="divide-y divide-border/40">
                  {target.files.map((file) => (
                    <div key={file.relativePath} className="px-3 py-2 text-[11px]">
                      <div className="flex items-center justify-between gap-3">
                        <span className="truncate font-mono">{file.relativePath}</span>
                        <span className="shrink-0 text-muted-foreground">
                          origin: {file.originExists ? t("disable.present") : t("disable.absent")}
                          {file.backupExists !== null && ` · bak: ${file.backupExists ? t("disable.present") : t("disable.absent")}`}
                        </span>
                      </div>
                      {file.error && <p className="mt-1 text-destructive">{file.error}</p>}
                    </div>
                  ))}
                </div>
              </div>
            ))}
          </div>
        ) : (
          <p className="text-xs text-muted-foreground">{t("disable.nothingToRestore")}</p>
        )}

        <DialogFooter>
          <Button variant="ghost" size="sm" onClick={() => onOpenChange(false)} disabled={submitting}>
            {t("disable.cancel")}
          </Button>
          <Button variant="destructive" size="sm" onClick={handleConfirm} disabled={submitting || loading}>
            {submitting && <Loader2 className="mr-1.5 h-3 w-3 animate-spin" />}
            {t("disable.confirm")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
