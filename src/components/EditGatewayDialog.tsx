import { useState, useEffect, useRef, useCallback } from "react";
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
  startDeviceLogin,
  pollDeviceLogin,
  fetchKeysWithToken,
  openUrl,
  updateGateway,
  applyConfig,
  type ApiKey,
  type DeviceCodeResponse,
  type GatewayWithHealth,
  type ApplyConfigParams,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { extractErrorMessage } from "@/lib/error";
import { Loader2, Check, Copy, ExternalLink, KeyRound } from "lucide-react";

interface EditGatewayDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  gateway: GatewayWithHealth;
  onUpdated: () => void;
}

type Step = "form" | "device" | "keys";

export function EditGatewayDialog({ open, onOpenChange, gateway, onUpdated }: EditGatewayDialogProps) {
  const { t } = useI18n();
  const [step, setStep] = useState<Step>("form");
  const [editName, setEditName] = useState(gateway.name);
  const [editUrl, setEditUrl] = useState(gateway.url);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  // Device code step
  const [deviceCode, setDeviceCode] = useState<DeviceCodeResponse | null>(null);
  const [copied, setCopied] = useState(false);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // Auth result
  const [userName, setUserName] = useState("");

  // Key selection
  const [keys, setKeys] = useState<ApiKey[]>([]);
  const [selectedKeyId, setSelectedKeyId] = useState<string | null>(null);
  const [loadingKeys, setLoadingKeys] = useState(false);

  // Reset when dialog opens/closes
  useEffect(() => {
    if (open) {
      setStep("form");
      setEditName(gateway.name);
      setEditUrl(gateway.url);
      setError("");
      setLoading(false);
      setDeviceCode(null);
      setCopied(false);
      setUserName("");
      setKeys([]);
      setSelectedKeyId(null);
    } else {
      if (pollRef.current) {
        clearInterval(pollRef.current);
        pollRef.current = null;
      }
    }
  }, [open, gateway]);

  useEffect(() => {
    return () => {
      if (pollRef.current) clearInterval(pollRef.current);
    };
  }, []);

  const handleSignIn = async () => {
    setError("");
    setLoading(true);
    try {
      const trimmedUrl = editUrl.replace(/\/+$/, "");
      setEditUrl(trimmedUrl);
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
            setUserName(result.userName || "");

            setStep("keys");
            setLoadingKeys(true);
            try {
              const fetchedKeys = await fetchKeysWithToken(gatewayUrl, result.sessionToken || "");
              setKeys(fetchedKeys);
              if (fetchedKeys.length === 1) {
                setSelectedKeyId(fetchedKeys[0].id);
              }
            } catch (err) {
              setError(`Failed to fetch keys: ${String(err)}`);
            } finally {
              setLoadingKeys(false);
            }
          } else if (result.status === "expired") {
            if (pollRef.current) {
              clearInterval(pollRef.current);
              pollRef.current = null;
            }
            setError("Code expired. Please try again.");
            setStep("form");
          }
        } catch {
          // Network error — keep polling
        }
      };

      pollRef.current = setInterval(poll, 5000);
    },
    []
  );

  const handleSave = async () => {
    const selected = keys.find((k) => k.id === selectedKeyId);
    if (!selected) return;

    setLoading(true);
    setError("");
    try {
      const trimmedUrl = editUrl.replace(/\/+$/, "");
      // Update gateway with new auth key, session token, and user info
      await updateGateway(gateway.id, editName, trimmedUrl, selected.key);

      // Also update session token and user info via a separate command if needed
      // For now, updateGateway handles name/url/authKey. We need to also store session info.
      // Let's apply config to set the active key
      const params: ApplyConfigParams = {
        gatewayId: gateway.id,
        keyId: selected.id,
        keyName: selected.name,
        keyValue: selected.key,
      };
      await applyConfig(params);

      onOpenChange(false);
      onUpdated();
    } catch (err) {
      setError(`Failed to save: ${extractErrorMessage(err)}`);
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

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[425px]" onPointerDownOutside={(e) => e.preventDefault()} onEscapeKeyDown={(e) => e.preventDefault()}>
        <DialogHeader>
          <DialogTitle>{t('editDialog.title')}</DialogTitle>
          <DialogDescription>
            {step === "form" && t('editDialog.description')}
            {step === "device" && t('addDialog.deviceStep')}
            {step === "keys" && t('addDialog.keysStep')}
          </DialogDescription>
        </DialogHeader>

        {/* Step 1: Edit form + Sign In */}
        {step === "form" && (
          <div>
            <div className="grid gap-4 py-4">
              <div className="grid gap-2">
                <Label>Name</Label>
                <Input
                  value={editName}
                  onChange={(e) => setEditName(e.target.value)}
                  placeholder="Gateway name"
                />
              </div>
              <div className="grid gap-2">
                <Label>{t('addDialog.gatewayUrl')}</Label>
                <Input
                  value={editUrl}
                  onChange={(e) => setEditUrl(e.target.value)}
                  placeholder={t('addDialog.urlPlaceholder')}
                />
              </div>
              {error && <p className="text-sm text-destructive">{error}</p>}
            </div>
            <DialogFooter>
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                {t('common.cancel')}
              </Button>
              <Button onClick={handleSignIn} disabled={loading || !editUrl}>
                {loading && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                {t('common.signIn')}
              </Button>
            </DialogFooter>
          </div>
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
                  try { await openUrl(`${editUrl}/device/login`); } catch {}
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
                  setStep("form");
                  setError("");
                }}
              >
                {t('common.back')}
              </Button>
            </DialogFooter>
          </div>
        )}

        {/* Step 3: Key Selection */}
        {step === "keys" && (
          <div className="py-4">
            {loadingKeys ? (
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
                <div>
                  <label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground mb-2 block">
                    {t('editDialog.selectKey')}
                  </label>
                  <div className="grid gap-1.5 max-h-[200px] overflow-y-auto">
                    {keys.map((key) => {
                      const isSelected = selectedKeyId === key.id;
                      return (
                        <button
                          key={key.id}
                          onClick={() => setSelectedKeyId(key.id)}
                          className={`flex items-center gap-3 p-3 rounded-lg border text-left transition-all ${
                            isSelected
                              ? "border-primary bg-primary/5 shadow-sm"
                              : "border-border/60 hover:border-primary/40 hover:bg-secondary/30"
                          }`}
                        >
                          <div className={`shrink-0 w-8 h-8 rounded-md flex items-center justify-center ${
                            isSelected ? "bg-primary/10" : "bg-secondary/50"
                          }`}>
                            <KeyRound className={`h-4 w-4 ${isSelected ? "text-primary" : "text-muted-foreground/50"}`} />
                          </div>
                          <div className="flex-1 min-w-0">
                            <div className={`font-medium text-sm ${isSelected ? "text-primary" : ""}`}>{key.name}</div>
                            <div className="text-[11px] text-muted-foreground font-mono">
                              {key.key.slice(0, 8)}…{key.key.slice(-4)}
                            </div>
                          </div>
                          {isSelected && <Check className="h-4 w-4 text-primary shrink-0" />}
                        </button>
                      );
                    })}
                  </div>
                </div>
              </div>
            )}
            {error && <p className="text-sm text-destructive mt-2">{error}</p>}
            <DialogFooter className="mt-4">
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                {t('common.cancel')}
              </Button>
              <Button onClick={handleSave} disabled={loading || !selectedKeyId}>
                {loading && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                {t('editDialog.updateGateway')}
              </Button>
            </DialogFooter>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
