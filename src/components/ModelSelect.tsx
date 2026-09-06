import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

interface ModelSelectProps {
  label: string;
  value: string;
  onChange: (value: string) => void;
  models: string[];
  noModelsText?: string;
}

export function ModelSelect({
  label,
  value,
  onChange,
  models,
  noModelsText,
}: ModelSelectProps) {
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
          {models.map((model) => (
            <SelectItem key={model} value={model} className="text-xs font-mono cursor-pointer">
              {model}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}
