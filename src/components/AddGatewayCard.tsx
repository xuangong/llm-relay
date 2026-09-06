import { useState, useEffect, useRef, useCallback } from "react";
import { Card, CardContent } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ClaudeExtraConfigDialog } from "@/components/ClaudeExtraConfigDialog";
import { ModelSettings } from "@/components/ModelSettings";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  addGateway,
  applyConfig,
  startDeviceLogin,
  pollDeviceLogin,
  fetchKeysWithToken,
  openUrl,
  type ApiKey,
  type DeviceCodeResponse,
  type ModelList,
  type ClaudeExtraConfig,
  DEFAULT_CLAUDE_EXTRA_CONFIG_ID,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { extractErrorMessage } from "@/lib/error";
import {
  claudeRoleModels,
  codexModels as getCodexModels,
  geminiModels as getGeminiModels,
  preferredClaudeCodeModel,
  preferredCodexModel,
  preferredCodexSubagentModel,
  preferredGeminiModel,
  reconcileModelSelection,
} from "@/lib/models";
import {
  Loader2,
  Check,
  Copy,
  ExternalLink,
  X,
  Plus,
} from "lucide-react";

interface AddGatewayCardProps {
  onAdded: () => void;
  extraConfigs: ClaudeExtraConfig[];
  onExtraConfigsChanged: (configs: ClaudeExtraConfig[]) => void;
  managedClients: import("@/lib/api").ManagedClients;
}

type Phase = "idle" | "url" | "device" | "config";

