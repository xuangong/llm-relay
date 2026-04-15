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
  addGateway,
  startDeviceLogin,
  pollDeviceLogin,
  fetchKeysWithToken,
  openUrl,
  type ApiKey,
  type DeviceCodeResponse,
} from "@/lib/api";
import { Loader2, Check, Copy, ExternalLink, KeyRound } from "lucide-react";

interface AddGatewayDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onAdded: () => void;
}

type Step = "url" | "device" | "keys";

export function AddGatewayDialog({ open, onOpenChange, onAdded }: AddGatewayDialogProps) {
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

  // Key selection step
  const [keys, setKeys] = useState<ApiKey[]>([]);
  const [selectedKeyId, setSelectedKeyId] = useState<string | null>(null);
  const [loadingKeys, setLoadingKeys] = useState(false);

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
      if (pollRef.current) {
        clearInterval(pollRef.current);
        pollRef.current = null;
      }
    }
  }, [open]);

  // Cleanup polling on unmount
  useEffect(() => {
    return () => {
      if (pollRef.current) {
        clearInterval(pollRef.current);
      }
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

      // Copy code to clipboard
      try {
        await navigator.clipboard.writeText(result.userCode);
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      } catch {}

      // Open browser to gateway's device login page
      try {
        await openUrl(`${trimmedUrl}/device/login`);
      } catch {}

      // Start polling
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

            // Fetch keys using the session token
            setStep("keys");
            setLoadingKeys(true);
            try {
              // Use the session token to fetch keys directly
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

  const handleSave = async () => {
    const selected = keys.find((k) => k.id === selectedKeyId);
    if (!selected) return;

    setLoading(true);
    setError("");
    try {
      const trimmedUrl = url.replace(/\/+$/, "");
      await addGateway({
        name: userName || trimmedUrl,
        url: trimmedUrl,
        authKey: selected.key,
        sessionToken,
        userId,
        userName,
      });
      onOpenChange(false);
      onAdded();
    } catch (err) {
      setError(String(err));
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
          <DialogTitle>Add Gateway</DialogTitle>
          <DialogDescription>
            {step === "url" && "Enter the gateway URL to sign in."}
            {step === "device" && "Enter the code on the gateway website to authorize this app."}
            {step === "keys" && "Select the API key to use for this gateway."}
          </DialogDescription>
        </DialogHeader>

        {/* Step 1: URL */}
        {step === "url" && (
          <form onSubmit={handleStartLogin}>
            <div className="grid gap-4 py-4">
              <div className="grid gap-2">
                <Label htmlFor="url">Gateway URL</Label>
                <Input
                  id="url"
                  placeholder="https://gateway.example.com"
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
                Cancel
              </Button>
              <Button type="submit" disabled={loading || !url}>
                {loading && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                Sign In
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
                Code copied to clipboard. Enter it on the gateway website.
              </p>
              <div className="flex items-center gap-2 text-sm text-muted-foreground">
                <Loader2 className="h-4 w-4 animate-spin" />
                Waiting for authorization...
              </div>
              <Button
                variant="outline"
                size="sm"
                onClick={async () => {
                  try {
                    await openUrl(`${url}/device/login`);
                  } catch {}
                }}
              >
                <ExternalLink className="mr-2 h-3 w-3" />
                Open Gateway
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
                Back
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
                Loading keys...
              </div>
            ) : keys.length === 0 ? (
              <div className="text-center py-8">
                <KeyRound className="h-8 w-8 mx-auto text-muted-foreground/30 mb-3" />
                <p className="text-sm text-muted-foreground">No API keys found.</p>
                <p className="text-xs text-muted-foreground mt-1">
                  Create an API key on the gateway dashboard first.
                </p>
              </div>
            ) : (
              <div className="space-y-4">
                {userName && (
                  <div className="flex items-center gap-2 px-3 py-2 rounded-lg bg-green-500/10 border border-green-500/20">
                    <Check className="h-3.5 w-3.5 text-green-500 shrink-0" />
                    <span className="text-xs text-green-700 dark:text-green-400">
                      Signed in as <span className="font-semibold">{userName}</span>
                    </span>
                  </div>
                )}
                <div>
                  <label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground mb-2 block">
                    Select an API Key
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
                          {isSelected && (
                            <Check className="h-4 w-4 text-primary shrink-0" />
                          )}
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
                Cancel
              </Button>
              <Button onClick={handleSave} disabled={loading || !selectedKeyId}>
                {loading && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                Add Gateway
              </Button>
            </DialogFooter>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
