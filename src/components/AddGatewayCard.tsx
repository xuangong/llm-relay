import { useState, useEffect, useRef, useCallback } from "react";
import { Card, CardContent } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { extractErrorMessage } from "@/lib/error";
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
}

type Phase = "idle" | "url" | "device" | "config";

export function AddGatewayCard({ onAdded }: AddGatewayCardProps) {
  const { t } = useI18n();
  const [phase, setPhase] = useState<Phase>("idle");
  const [url, setUrl] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  // Device code
  const [deviceCode, setDeviceCode] = useState<DeviceCodeResponse | null>(null);
  const [copied, setCopied] = useState(false);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // Auth result
  const [sessionToken, setSessionToken] = useState("");
  const [userId, setUserId] = useState("");
  const [userName, setUserName] = useState("");

  // Key + model selection
  const [keys, setKeys] = useState<ApiKey[]>([]);
  const [selectedKeyId, setSelectedKeyId] = useState<string | null>(null);
  const [models, setModels] = useState<ModelList | null>(null);
  const [claudeModel, setClaudeModel] = useState("");
  const [claudeSmallModel, setClaudeSmallModel] = useState("");
  const [codexModel, setCodexModel] = useState("");
  const [geminiModel, setGeminiModel] = useState("");

  useEffect(() => {
    return () => {
      if (pollRef.current) clearInterval(pollRef.current);
    };
  }, []);

  const reset = () => {
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
    setClaudeModel("");
    setClaudeSmallModel("");
    setCodexModel("");
    setGeminiModel("");
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

  const loadModelsForKey = async (gatewayUrl: string, keyValue: string) => {
    try {
      const resp = await fetch(`${gatewayUrl}/v1/models`, {
        headers: { "x-api-key": keyValue },
      });
      if (resp.ok) {
        const modelsResult: ModelList = await resp.json();
        setModels(modelsResult);

        if (modelsResult.data.length > 0) {
          const modelIds = modelsResult.data.map((m) => m.id).sort((a, b) => b.localeCompare(a));
          setClaudeModel(
            modelIds.find((id) => id.toLowerCase().includes("opus")) ||
            modelIds.find((id) => id.toLowerCase().includes("claude")) || ""
          );
          setClaudeSmallModel(
            modelIds.find((id) => id.toLowerCase().includes("haiku")) ||
            modelIds.find((id) => id.toLowerCase().includes("claude")) || ""
          );
          setCodexModel(
            modelIds.find((id) => /gpt-[5-9]/i.test(id)) ||
            modelIds.find((id) => /\bo[1-9]/i.test(id)) || ""
          );
          setGeminiModel(
            modelIds.find((id) => id.toLowerCase().includes("gemini")) || ""
          );
        }
      }
    } catch {
      // Models fetch failed — not critical
    }
  };

  const handleKeyChange = async (keyId: string) => {
    setSelectedKeyId(keyId);
    const key = keys.find((k) => k.id === keyId);
    if (key) {
      const trimmedUrl = url.replace(/\/+$/, "");
      await loadModelsForKey(trimmedUrl, key.key);
    }
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

      const allModelIds = models?.data.map((m) => m.id) || [];
      const cModels = allModelIds.filter((m) => m.toLowerCase().includes("claude") && !m.toLowerCase().includes("haiku"));
      const csModels = allModelIds.filter((m) => m.toLowerCase().includes("claude"));
      const xModels = allModelIds.filter((m) => { const l = m.toLowerCase(); return /gpt-[5-9]/.test(l) || /\bo[1-9]/.test(l); });
      const gModels = allModelIds.filter((m) => m.toLowerCase().includes("gemini"));

      await applyConfig({
        gatewayId: gw.id,
        keyId: selected.id,
        keyName: selected.name,
        keyValue: selected.key,
        claudeModel: cModels.length > 0 ? (claudeModel || undefined) : undefined,
        claudeSmallModel: csModels.length > 0 ? (claudeSmallModel || undefined) : undefined,
        codexModel: xModels.length > 0 ? (codexModel || undefined) : undefined,
        geminiModel: gModels.length > 0 ? (geminiModel || undefined) : undefined,
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
  const claudeModels = allModelIds.filter((m) =>
    m.toLowerCase().includes("claude") && !m.toLowerCase().includes("haiku")
  );
  const claudeSmallModels = allModelIds.filter((m) =>
    m.toLowerCase().includes("claude")
  );
  const codexModels = allModelIds.filter((m) => {
    const lower = m.toLowerCase();
    return /gpt-[5-9]/.test(lower) || /\bo[1-9]/.test(lower);
  });
  const geminiModels = allModelIds.filter((m) =>
    m.toLowerCase().includes("gemini")
  );

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
                <div className="grid grid-cols-2 gap-2">
                  <ModelSelect label={t('models.claude')} value={claudeModel} onChange={setClaudeModel} models={claudeModels} noModelsText={t('gateway.noModels')} />
                  <ModelSelect label={t('models.claudeSmall')} value={claudeSmallModel} onChange={setClaudeSmallModel} models={claudeSmallModels} noModelsText={t('gateway.noModels')} />
                  <ModelSelect label={t('models.codex')} value={codexModel} onChange={setCodexModel} models={codexModels} noModelsText={t('gateway.noModels')} />
                  <ModelSelect label={t('models.gemini')} value={geminiModel} onChange={setGeminiModel} models={geminiModels} noModelsText={t('gateway.noModels')} />
                </div>
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
                disabled={loading || !selectedKeyId}
                className="h-7 px-3 text-xs"
              >
                {loading ? <Loader2 className="h-3 w-3 mr-1 animate-spin" /> : <Check className="h-3 w-3 mr-1" />}
                {t('common.done')}
              </Button>
            </div>
          </>
        )}
      </CardContent>
    </Card>
  );
}

function ModelSelect({
  label,
  value,
  onChange,
  models,
  noModelsText,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  models: string[];
  noModelsText?: string;
}) {
  if (models.length === 0) {
    return (
      <div className="space-y-1">
        <label className="text-[10px] font-medium text-muted-foreground">{label}</label>
        <div className="h-7 flex items-center px-2 text-xs text-muted-foreground/40 italic">
          {noModelsText || "No models available"}
        </div>
      </div>
    );
  }
  return (
    <div className="space-y-1">
      <label className="text-[10px] font-medium text-muted-foreground">{label}</label>
      <Select value={value} onValueChange={onChange}>
        <SelectTrigger className="h-7 text-xs border-border/60 transition-elegant-fast bg-background/50">
          <SelectValue placeholder="—" />
        </SelectTrigger>
        <SelectContent className="border-border/60">
          {models.map((m) => (
            <SelectItem key={m} value={m} className="text-xs font-mono cursor-pointer">
              {m}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}
