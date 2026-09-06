import { useState, useCallback, useEffect } from "react";
import {
  DndContext,
  closestCenter,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
  DragEndEvent,
} from "@dnd-kit/core";
import {
  arrayMove,
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { GatewayCard } from "./GatewayCard";
import type { ClaudeExtraConfig, GatewayWithHealth } from "@/lib/api";
import * as api from "@/lib/api";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { toast } from "sonner";
import { extractErrorMessage } from "@/lib/error";

interface GatewayListProps {
  gateways: GatewayWithHealth[];
  activeGatewayId: string | null;
  activeKeyId: string | null;
  activeKeyName: string | null;
  extraConfigs: ClaudeExtraConfig[];
  onExtraConfigsChanged: (configs: ClaudeExtraConfig[]) => void;
  managedClients: import("@/lib/api").ManagedClients;
  activeModels: {
    claude: string | null;
    claudeSubagent: string | null;
    claudeSmall: string | null;
    codex: string | null;
    codexSubagent: string | null;
    gemini: string | null;
  };
  onRefresh: () => void;
}

function SortableGatewayCard({
  gateway,
  isActive,
  activeKeyId,
  activeKeyName,
  extraConfigs,
  onExtraConfigsChanged,
  managedClients,
  activeModels,
  canPinTop,
  onPinTop,
  onSelect,
  onDelete,
  onApplied,
}: {
  gateway: GatewayWithHealth;
  isActive: boolean;
  activeKeyId: string | null;
  activeKeyName: string | null;
  extraConfigs: ClaudeExtraConfig[];
  onExtraConfigsChanged: (configs: ClaudeExtraConfig[]) => void;
  managedClients: import("@/lib/api").ManagedClients;
  activeModels: {
    claude: string | null;
    claudeSubagent: string | null;
    claudeSmall: string | null;
    codex: string | null;
    codexSubagent: string | null;
    gemini: string | null;
  };
  canPinTop: boolean;
  onPinTop: () => void;
  onSelect: () => void;
  onDelete: () => void;
  onApplied: () => void;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: gateway.id });

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.5 : 1,
    zIndex: isDragging ? 50 : ("auto" as const),
  };

  return (
    <div ref={setNodeRef} style={style} {...attributes}>
      <GatewayCard
        gateway={gateway}
        isActive={isActive}
        activeKeyId={activeKeyId}
        activeKeyName={activeKeyName}
        extraConfigs={extraConfigs}
        onExtraConfigsChanged={onExtraConfigsChanged}
        managedClients={managedClients}
        activeModels={activeModels}
        dragHandleProps={listeners}
        canPinTop={canPinTop}
        onPinTop={onPinTop}
        onSelect={onSelect}
        onDelete={onDelete}
        onApplied={onApplied}
      />
    </div>
  );
}

export function GatewayList({
  gateways: initialGateways,
  activeGatewayId,
  activeKeyId,
  activeKeyName,
  extraConfigs,
  onExtraConfigsChanged,
  managedClients,
  activeModels,
  onRefresh,
}: GatewayListProps) {
  const [gateways, setGateways] = useState(initialGateways);

  useEffect(() => {
    setGateways(initialGateways);
  }, [initialGateways]);

  useEffect(() => {
    const appWindow = getCurrentWebviewWindow();
    const unlisten1 = appWindow.listen<GatewayWithHealth[]>("health-updated", (event) => {
      setGateways(event.payload);
    });

    return () => {
      unlisten1.then((fn) => fn());
    };
  }, []);

  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: { distance: 8 },
    }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    })
  );

  const handleDragEnd = useCallback(
    async (event: DragEndEvent) => {
      const { active, over } = event;
      if (!over || active.id === over.id) return;

      const oldIndex = gateways.findIndex((g) => g.id === active.id);
      const newIndex = gateways.findIndex((g) => g.id === over.id);
      const newOrder = arrayMove(gateways, oldIndex, newIndex);
      setGateways(newOrder);

      try {
        await api.reorderGateways(newOrder.map((g) => g.id));
      } catch (err) {
        toast.error(`Reorder failed: ${extractErrorMessage(err)}`);
        setGateways(gateways);
      }
    },
    [gateways]
  );

  const handlePinTop = useCallback(
    async (id: string) => {
      const index = gateways.findIndex((g) => g.id === id);
      if (index <= 0) return;
      const previous = gateways;
      const newOrder = arrayMove(gateways, index, 0);
      setGateways(newOrder);

      try {
        await api.reorderGateways(newOrder.map((g) => g.id));
      } catch (err) {
        toast.error(`Pin failed: ${extractErrorMessage(err)}`);
        setGateways(previous);
      }
    },
    [gateways]
  );

  const handleDelete = useCallback(
    async (id: string) => {
      try {
        await api.deleteGateway(id);
        onRefresh();
      } catch (err) {
        toast.error(`Delete failed: ${extractErrorMessage(err)}`);
      }
    },
    [onRefresh]
  );

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      onDragEnd={handleDragEnd}
    >
      <SortableContext
        items={gateways.map((g) => g.id)}
        strategy={verticalListSortingStrategy}
      >
        <div className="space-y-2">
          {gateways.map((gw, i) => (
            <SortableGatewayCard
              key={gw.id}
              gateway={gw}
              isActive={activeGatewayId === gw.id}
              activeKeyId={activeKeyId}
              activeKeyName={activeKeyName}
              extraConfigs={extraConfigs}
              onExtraConfigsChanged={onExtraConfigsChanged}
              managedClients={managedClients}
              activeModels={activeModels}
              canPinTop={i > 0}
              onPinTop={() => handlePinTop(gw.id)}
              onSelect={() => {}}
              onDelete={() => handleDelete(gw.id)}
              onApplied={onRefresh}
            />
          ))}
        </div>
      </SortableContext>
    </DndContext>
  );
}
