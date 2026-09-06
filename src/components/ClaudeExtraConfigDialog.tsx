import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import type { ClaudeExtraConfig } from "@/lib/api";
import * as api from "@/lib/api";
import { extractErrorMessage } from "@/lib/error";
import { useI18n } from "@/lib/i18n";
import { Plus, Trash2 } from "lucide-react";

interface Entry {
  key: string;
  value: string;
}

interface ClaudeExtraConfigDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  configs: ClaudeExtraConfig[];
  selectedId: string | null;
  onChanged: (configs: ClaudeExtraConfig[], selectedId?: string | null) => void;
}

export function ClaudeExtraConfigDialog({
  open,
  onOpenChange,
  configs,
  selectedId,
  onChanged,
}: ClaudeExtraConfigDialogProps) {
  const { t } = useI18n();
  const [editingId, setEditingId] = useState<string | null>(selectedId);
  const [name, setName] = useState("");
  const [entries, setEntries] = useState<Entry[]>([{ key: "", value: "" }]);
  const [saving, setSaving] = useState(false);
  const selectableConfigs = configs;

  const load = (config: ClaudeExtraConfig | null) => {
    setEditingId(config?.id ?? selectableConfigs[0]?.id ?? null);
    setName(config?.name ?? selectableConfigs[0]?.name ?? "");
    const source = config ?? selectableConfigs[0] ?? null;
    const next = source
      ? Object.entries(source.env).map(([key, value]) => ({ key, value }))
      : [];
    setEntries(next.length > 0 ? next : [{ key: "", value: "" }]);
  };

  useEffect(() => {
    if (!open) return;
    load(selectableConfigs.find((config) => config.id === selectedId) ?? selectableConfigs[0] ?? null);
  }, [open, configs, selectedId]);

  const save = async () => {
    const cleanName = name.trim();
    const env: Record<string, string> = {};
    for (const entry of entries) {
      const key = entry.key.trim();
      if (!key) continue;
      if (Object.prototype.hasOwnProperty.call(env, key)) {
        toast.error(t("extra.duplicateKey"));
        return;
      }
      env[key] = entry.value;
    }
    if (!cleanName || Object.keys(env).length === 0) {
      toast.error(t("extra.invalid"));
      return;
    }
    setSaving(true);
    try {
      const saved = editingId
        ? await api.updateClaudeExtraConfig(editingId, cleanName, env)
        : await api.createClaudeExtraConfig(cleanName, env);
      const next = editingId
        ? configs.map((config) => (config.id === saved.id ? saved : config))
        : [...configs, saved];
      onChanged(next, saved.id);
      load(saved);
      toast.success(t("extra.saved"));
    } catch (error) {
      toast.error(extractErrorMessage(error));
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    if (!editingId) return;
    setSaving(true);
    try {
      await api.deleteClaudeExtraConfig(editingId);
      const next = configs.filter((config) => config.id !== editingId);
      onChanged(next, selectedId === editingId ? null : undefined);
      load(next[0] ?? null);
    } catch (error) {
      toast.error(extractErrorMessage(error));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>{t("extra.title")}</DialogTitle>
          <DialogDescription>{t("extra.description")}</DialogDescription>
        </DialogHeader>

        <div className="grid min-h-72 grid-cols-[12rem_1fr] gap-3">
          <div className="space-y-1 rounded-lg border border-border/50 p-2">
            {selectableConfigs.map((config) => (
              <button
                type="button"
                key={config.id}
                onClick={() => load(config)}
                className={`w-full rounded px-2 py-1.5 text-left text-xs ${editingId === config.id ? "bg-primary/10 text-primary" : "hover:bg-secondary"}`}
              >
                {config.name}
              </button>
            ))}
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 w-full justify-start text-xs"
              onClick={() => load(null)}
            >
              <Plus className="mr-1 h-3 w-3" />
              {t("extra.new")}
            </Button>
          </div>

          <div className="space-y-3">
            <div className="space-y-1">
              <label className="text-[10px] font-medium text-muted-foreground">{t("extra.name")}</label>
              <Input value={name} onChange={(event) => setName(event.target.value)} className="h-8 text-xs" />
            </div>
            <div className="max-h-64 space-y-1.5 overflow-y-auto pr-1">
              {entries.map((entry, index) => (
                <div key={index} className="grid grid-cols-[1fr_8rem_1.75rem] gap-1.5">
                  <Input
                    value={entry.key}
                    onChange={(event) => setEntries((current) => current.map((item, itemIndex) => itemIndex === index ? { ...item, key: event.target.value } : item))}
                    placeholder={t("extra.key")}
                    className="h-8 font-mono text-xs"
                  />
                  <Input
                    value={entry.value}
                    onChange={(event) => setEntries((current) => current.map((item, itemIndex) => itemIndex === index ? { ...item, value: event.target.value } : item))}
                    placeholder={t("extra.value")}
                    className="h-8 font-mono text-xs"
                  />
                  <Button type="button" variant="ghost" size="icon" className="h-8 w-7" onClick={() => setEntries((current) => current.filter((_, itemIndex) => itemIndex !== index))}>
                    <Trash2 className="h-3 w-3" />
                  </Button>
                </div>
              ))}
            </div>
            <Button type="button" variant="outline" size="sm" className="h-7 text-xs" onClick={() => setEntries((current) => [...current, { key: "", value: "" }])}>
              <Plus className="mr-1 h-3 w-3" />
              {t("extra.addEntry")}
            </Button>
          </div>
        </div>

        <DialogFooter className="flex-row justify-between sm:justify-between">
          <Button
            type="button"
            variant="destructive"
            size="sm"
            disabled={!editingId || saving}
            onClick={remove}
          >
            {t("common.delete")}
          </Button>
          <div className="flex gap-2">
            <Button type="button" variant="outline" size="sm" onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
            <Button type="button" size="sm" disabled={saving} onClick={save}>{t("common.save")}</Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
