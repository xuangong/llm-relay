import { useState, useEffect } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { Card, CardContent } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  GatewayWithHealth,
  ApiKey,
  ModelList,
  ApplyConfigParams,
} from "@/lib/api";
import * as api from "@/lib/api";
import {
  GripVertical,
  Trash2,
  Check,
  Loader2,
  ChevronDown,
  ChevronRight,
  Wifi,
  WifiOff,
} from "lucide-react";

interface GatewayCardProps {
  gateway: GatewayWithHealth;
  isActive: boolean;
  activeKeyId: string | null;
  activeModels: {
    claude: string | null;
    claudeSmall: string | null;
    codex: string | null;
    gemini: string | null;
  };
  dragHandleProps?: Record<string, unknown>;
  onSelect: () => void;
  onDelete: () => void;
  onApplied: () => void;
}

export function GatewayCard({
  gateway,
  isActive,
  activeKeyId,
  activeModels,
  dragHandleProps,
  onSelect,
  onDelete,
  onApplied,
}: GatewayCardProps) {
  const [expanded, setExpanded] = useState(isActive);
  const [keys, setKeys] = useState<ApiKey[]>([]);
  const [models, setModels] = useState<ModelList | null>(null);
  const [selectedKeyId, setSelectedKeyId] = useState<string | null>(activeKeyId);
  const [claudeModel, setClaudeModel] = useState(activeModels.claude || "");
  const [claudeSmallModel, setClaudeSmallModel] = useState(activeModels.claudeSmall || "");
  const [codexModel, setCodexModel] = useState(activeModels.codex || "");
  const [loading, setLoading] = useState(false);
  const [applying, setApplying] = useState(false);

  useEffect(() => {
    setExpanded(isActive);
  }, [isActive]);

  useEffect(() => {
    if (expanded && keys.length === 0) {
      loadKeysAndModels();
    }
  }, [expanded]);

  const loadKeysAndModels = async () => {
    setLoading(true);
    try {
      const [keysResult, modelsResult] = await Promise.all([
        api.fetchKeys(gateway.id),
        api.fetchModels(gateway.id),
      ]);
      setKeys(keysResult);
      setModels(modelsResult);

      // Auto-select first key if none selected
      if (!selectedKeyId && keysResult.length > 0) {
        setSelectedKeyId(keysResult[0].id);
      }

      // Auto-suggest models
      if (modelsResult.data.length > 0) {
        const modelIds = modelsResult.data.map((m) => m.id);
        if (!claudeModel) {
          const claude = modelIds.find((id) => id.includes("claude-sonnet-4")) ||
            modelIds.find((id) => id.includes("claude")) || "";
          setClaudeModel(claude);
        }
        if (!claudeSmallModel) {
          const small = modelIds.find((id) => id.includes("claude-haiku")) || "";
          setClaudeSmallModel(small);
        }
        if (!codexModel) {
          const codex = modelIds.find((id) => id.includes("gpt-4")) ||
            modelIds.find((id) => id.includes("o4-mini")) || "";
          setCodexModel(codex);
        }
      }
    } catch (err) {
      console.error("Failed to load keys/models:", err);
    } finally {
      setLoading(false);
    }
  };

  const handleApply = async () => {
    const selectedKey = keys.find((k) => k.id === selectedKeyId);
    setApplying(true);
    try {
      const params: ApplyConfigParams = {
        gatewayId: gateway.id,
        keyId: selectedKey?.id,
        keyName: selectedKey?.name,
        keyValue: selectedKey?.key,
        claudeModel: claudeModel || undefined,
        claudeSmallModel: claudeSmallModel || undefined,
        codexModel: codexModel || undefined,
      };
      await api.applyConfig(params);
      onApplied();
    } catch (err) {
      console.error("Failed to apply config:", err);
    } finally {
      setApplying(false);
    }
  };

  const handleToggle = () => {
    if (!isActive) {
      onSelect();
    }
    setExpanded(!expanded);
  };

  const allModels = models?.data.map((m) => m.id) || [];

  return (
    <Card
      className={`transition-all ${
        isActive ? "border-primary shadow-md" : "hover:border-muted-foreground/30"
      }`}
    >
      {/* Header */}
      <div
        className="flex items-center gap-3 p-4 cursor-pointer select-none"
        onClick={handleToggle}
      >
        <div {...dragHandleProps} className="cursor-grab active:cursor-grabbing text-muted-foreground">
          <GripVertical className="h-5 w-5" />
        </div>

        {/* Health indicator */}
        <div className="flex-shrink-0">
          {gateway.isHealthy ? (
            <Wifi className="h-4 w-4 text-green-500" />
          ) : (
            <WifiOff className="h-4 w-4 text-red-500" />
          )}
        </div>

        {/* Name + URL */}
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            <span className="font-medium truncate">{gateway.name}</span>
            {isActive && (
              <span className="text-xs bg-primary/10 text-primary px-2 py-0.5 rounded-full">
                active
              </span>
            )}
          </div>
          <div className="text-xs text-muted-foreground truncate">{gateway.url}</div>
        </div>

        {/* Latency + model count */}
        <div className="text-right text-xs text-muted-foreground flex-shrink-0">
          {gateway.isHealthy ? (
            <>
              <div>{gateway.latencyMs}ms</div>
              <div>{gateway.modelCount} models</div>
            </>
          ) : (
            <div className="text-red-500">offline</div>
          )}
        </div>

        {/* Expand icon */}
        <div className="text-muted-foreground">
          {expanded ? (
            <ChevronDown className="h-4 w-4" />
          ) : (
            <ChevronRight className="h-4 w-4" />
          )}
        </div>
      </div>

      {/* Expanded content */}
      <AnimatePresence>
        {expanded && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.2 }}
            style={{ overflow: "hidden" }}
          >
            <CardContent className="pt-0 pb-4 space-y-4">
              {loading ? (
                <div className="flex items-center justify-center py-4">
                  <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
                </div>
              ) : (
                <>
                  {/* Key selector */}
                  {keys.length > 0 && (
                    <div className="space-y-2">
                      <label className="text-sm font-medium">Key</label>
                      <div className="flex flex-wrap gap-2">
                        {keys.map((key) => (
                          <button
                            key={key.id}
                            onClick={() => setSelectedKeyId(key.id)}
                            className={`px-3 py-1.5 text-sm rounded-md border transition-colors ${
                              selectedKeyId === key.id
                                ? "border-primary bg-primary/10 text-primary"
                                : "border-input hover:border-muted-foreground/30"
                            }`}
                          >
                            {key.name}
                          </button>
                        ))}
                      </div>
                    </div>
                  )}

                  {/* Model selectors */}
                  {allModels.length > 0 && (
                    <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                      <ModelSelect
                        label="Claude Model"
                        value={claudeModel}
                        onChange={setClaudeModel}
                        models={allModels}
                      />
                      <ModelSelect
                        label="Claude Small"
                        value={claudeSmallModel}
                        onChange={setClaudeSmallModel}
                        models={allModels}
                      />
                      <ModelSelect
                        label="Codex Model"
                        value={codexModel}
                        onChange={setCodexModel}
                        models={allModels}
                      />
                    </div>
                  )}

                  {/* Action buttons */}
                  <div className="flex items-center justify-between pt-2">
                    <Button
                      variant="destructive"
                      size="sm"
                      onClick={(e) => {
                        e.stopPropagation();
                        onDelete();
                      }}
                    >
                      <Trash2 className="h-4 w-4 mr-1" />
                      Delete
                    </Button>

                    <Button
                      size="sm"
                      onClick={handleApply}
                      disabled={applying || !selectedKeyId}
                    >
                      {applying ? (
                        <Loader2 className="h-4 w-4 mr-1 animate-spin" />
                      ) : (
                        <Check className="h-4 w-4 mr-1" />
                      )}
                      Apply Config
                    </Button>
                  </div>
                </>
              )}
            </CardContent>
          </motion.div>
        )}
      </AnimatePresence>
    </Card>
  );
}

function ModelSelect({
  label,
  value,
  onChange,
  models,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  models: string[];
}) {
  return (
    <div className="space-y-1">
      <label className="text-xs text-muted-foreground">{label}</label>
      <Select value={value} onValueChange={onChange}>
        <SelectTrigger className="h-8 text-xs">
          <SelectValue placeholder="Select model" />
        </SelectTrigger>
        <SelectContent>
          {models.map((m) => (
            <SelectItem key={m} value={m} className="text-xs">
              {m}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}
