import { useEffect, useId, useRef, useState } from "react";
import * as Popover from "@radix-ui/react-popover";
import { Check, ChevronDown, Search } from "lucide-react";
import { useI18n } from "@/lib/i18n";
import { searchModels } from "@/lib/models";
import { cn } from "@/lib/utils";

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
  const { t } = useI18n();
  const id = useId();
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [activeModel, setActiveModel] = useState<string | null>(null);
  const filteredModels = searchModels(models, query);
  const activeIndex = Math.max(0, filteredModels.indexOf(activeModel ?? value));
  const activeId =
    filteredModels.length > 0 ? `${id}-option-${activeIndex}` : undefined;

  useEffect(() => {
    if (open) {
      listRef.current
        ?.querySelector('[data-active="true"]')
        ?.scrollIntoView({ block: "nearest" });
    }
  }, [open, activeId, query]);

  const handleOpenChange = (nextOpen: boolean) => {
    setOpen(nextOpen);
    setQuery("");
    setActiveModel(value || models[0] || null);
  };

  const selectModel = (model: string) => {
    onChange(model);
    handleOpenChange(false);
  };

  if (models.length === 0) {
    return (
      <div className="space-y-1">
        <label className="text-[10px] font-medium text-muted-foreground">
          {label}
        </label>
        <div className="h-7 flex items-center px-2 text-xs text-muted-foreground/40 italic">
          {noModelsText || "No models available"}
        </div>
      </div>
    );
  }

  return (
    <div className="min-w-0 space-y-1">
      <label
        htmlFor={`${id}-trigger`}
        className="text-[10px] font-medium text-muted-foreground"
      >
        {label}
      </label>
      <Popover.Root open={open} onOpenChange={handleOpenChange}>
        <Popover.Trigger asChild>
          <button
            id={`${id}-trigger`}
            type="button"
            aria-label={`${label}: ${value || "—"}`}
            title={value || undefined}
            onKeyDown={(event) => {
              if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                event.preventDefault();
                handleOpenChange(true);
              }
            }}
            className="flex h-7 w-full items-center justify-between gap-1 rounded-md border border-border/60 bg-background/50 px-2 text-xs transition-elegant-fast focus:outline-none focus:ring-2 focus:ring-ring"
          >
            <span className="truncate">{value || "—"}</span>
            <ChevronDown
              aria-hidden="true"
              className="h-3.5 w-3.5 shrink-0 opacity-50"
            />
          </button>
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Content
            align="start"
            sideOffset={4}
            collisionPadding={8}
            aria-label={label}
            onOpenAutoFocus={(event) => {
              event.preventDefault();
              inputRef.current?.focus();
            }}
            className="z-50 flex max-h-[var(--radix-popover-content-available-height)] w-[max(16rem,var(--radix-popover-trigger-width))] max-w-[calc(100vw-1rem)] flex-col overflow-hidden rounded-md border border-border/60 bg-popover text-popover-foreground shadow-md"
          >
            <div className="flex shrink-0 items-center gap-2 border-b border-border/60 px-2">
              <Search
                aria-hidden="true"
                className="h-3.5 w-3.5 shrink-0 text-muted-foreground"
              />
              <input
                ref={inputRef}
                role="combobox"
                aria-label={`${label}: ${t("models.search")}`}
                aria-expanded={open}
                aria-controls={`${id}-listbox`}
                aria-autocomplete="list"
                aria-activedescendant={activeId}
                autoComplete="off"
                spellCheck={false}
                placeholder={t("models.search")}
                value={query}
                onChange={(event) => {
                  setQuery(event.target.value);
                  setActiveModel(null);
                }}
                onKeyDown={(event) => {
                  if (event.nativeEvent.isComposing) return;
                  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                    event.preventDefault();
                    if (filteredModels.length > 0) {
                      const offset = event.key === "ArrowDown" ? 1 : -1;
                      const index =
                        (activeIndex + offset + filteredModels.length) %
                        filteredModels.length;
                      setActiveModel(filteredModels[index]);
                    }
                  } else if (event.key === "Enter") {
                    event.preventDefault();
                    if (filteredModels[activeIndex])
                      selectModel(filteredModels[activeIndex]);
                  }
                }}
                className="h-8 min-w-0 flex-1 bg-transparent text-xs outline-none placeholder:text-muted-foreground"
              />
            </div>
            <div
              ref={listRef}
              id={`${id}-listbox`}
              role="listbox"
              aria-label={label}
              className="min-h-0 max-h-60 overflow-y-auto overscroll-contain p-1"
            >
              {filteredModels.map((model, index) => (
                <div
                  id={`${id}-option-${index}`}
                  key={model}
                  role="option"
                  aria-selected={model === value}
                  data-active={index === activeIndex}
                  onPointerMove={() => setActiveModel(model)}
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => selectModel(model)}
                  className={cn(
                    "flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 font-mono text-xs",
                    index === activeIndex && "bg-accent text-accent-foreground",
                  )}
                >
                  <Check
                    aria-hidden="true"
                    className={cn(
                      "h-3.5 w-3.5 shrink-0",
                      model !== value && "invisible",
                    )}
                  />
                  <span className="min-w-0 break-all">{model}</span>
                </div>
              ))}
            </div>
            {filteredModels.length === 0 && (
              <p
                role="status"
                className="px-3 py-3 text-xs text-muted-foreground"
              >
                {t("models.noMatches")}
              </p>
            )}
          </Popover.Content>
        </Popover.Portal>
      </Popover.Root>
    </div>
  );
}