export function AddGatewayCard({
  onAdded,
  extraConfigs,
  onExtraConfigsChanged,
  managedClients,
}: AddGatewayCardProps) {
  const { t } = useI18n();
  const [phase, setPhase] = useState<Phase>("idle");
  const [url, setUrl] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  // Device code
  const [deviceCode, setDeviceCode] = useState<DeviceCodeResponse | null>(null);
  const [copied, setCopied] = useState(false);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const modelRequestRef = useRef(0);
  const extraDefaultInitializedRef = useRef(false);

  // Auth result
  const [sessionToken, setSessionToken] = useState("");
  const [userId, setUserId] = useState("");
  const [userName, setUserName] = useState("");

  // Key + model selection
  const [keys, setKeys] = useState<ApiKey[]>([]);
  const [selectedKeyId, setSelectedKeyId] = useState<string | null>(null);
  const [models, setModels] = useState<ModelList | null>(null);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [claudeModel, setClaudeModel] = useState("");
  const [claudeSubagentModel, setClaudeSubagentModel] = useState("");
  const [claudeSmallModel, setClaudeSmallModel] = useState("");
  const [codexModel, setCodexModel] = useState("");
  const [codexSubagentModel, setCodexSubagentModel] = useState("");
  const [geminiModel, setGeminiModel] = useState("");
  const [claudeExtraConfigId, setClaudeExtraConfigId] = useState<string | null>(null);
  const [extraDialogOpen, setExtraDialogOpen] = useState(false);

  useEffect(() => {
    return () => {
      if (pollRef.current) clearInterval(pollRef.current);
    };
  }, []);

  useEffect(() => {
    if (!extraDefaultInitializedRef.current && extraConfigs.length > 0) {
      setClaudeExtraConfigId(
        extraConfigs.find((config) => config.id === DEFAULT_CLAUDE_EXTRA_CONFIG_ID)?.id ?? null,
      );
      extraDefaultInitializedRef.current = true;
    }
  }, [extraConfigs]);

  const reset = () => {
    modelRequestRef.current += 1;
    setPhase("idle");
    setUrl("");
    setError("");
    setLoading(false);
    setDeviceCode(null);
    setCopied(false);
    setSessionToken("");
    setUserId("");
    setUserName("");
    setKeys([]);
    setSelectedKeyId(null);
    setModels(null);
    setModelsLoading(false);
    setClaudeModel("");
    setClaudeSubagentModel("");
    setClaudeSmallModel("");
    setCodexModel("");
    setCodexSubagentModel("");
    setGeminiModel("");
    setClaudeExtraConfigId(
      extraConfigs.find((config) => config.id === DEFAULT_CLAUDE_EXTRA_CONFIG_ID)?.id ?? null,
    );
    extraDefaultInitializedRef.current = extraConfigs.length > 0;
    if (pollRef.current) {
      clearInterval(pollRef.current);
      pollRef.current = null;
    }
  };

  const handleStartLogin = async () => {
    setError("");
    setLoading(true);
    try {
      const trimmedUrl = url.replace(/\/+$/, "");
      setUrl(trimmedUrl);

      const result = await startDeviceLogin(trimmedUrl);
      setDeviceCode(result);
      setPhase("device");

      try {
        await navigator.clipboard.writeText(result.userCode);
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      } catch {}

      try {
        await openUrl(`${trimmedUrl}/device/login`);
      } catch {}

      startPolling(trimmedUrl, result.deviceCode);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  };

  const startPolling = useCallback(
    (gatewayUrl: string, devCode: string) => {
      if (pollRef.current) clearInterval(pollRef.current);

      const poll = async () => {
        try {
          const result = await pollDeviceLogin(gatewayUrl, devCode);
          if (result.status === "complete") {
            if (pollRef.current) {
              clearInterval(pollRef.current);
              pollRef.current = null;
            }
            const token = result.sessionToken || "";
            setSessionToken(token);
            setUserId(result.userId || "");
            setUserName(result.userName || "");
            setPhase("config");

            // Fetch keys + models
            try {
              const fetchedKeys = await fetchKeysWithToken(gatewayUrl, token);
              setKeys(fetchedKeys);
              if (fetchedKeys.length > 0) {
                const firstKey = fetchedKeys[0];
                setSelectedKeyId(firstKey.id);
                await loadModelsForKey(gatewayUrl, firstKey.key);
              }
            } catch (err) {
              setError(`Failed to fetch keys: ${String(err)}`);
            }
          } else if (result.status === "expired") {
            if (pollRef.current) {
              clearInterval(pollRef.current);
              pollRef.current = null;
            }
            setError("Code expired. Please try again.");
            setPhase("url");
          }
        } catch {
          // Network error — keep polling
        }
      };

      pollRef.current = setInterval(poll, 5000);
    },
    []
  );

  const applyModelCatalog = (modelsResult: ModelList | null) => {
    setModels(modelsResult);
    const modelIds = modelsResult?.data.map((model) => model.id) ?? [];
    const claudeMainCandidates = claudeRoleModels(modelIds, "main");
    const claudeSubagentCandidates = claudeRoleModels(modelIds, "subagent");
    const claudeHaikuCandidates = claudeRoleModels(modelIds, "haiku");
    const codexCandidates = getCodexModels(modelIds);
    const geminiCandidates = getGeminiModels(modelIds);

    if (managedClients.claude) {
      setClaudeModel((current) =>
        reconcileModelSelection(current, claudeMainCandidates, preferredClaudeCodeModel(modelIds, "main")),
      );
      setClaudeSubagentModel((current) =>
        reconcileModelSelection(current, claudeSubagentCandidates, preferredClaudeCodeModel(modelIds, "subagent")),
      );
      setClaudeSmallModel((current) =>
        reconcileModelSelection(current, claudeHaikuCandidates, preferredClaudeCodeModel(modelIds, "haiku")),
      );
    }
    if (managedClients.codex) {
      const preferredCodex = preferredCodexModel(modelIds);
      setCodexModel((current) =>
        reconcileModelSelection(current, codexCandidates, preferredCodex),
      );
      setCodexSubagentModel((current) =>
        reconcileModelSelection(
          current,
          codexCandidates,
          preferredCodexSubagentModel(
            modelIds,
            codexCandidates.includes(codexModel) ? codexModel : preferredCodex,
          ),
        ),
      );
    }
    if (managedClients.gemini) {
      setGeminiModel((current) =>
        reconcileModelSelection(current, geminiCandidates, preferredGeminiModel(modelIds)),
      );
    }
  };

  const loadModelsForKey = async (gatewayUrl: string, keyValue: string) => {
    const requestId = ++modelRequestRef.current;
    applyModelCatalog(null);
    setModelsLoading(true);
    try {
      const resp = await fetch(`${gatewayUrl}/v1/models`, {
        headers: { "x-api-key": keyValue },
      });
      if (!resp.ok) return;
      const modelsResult: ModelList = await resp.json();
      if (requestId !== modelRequestRef.current) return;
      applyModelCatalog(modelsResult);
    } catch {
      // The catalog was cleared before the request to avoid stale key/model pairs.
    } finally {
      if (requestId === modelRequestRef.current) setModelsLoading(false);
    }
  };

  const handleKeyChange = async (keyId: string) => {
    setSelectedKeyId(keyId);
    const key = keys.find((candidate) => candidate.id === keyId);
    if (!key) {
      modelRequestRef.current += 1;
      applyModelCatalog(null);
      setModelsLoading(false);
      return;
    }
    const trimmedUrl = url.replace(/\/+$/, "");
    await loadModelsForKey(trimmedUrl, key.key);
  };

  const handleSave = async () => {
    const selected = keys.find((k) => k.id === selectedKeyId);
    if (!selected) return;

    setLoading(true);
    setError("");
    try {
      const trimmedUrl = url.replace(/\/+$/, "");
      const gw = await addGateway({
        name: userName || trimmedUrl,
        url: trimmedUrl,
        authKey: selected.key,
        sessionToken,
        userId,
        userName,
      });

      await applyConfig({
        gatewayId: gw.id,
        keyId: selected.id,
        keyName: selected.name,
        keyValue: selected.key,
        claudeModel: managedClients.claude && claudeMainModels.includes(claudeModel) ? claudeModel : undefined,
        claudeSubagentModel: managedClients.claude && claudeSubagentModels.includes(claudeSubagentModel) ? claudeSubagentModel : undefined,
        claudeSmallModel: managedClients.claude && claudeHaikuModels.includes(claudeSmallModel) ? claudeSmallModel : undefined,
        codexModel: managedClients.codex && codexModels.includes(codexModel) ? codexModel : undefined,
        codexSubagentModel: managedClients.codex && codexModels.includes(codexSubagentModel) ? codexSubagentModel : undefined,
        geminiModel: managedClients.gemini && geminiModels.includes(geminiModel) ? geminiModel : undefined,
        claudeExtraConfigId: managedClients.claude ? claudeExtraConfigId ?? undefined : undefined,
        claudeExtraConfigSet: managedClients.claude,
      });

      reset();
      onAdded();
    } catch (err) {
      setError(`Failed to add: ${extractErrorMessage(err)}`);
    } finally {
      setLoading(false);
    }
  };

  const copyCode = async () => {
    if (!deviceCode) return;
    try {
      await navigator.clipboard.writeText(deviceCode.userCode);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {}
  };

  // Model filtering
  const allModelIds = models?.data.map((m) => m.id) || [];
  const claudeMainModels = claudeRoleModels(allModelIds, "main");
  const claudeSubagentModels = claudeRoleModels(allModelIds, "subagent");
  const claudeHaikuModels = claudeRoleModels(allModelIds, "haiku");
  const codexModels = getCodexModels(allModelIds);
  const geminiModels = getGeminiModels(allModelIds);

  // Idle state: just a button
  if (phase === "idle") {
    return (
      <Button
        variant="outline"
        className="w-full border-dashed hover:border-primary/50 hover:bg-primary/5 transition-elegant h-9"
        onClick={() => setPhase("url")}
      >
        <Plus className="h-3.5 w-3.5 mr-1.5" />
        <span className="text-sm font-medium">{t('addDialog.title')}</span>
      </Button>
    );
  }

  return (
    <Card className="border-dashed border-primary/30 bg-primary/[0.02]">
      <CardContent className="px-4 pt-3 pb-3 space-y-3">
        {/* URL input phase */}
        {phase === "url" && (
          <>
            <div className="space-y-1.5">
              <label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                {t('addDialog.gatewayUrl')}
              </label>
              <form
                className="flex gap-2"
                onSubmit={(e) => { e.preventDefault(); handleStartLogin(); }}
              >
                <Input
                  placeholder={t('addDialog.urlPlaceholder')}
                  value={url}
                  onChange={(e) => setUrl(e.target.value)}
                  className="h-8 text-xs flex-1"
                  autoFocus
                />
                <Button type="submit" size="sm" disabled={loading || !url} className="h-8 px-3 text-xs">
                  {loading ? <Loader2 className="h-3 w-3 animate-spin" /> : t('common.signIn')}
                </Button>
              </form>
            </div>
            {error && <p className="text-xs text-destructive">{error}</p>}
            <div className="flex items-center pt-1">
              <Button
                variant="ghost"
                size="sm"
                onClick={reset}
                className="h-7 px-2 text-xs text-muted-foreground hover:text-foreground"
              >
                <X className="h-3 w-3 mr-1" />
                {t('common.cancel')}
              </Button>
            </div>
          </>
        )}

        {/* Device code phase */}
        {phase === "device" && deviceCode && (
          <>
            <div className="flex flex-col items-center gap-3 py-2">
              <div className="flex items-center gap-2">
                <code className="text-2xl font-bold tracking-[0.2em] font-mono select-all px-3 py-1.5 bg-muted rounded-lg">
                  {deviceCode.userCode}
                </code>
                <Button variant="ghost" size="icon" onClick={copyCode} className="h-7 w-7">
                  {copied ? <Check className="h-3.5 w-3.5 text-green-500" /> : <Copy className="h-3.5 w-3.5" />}
                </Button>
              </div>
              <p className="text-xs text-muted-foreground text-center">
                {t('addDialog.codeCopied')}
              </p>
              <div className="flex items-center gap-2 text-xs text-muted-foreground">
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
                {t('addDialog.waitingAuth')}
              </div>
              <Button
                variant="outline"
                size="sm"
                className="h-7 text-xs"
                onClick={async () => {
                  try { await openUrl(`${url.replace(/\/+$/, "")}/device/login`); } catch {}
                }}
              >
                <ExternalLink className="mr-1.5 h-3 w-3" />
                {t('addDialog.openGateway')}
              </Button>
            </div>
            {error && <p className="text-xs text-destructive text-center">{error}</p>}
            <div className="flex items-center pt-1">
              <Button
                variant="ghost"
                size="sm"
                onClick={() => {
                  if (pollRef.current) {
                    clearInterval(pollRef.current);
                    pollRef.current = null;
                  }
                  reset();
                }}
                className="h-7 px-2 text-xs text-muted-foreground hover:text-foreground"
              >
                <X className="h-3 w-3 mr-1" />
                {t('common.cancel')}
              </Button>
            </div>
          </>
        )}

        {/* Config phase — same layout as GatewayCard edit mode */}
        {phase === "config" && (
          <>
            {/* Header line showing gateway info */}
            <div className="flex items-center gap-2 text-xs">
              <span className="font-medium">{userName || url}</span>
              <span className="text-muted-foreground font-mono truncate">{url}</span>
            </div>

            {/* API Key selector */}
            <div className="space-y-1.5">
              <label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                {t('gateway.apiKey')}
              </label>
              {keys.length > 0 ? (
                <Select value={selectedKeyId || ""} onValueChange={handleKeyChange}>
                  <SelectTrigger className="h-8 text-xs border-border/60 bg-background/50">
                    <SelectValue placeholder="—" />
                  </SelectTrigger>
                  <SelectContent className="border-border/60">
                    {keys.map((key) => (
                      <SelectItem key={key.id} value={key.id} className="text-xs cursor-pointer">
                        <div className="flex items-center gap-2">
                          <span className="font-medium">{key.name}</span>
                          {key.ownerName && <span className="text-muted-foreground">@{key.ownerName}</span>}
                          <span className="text-muted-foreground font-mono">{key.key.slice(0, 6)}…{key.key.slice(-3)}</span>
                        </div>
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              ) : (
                <p className="text-[10px] text-muted-foreground">{t('gateway.noKeys')}</p>
              )}
            </div>

            {/* Model selectors */}
            {allModelIds.length > 0 && (
              <div className="space-y-1.5">
                <label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                  {t('gateway.models')}
                </label>
                <ModelSettings
                  managedClients={managedClients}
                  claudeModel={claudeModel}
                  onClaudeModelChange={setClaudeModel}
                  claudeModels={claudeMainModels}
                  claudeSubagentModel={claudeSubagentModel}
                  onClaudeSubagentModelChange={setClaudeSubagentModel}
                  claudeSubagentModels={claudeSubagentModels}
                  claudeHaikuModel={claudeSmallModel}
                  onClaudeHaikuModelChange={setClaudeSmallModel}
                  claudeHaikuModels={claudeHaikuModels}
                  codexModel={codexModel}
                  onCodexModelChange={setCodexModel}
                  codexModels={codexModels}
                  codexSubagentModel={codexSubagentModel}
                  onCodexSubagentModelChange={setCodexSubagentModel}
                  geminiModel={geminiModel}
                  onGeminiModelChange={setGeminiModel}
                  geminiModels={geminiModels}
                  extraConfigs={extraConfigs}
                  extraConfigId={claudeExtraConfigId}
                  onExtraConfigChange={setClaudeExtraConfigId}
                  onManageExtraConfigs={() => setExtraDialogOpen(true)}
                />
              </div>
            )}

            {error && <p className="text-xs text-destructive">{error}</p>}

            {/* Action buttons */}
            <div className="flex items-center justify-between pt-2 border-t border-border/30">
              <Button
                variant="ghost"
                size="sm"
                onClick={reset}
                disabled={loading}
                className="h-7 px-2 text-xs text-muted-foreground hover:text-foreground"
              >
                <X className="h-3 w-3 mr-1" />
                {t('common.cancel')}
              </Button>
              <Button
                size="sm"
                onClick={handleSave}
                disabled={loading || modelsLoading || !selectedKeyId}
                className="h-7 px-3 text-xs"
              >
                {loading ? <Loader2 className="h-3 w-3 mr-1 animate-spin" /> : <Check className="h-3 w-3 mr-1" />}
                {t('common.done')}
              </Button>
            </div>
          </>
        )}
      </CardContent>
      <ClaudeExtraConfigDialog
        open={extraDialogOpen}
        onOpenChange={setExtraDialogOpen}
        configs={extraConfigs}
        selectedId={claudeExtraConfigId}
        onChanged={(configs, selectedId) => {
          onExtraConfigsChanged(configs);
          if (selectedId !== undefined) setClaudeExtraConfigId(selectedId);
        }}
      />
    </Card>
  );
}
