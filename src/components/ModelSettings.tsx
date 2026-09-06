import { Button } from "@/components/ui/button";
import { ModelSelect } from "@/components/ModelSelect";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  DEFAULT_CLAUDE_EXTRA_CONFIG_ID,
  type ClaudeExtraConfig,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { allClaudeRolesUseClaudeFamily } from "@/lib/models";
import { Settings2 } from "lucide-react";
import { useEffect } from "react";

interface ModelSettingsProps {
  managedClients: import("@/lib/api").ManagedClients;
  claudeModel: string;
  onClaudeModelChange: (value: string) => void;
  claudeModels: string[];
  claudeSubagentModel: string;
  onClaudeSubagentModelChange: (value: string) => void;
  claudeSubagentModels: string[];
  claudeHaikuModel: string;
  onClaudeHaikuModelChange: (value: string) => void;
  claudeHaikuModels: string[];
  codexModel: string;
  onCodexModelChange: (value: string) => void;
  codexModels: string[];
  codexSubagentModel: string;
  onCodexSubagentModelChange: (value: string) => void;
  geminiModel: string;
  onGeminiModelChange: (value: string) => void;
  geminiModels: string[];
  extraConfigs: ClaudeExtraConfig[];
  extraConfigId: string | null;
  onExtraConfigChange: (value: string | null) => void;
  onManageExtraConfigs: () => void;
}

export function ModelSettings(props: ModelSettingsProps) {
  const { t } = useI18n();
  const noModelsText = t("gateway.noModels");
  const allClaude = allClaudeRolesUseClaudeFamily(
    props.claudeModel,
    props.claudeSubagentModel,
    props.claudeHaikuModel,
  );
  const selectableExtraConfigs = props.extraConfigs.filter(
    (config) =>
      config.id === DEFAULT_CLAUDE_EXTRA_CONFIG_ID ||
      allClaude,
  );
  const effectiveExtraConfigId = selectableExtraConfigs.some(
    (config) => config.id === props.extraConfigId,
  )
    ? props.extraConfigId ?? undefined
    : selectableExtraConfigs[0]?.id;

  useEffect(() => {
    if (
      props.managedClients.claude &&
      effectiveExtraConfigId &&
      effectiveExtraConfigId !== props.extraConfigId
    ) {
      props.onExtraConfigChange(effectiveExtraConfigId);
    }
  }, [props.managedClients.claude, effectiveExtraConfigId, props.extraConfigId, props.onExtraConfigChange]);

  return (
    <div className="space-y-2">
      {props.managedClients.codex && (
        <section className="w-full rounded-lg border border-border/50 bg-background/30 p-2.5">
          <div className="mb-2 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
            {t("models.codexRegion")}
          </div>
          <div className="grid grid-cols-2 gap-2">
            <ModelSelect label={t("models.codex")} value={props.codexModel} onChange={props.onCodexModelChange} models={props.codexModels} noModelsText={noModelsText} />
            <ModelSelect label={t("models.codexSubagent")} value={props.codexSubagentModel} onChange={props.onCodexSubagentModelChange} models={props.codexModels} noModelsText={noModelsText} />
          </div>
        </section>
      )}

      {props.managedClients.claude && (
      <section className="w-full rounded-lg border border-border/50 bg-background/30 p-2.5">
        <div className="mb-2 flex items-center justify-between gap-2">
          <span className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
            {t("models.claudeRegion")}
          </span>
          <div className="flex items-center gap-1.5">
            <Select
              value={effectiveExtraConfigId}
              onValueChange={props.onExtraConfigChange}
            >
              <SelectTrigger className="h-7 w-40 bg-background/50 text-xs">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {selectableExtraConfigs.map((config) => (
                  <SelectItem key={config.id} value={config.id} className="text-xs">
                    {config.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={props.onManageExtraConfigs}
              className="h-7 px-2 text-xs"
            >
              <Settings2 className="mr-1 h-3 w-3" />
              {t("extra.manage")}
            </Button>
          </div>
        </div>
        <div className="grid grid-cols-3 gap-2">
          <ModelSelect label={t("models.claude")} value={props.claudeModel} onChange={props.onClaudeModelChange} models={props.claudeModels} noModelsText={noModelsText} />
          <ModelSelect label={t("models.claudeSubagent")} value={props.claudeSubagentModel} onChange={props.onClaudeSubagentModelChange} models={props.claudeSubagentModels} noModelsText={noModelsText} />
          <ModelSelect label={t("models.claudeHaiku")} value={props.claudeHaikuModel} onChange={props.onClaudeHaikuModelChange} models={props.claudeHaikuModels} noModelsText={noModelsText} />
        </div>
      </section>
      )}

      {props.managedClients.gemini && (
      <section className="w-full rounded-lg border border-border/50 bg-background/30 p-2.5">
        <div className="mb-2 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
          {t("models.geminiRegion")}
        </div>
        <ModelSelect label={t("models.gemini")} value={props.geminiModel} onChange={props.onGeminiModelChange} models={props.geminiModels} noModelsText={noModelsText} />
      </section>
      )}
    </div>
  );
}
