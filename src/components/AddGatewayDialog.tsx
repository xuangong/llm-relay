import React, { useState, useEffect, useRef, useCallback } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
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
import { Loader2, Check, Copy, ExternalLink, KeyRound } from "lucide-react";

interface AddGatewayDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onAdded: () => void;
}

type Step = "url" | "device" | "config";

export function AddGatewayDialog({ open, onOpenChange, onAdded }: AddGatewayDialogProps) {
  const { t } = useI18n();
  const [step, setStep] = useState<Step>("url");
  const [url, setUrl] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  // Device code step
  const [deviceCode, setDeviceCode] = useState<DeviceCodeResponse | null>(null);
  const [copied, setCopied] = useState(false);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // Auth result
  const [sessionToken, setSessionToken] = useState("");
  const [userId, setUserId] = useState("");
  const [userName, setUserName] = useState("");

  // Key + model selection (config step)
  const [keys, setKeys] = useState<ApiKey[]>([]);
  const [selectedKeyId, setSelectedKeyId] = useState<string | null>(null);
  const [loadingConfig, setLoadingConfig] = useState(false);
  const [models, setModels] = useState<ModelList | null>(null);
  const [claudeModel, setClaudeModel] = useState("");
  const [claudeSmallModel, setClaudeSmallModel] = useState("");
  const [codexModel, setCodexModel] = useState("");
  const [geminiModel, setGeminiModel] = useState("");

  // Reset state when dialog closes
  useEffect(() => {
    if (!open) {
      setStep("url");
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
    }
  }, [open]);

  useEffect(() => {
    return () => {
      if (pollRef.current) clearInterval(pollRef.current);
    };
  }, []);

  const handleStartLogin = async (e: React.FormEvent) => {
    e.preventDefault();
    setError("");
    setLoading(true);

    try {
      const trimmedUrl = url.replace(/\/+$/, "");
      setUrl(trimmedUrl);

      const result = await startDeviceLogin(trimmedUrl);
      setDeviceCode(result);
      setStep("device");

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
            setSessionToken(result.sessionToken || "");
            setUserId(result.userId || "");
            setUserName(result.userName || "");

            // Move to config step: fetch keys + models
            setStep("config");
            setLoadingConfig(true);
            try {
              const fetchedKeys = await fetchKeysWithToken(gatewayUrl, result.sessionToken || "");
              setKeys(fetchedKeys);
              if (fetchedKeys.length > 0) {
                const firstKey = fetchedKeys[0];
                setSelectedKeyId(firstKey.id);
                // Fetch models using first key
                await loadModelsForKey(gatewayUrl, firstKey.key);
              }
            } catch (err) {
              setError(`Failed to fetch keys: ${String(err)}`);
            } finally {
              setLoadingConfig(false);
            }
          } else if (result.status === "expired") {
            if (pollRef.current) {
              clearInterval(pollRef.current);
              pollRef.current = null;
            }
            setError("Code expired. Please try again.");
            setStep("url");
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
      // Fetch models directly via HTTP since gateway isn't added yet
      const resp = await fetch(`${gatewayUrl}/v1/models`, {
        headers: { "x-api-key": keyValue },
      });
      if (resp.ok) {
        const modelsResult: ModelList = await resp.json();
        setModels(modelsResult);

        // Auto-suggest models
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

      // Apply config with key + models
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

      onOpenChange(false);
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

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[425px]" onPointerDownOutside={(e) => e.preventDefault()} onEscapeKeyDown={(e) => e.preventDefault()}>
        <DialogHeader>
          <DialogTitle>{t('addDialog.title')}</DialogTitle>
          <DialogDescription>
            {step === "url" && t('addDialog.urlStep')}
            {step === "device" && t('addDialog.deviceStep')}
            {step === "config" && t('addDialog.keysStep')}
          </DialogDescription>
        </DialogHeader>

        {/* Step 1: URL */}
        {step === "url" && (
          <form onSubmit={handleStartLogin}>
            <div className="grid gap-4 py-4">
              <div className="grid gap-2">
                <Label htmlFor="url">{t('addDialog.gatewayUrl')}</Label>
                <Input
                  id="url"
                  placeholder={t('addDialog.urlPlaceholder')}
                  value={url}
                  onChange={(e) => setUrl(e.target.value)}
                  required
                  autoFocus
                />
              </div>
              {error && <p className="text-sm text-destructive">{error}</p>}
            </div>
            <DialogFooter>
              <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
                {t('common.cancel')}
              </Button>
              <Button type="submit" disabled={loading || !url}>
                {loading && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                {t('common.signIn')}
              </Button>
            </DialogFooter>
          </form>
        )}

        {/* Step 2: Device Code */}
        {step === "device" && deviceCode && (
          <div className="py-4">
            <div className="flex flex-col items-center gap-4">
              <div className="flex items-center gap-2">
                <code className="text-3xl font-bold tracking-[0.2em] font-mono select-all px-4 py-2 bg-muted rounded-lg">
                  {deviceCode.userCode}
                </code>
                <Button variant="ghost" size="icon" onClick={copyCode} className="h-8 w-8">
                  {copied ? <Check className="h-4 w-4 text-green-500" /> : <Copy className="h-4 w-4" />}
                </Button>
              </div>
              <p className="text-sm text-muted-foreground text-center">
                {t('addDialog.codeCopied')}
              </p>
              <div className="flex items-center gap-2 text-sm text-muted-foreground">
                <Loader2 className="h-4 w-4 animate-spin" />
                {t('addDialog.waitingAuth')}
              </div>
              <Button
                variant="outline"
                size="sm"
                onClick={async () => {
                  try { await openUrl(`${url}/device/login`); } catch {}
                }}
              >
                <ExternalLink className="mr-2 h-3 w-3" />
                {t('addDialog.openGateway')}
              </Button>
            </div>
            {error && <p className="text-sm text-destructive mt-4 text-center">{error}</p>}
            <DialogFooter className="mt-6">
              <Button
                variant="outline"
                onClick={() => {
                  if (pollRef.current) {
                    clearInterval(pollRef.current);
                    pollRef.current = null;
                  }
                  setStep("url");
                  setError("");
                }}
              >
                {t('common.back')}
              </Button>
            </DialogFooter>
          </div>
        )}

        {/* Step 3: Key + Model Selection */}
        {step === "config" && (
          <div className="py-4">
            {loadingConfig ? (
              <div className="flex items-center justify-center gap-2 py-8 text-sm text-muted-foreground">
                <Loader2 className="h-4 w-4 animate-spin" />
                {t('addDialog.loadingKeys')}
              </div>
            ) : keys.length === 0 ? (
              <div className="text-center py-8">
                <KeyRound className="h-8 w-8 mx-auto text-muted-foreground/30 mb-3" />
                <p className="text-sm text-muted-foreground">{t('addDialog.noKeys')}</p>
                <p className="text-xs text-muted-foreground mt-1">{t('addDialog.noKeysHint')}</p>
              </div>
            ) : (
              <div className="space-y-4">
                {userName && (
                  <div className="flex items-center gap-2 px-3 py-2 rounded-lg bg-green-500/10 border border-green-500/20">
                    <Check className="h-3.5 w-3.5 text-green-500 shrink-0" />
                    <span className="text-xs text-green-700 dark:text-green-400">
                      {t('addDialog.signedInAs')} <span className="font-semibold">{userName}</span>
                    </span>
                  </div>
                )}

                {/* Key selector (dropdown) */}
                <div className="space-y-1.5">
                  <label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                    {t('gateway.apiKey')}
                  </label>
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
              </div>
            )}
            {error && <p className="text-sm text-destructive mt-2">{error}</p>}
            <DialogFooter className="mt-4">
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                {t('common.cancel')}
              </Button>
              <Button onClick={handleSave} disabled={loading || !selectedKeyId}>
                {loading && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                {t('addDialog.addGateway')}
              </Button>
            </DialogFooter>
          </div>
        )}
      </DialogContent>
    </Dialog>
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
