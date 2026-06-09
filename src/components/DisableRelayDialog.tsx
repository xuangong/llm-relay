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
import type { CliConfigSnapshot } from "@/lib/api";
import { extractErrorMessage } from "@/lib/error";
import { useI18n } from "@/lib/i18n";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onDisabled: () => void;
}

// Flattened (label, original) pairs derived from the snapshot, in display order.
function snapshotRows(snap: CliConfigSnapshot): Array<{ label: string; value: string | null }> {
  return [
    { label: "Claude · ANTHROPIC_BASE_URL", value: snap.claude.anthropicBaseUrl },
    { label: "Claude · ANTHROPIC_MODEL", value: snap.claude.anthropicModel },
    { label: "Claude · ANTHROPIC_SMALL_FAST_MODEL", value: snap.claude.anthropicSmallFastModel },
    { label: "Claude · ANTHROPIC_AUTH_TOKEN", value: snap.claude.anthropicAuthToken },
    { label: "Codex · model", value: snap.codex.model },
    { label: "Codex · model_provider", value: snap.codex.modelProvider },
    { label: "Codex · OPENAI_API_KEY", value: snap.codex.openaiApiKey },
    {
      label: "Codex · [model_providers.copilot_gateway]",
      value: snap.codex.copilotGatewayProviderToml,
    },
    { label: "Gemini · GEMINI_API_KEY", value: snap.gemini.geminiApiKey },
    { label: "Gemini · GOOGLE_GEMINI_BASE_URL", value: snap.gemini.googleGeminiBaseUrl },
    { label: "Gemini · GEMINI_API_BASE_URL", value: snap.gemini.geminiApiBaseUrl },
    { label: "Gemini · security.auth.selectedType", value: snap.gemini.selectedAuthType },
  ];
}

// Truncate long values (tokens, TOML blocks) so the dialog stays readable.
function truncate(s: string, max = 80): string {
  if (s.length <= max) return s;
  return s.slice(0, max - 1) + "…";
}

export function DisableRelayDialog({ open, onOpenChange, onDisabled }: Props) {
  const { t } = useI18n();
  const [snapshot, setSnapshot] = useState<CliConfigSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  useEffect(() => {
    if (!open) return;
    setLoading(true);
    api
      .getConfigSnapshot()
      .then((s) => setSnapshot(s))
      .catch((err) => {
        console.error("Failed to load snapshot:", err);
        setSnapshot(null);
      })
      .finally(() => setLoading(false));
  }, [open]);

  const hasSnapshot = snapshot !== null;
  const rows = snapshot ? snapshotRows(snapshot) : [];

  const handleConfirm = async () => {
    setSubmitting(true);
    try {
      await api.clearConfig();
      toast.success(hasSnapshot ? t("disable.success") : t("disable.successCleared"));
      onDisabled();
      onOpenChange(false);
    } catch (err) {
      toast.error(t("disable.failed", { error: extractErrorMessage(err) }));
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
        ) : hasSnapshot ? (
          <div className="space-y-2">
            <p className="text-xs font-medium text-muted-foreground">
              {t("disable.restoreHeading")}
            </p>
            <div className="max-h-64 overflow-y-auto rounded-md border border-border/60 divide-y divide-border/40 text-xs">
              {rows.map((row) => (
                <div key={row.label} className="flex flex-col gap-0.5 px-3 py-2">
                  <span className="font-mono text-[11px] text-foreground/90">{row.label}</span>
                  {row.value === null ? (
                    <span className="text-[11px] text-muted-foreground italic">
                      {t("disable.removed")}
                    </span>
                  ) : (
                    <span className="text-[11px] text-muted-foreground">
                      <span className="opacity-70">{t("disable.setTo")} </span>
                      <span className="font-mono break-all">{truncate(row.value)}</span>
                    </span>
                  )}
                </div>
              ))}
            </div>
            {snapshot && (
              <p className="text-[11px] text-muted-foreground">
                {t("disable.capturedAt", {
                  time: new Date(snapshot.capturedAt).toLocaleString(),
                })}
              </p>
            )}
          </div>
        ) : (
          <p className="text-xs text-muted-foreground">{t("disable.nothingToRestore")}</p>
        )}

        <DialogFooter>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => onOpenChange(false)}
            disabled={submitting}
          >
            {t("disable.cancel")}
          </Button>
          <Button
            variant="destructive"
            size="sm"
            onClick={handleConfirm}
            disabled={submitting || loading}
          >
            {submitting && <Loader2 className="mr-1.5 h-3 w-3 animate-spin" />}
            {t("disable.confirm")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
