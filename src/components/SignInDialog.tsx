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
import {
  startDeviceLogin,
  pollDeviceLogin,
  openUrl,
  type DeviceCodeResponse,
} from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { Loader2, Check, Copy, ExternalLink } from "lucide-react";

interface SignInDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  gatewayUrl: string;
  onComplete: (sessionToken: string) => void;
}

export function SignInDialog({ open, onOpenChange, gatewayUrl, onComplete }: SignInDialogProps) {
  const { t } = useI18n();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [deviceCode, setDeviceCode] = useState<DeviceCodeResponse | null>(null);
  const [copied, setCopied] = useState(false);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // Auto-start login when dialog opens
  useEffect(() => {
    if (open) {
      setError("");
      setDeviceCode(null);
      setCopied(false);
      handleStart();
    } else {
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

  const handleStart = async () => {
    setError("");
    setLoading(true);
    try {
      const url = gatewayUrl.replace(/\/+$/, "");
      const result = await startDeviceLogin(url);
      setDeviceCode(result);

      try {
        await navigator.clipboard.writeText(result.userCode);
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      } catch {}

      try {
        await openUrl(`${url}/device/login`);
      } catch {}

      startPolling(url, result.deviceCode);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  };

  const startPolling = useCallback(
    (url: string, devCode: string) => {
      if (pollRef.current) clearInterval(pollRef.current);

      const poll = async () => {
        try {
          const result = await pollDeviceLogin(url, devCode);
          if (result.status === "complete") {
            if (pollRef.current) {
              clearInterval(pollRef.current);
              pollRef.current = null;
            }
            onComplete(result.sessionToken || "");
          } else if (result.status === "expired") {
            if (pollRef.current) {
              clearInterval(pollRef.current);
              pollRef.current = null;
            }
            setError("Code expired. Please try again.");
            setDeviceCode(null);
          }
        } catch {
          // Network error — keep polling
        }
      };

      pollRef.current = setInterval(poll, 5000);
    },
    [onComplete]
  );

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
      <DialogContent className="sm:max-w-[380px]" onPointerDownOutside={(e) => e.preventDefault()}>
        <DialogHeader>
          <DialogTitle>{t('common.signIn')}</DialogTitle>
          <DialogDescription>{t('addDialog.deviceStep')}</DialogDescription>
        </DialogHeader>

        <div className="py-4">
          {loading && !deviceCode ? (
            <div className="flex items-center justify-center gap-2 py-8 text-sm text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin" />
              {t('common.loading')}
            </div>
          ) : deviceCode ? (
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
                  try { await openUrl(`${gatewayUrl.replace(/\/+$/, "")}/device/login`); } catch {}
                }}
              >
                <ExternalLink className="mr-2 h-3 w-3" />
                {t('addDialog.openGateway')}
              </Button>
            </div>
          ) : error ? (
            <div className="text-center py-4">
              <p className="text-sm text-destructive mb-3">{error}</p>
              <Button variant="outline" size="sm" onClick={handleStart}>
                {t('common.retry')}
              </Button>
            </div>
          ) : null}
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {t('common.cancel')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
