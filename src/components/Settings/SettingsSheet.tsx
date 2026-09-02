import { useEffect, useState } from "react";
import { toast } from "sonner";
import { PowerOff } from "lucide-react";
import { Sheet, SheetContent, SheetTitle } from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { WslDistros } from "@/components/Settings/WslDistros";
import * as api from "@/lib/api";
import { extractErrorMessage } from "@/lib/error";
import { useI18n, type Lang } from "@/lib/i18n";

interface SettingsSheetProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  autoSwitch: boolean;
  onAutoSwitchChange: (checked: boolean) => void;
  autostart: boolean;
  onAutostartChange: (checked: boolean) => void;
  clientName: string;
  onClientNameChange: (name: string) => void;
  canDisable: boolean;
  onDisable: () => void;
}

/// Set-once configuration, kept off the main surface. Auto-failover lives here
/// too rather than in the header: what you need from it while watching the
/// gateway list is *awareness*, not access, and the header covers that with an
/// indicator shown only when it's off.
export function SettingsSheet({
  open,
  onOpenChange,
  autoSwitch,
  onAutoSwitchChange,
  autostart,
  onAutostartChange,
  clientName,
  onClientNameChange,
  canDisable,
  onDisable,
}: SettingsSheetProps) {
  const { t, lang, toggleLang } = useI18n();
  const [nameDraft, setNameDraft] = useState(clientName);

  // Re-seed whenever the drawer opens: the name can also change from elsewhere
  // (first-run default, another window), and an abandoned edit shouldn't
  // survive until the next visit.
  useEffect(() => {
    if (open) setNameDraft(clientName);
  }, [open, clientName]);

  const commitName = async () => {
    const name = nameDraft.trim();
    if (!name || name === clientName) {
      setNameDraft(clientName);
      return;
    }
    try {
      await api.setClientName(name);
      onClientNameChange(name);
    } catch (err) {
      toast.error(`Failed to save client name: ${extractErrorMessage(err)}`);
      setNameDraft(clientName);
    }
  };

  const langs: { value: Lang; label: string }[] = [
    { value: "en", label: "EN" },
    { value: "zh", label: "中" },
  ];

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent>
        <div className="flex h-9 shrink-0 items-center border-b border-border/60 px-4">
          <SheetTitle>{t("settings.title")}</SheetTitle>
        </div>

        <div className="flex-1 space-y-4 overflow-y-auto px-4 py-4">
          {/* Device name */}
          <div className="space-y-1.5">
            <Label htmlFor="settings-client-name" className="text-xs font-medium">
              {t("settings.deviceName")}
            </Label>
            <input
              id="settings-client-name"
              value={nameDraft}
              onChange={(e) => setNameDraft(e.target.value)}
              onBlur={commitName}
              onKeyDown={(e) => {
                if (e.key === "Enter") e.currentTarget.blur();
                if (e.key === "Escape") setNameDraft(clientName);
              }}
              maxLength={32}
              placeholder={t("settings.deviceNamePlaceholder")}
              className="h-7 w-full rounded border border-border bg-background px-2 text-xs outline-none focus:border-primary/50"
            />
            <p className="text-[11px] text-muted-foreground">
              {t("settings.deviceNameHint")}
            </p>
          </div>

          {/* Language */}
          <div className="flex items-center justify-between gap-3">
            <Label className="text-xs font-medium">{t("settings.language")}</Label>
            <div className="flex overflow-hidden rounded border border-border/60">
              {langs.map((l) => (
                <button
                  key={l.value}
                  onClick={() => {
                    if (l.value !== lang) toggleLang();
                  }}
                  className={`px-2.5 py-1 text-xs transition-colors ${
                    l.value === lang
                      ? "bg-primary text-primary-foreground"
                      : "text-muted-foreground hover:bg-secondary"
                  }`}
                >
                  {l.label}
                </button>
              ))}
            </div>
          </div>

          {/* Launch at login */}
          <div className="flex items-center justify-between gap-3">
            <Label htmlFor="settings-autostart" className="text-xs font-medium cursor-pointer">
              {t("header.launchAtLogin")}
            </Label>
            <Switch
              id="settings-autostart"
              checked={autostart}
              onCheckedChange={onAutostartChange}
              className="scale-75"
            />
          </div>

          {/* Auto failover — last of the general settings, and the only one
              with a hint line, since it's what changes the relay's behaviour
              while you're not looking. */}
          <div className="space-y-1">
            <div className="flex items-center justify-between gap-3">
              <Label htmlFor="settings-auto-switch" className="text-xs font-medium cursor-pointer">
                {t("header.autoFailover")}
              </Label>
              <Switch
                id="settings-auto-switch"
                checked={autoSwitch}
                onCheckedChange={onAutoSwitchChange}
                className="scale-75"
              />
            </div>
            <p className="text-[11px] text-muted-foreground">
              {t("settings.autoFailoverHint")}
            </p>
          </div>

          {/* Windows only; renders nothing elsewhere, border included. */}
          <WslDistros />

          {/* Last, and only while the relay is actually active. It's a rare,
              one-way action that rewrites every CLI's config back — the header
              is the wrong place for something you click once and then not
              again for weeks. */}
          {canDisable && (
            <div className="border-t border-border/60 pt-4">
              <Button
                variant="outline"
                size="sm"
                onClick={onDisable}
                className="h-8 w-full gap-1.5 border-destructive/30 bg-destructive/5 text-xs text-destructive hover:bg-destructive/10 hover:text-destructive"
              >
                <PowerOff className="h-3.5 w-3.5" />
                {t("header.disableRelay")}
              </Button>
              <p className="mt-1.5 text-[11px] text-muted-foreground">
                {t("settings.disableHint")}
              </p>
            </div>
          )}
        </div>
      </SheetContent>
    </Sheet>
  );
}
