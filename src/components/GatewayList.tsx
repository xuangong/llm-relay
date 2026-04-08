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
import type { GatewayWithHealth } from "@/lib/api";
import * as api from "@/lib/api";
import { listen } from "@tauri-apps/api/event";

interface GatewayListProps {
  gateways: GatewayWithHealth[];
  activeGatewayId: string | null;
  activeKeyId: string | null;
  activeModels: {
    claude: string | null;
    claudeSmall: string | null;
    codex: string | null;
    gemini: string | null;
  };
  onRefresh: () => void;
}

function SortableGatewayCard({
  gateway,
  isActive,
  activeKeyId,
  activeModels,
  onSelect,
  onDelete,
  onApplied,
}: {
  gateway: GatewayWithHealth;
  isActive: boolean;
  activeKeyId: string | null;
  activeModels: {
    claude: string | null;
    claudeSmall: string | null;
    codex: string | null;
    gemini: string | null;
  };
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
        activeModels={activeModels}
        dragHandleProps={listeners}
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
  activeModels,
  onRefresh,
}: GatewayListProps) {
  const [gateways, setGateways] = useState(initialGateways);

  useEffect(() => {
    setGateways(initialGateways);
  }, [initialGateways]);

  useEffect(() => {
    const unlisten1 = listen<GatewayWithHealth[]>("health-updated", (event) => {
      setGateways(event.payload);
    });

    const unlisten2 = listen("gateway-switched", () => {
      onRefresh();
    });

    return () => {
      unlisten1.then((fn) => fn());
      unlisten2.then((fn) => fn());
    };
  }, [onRefresh]);

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
        console.error("Failed to reorder:", err);
        setGateways(gateways);
      }
    },
    [gateways]
  );

  const handleDelete = async (id: string) => {
    try {
      await api.deleteGateway(id);
      onRefresh();
    } catch (err) {
      console.error("Failed to delete gateway:", err);
    }
  };

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
        <div className="space-y-3">
          {gateways.map((gw) => (
            <SortableGatewayCard
              key={gw.id}
              gateway={gw}
              isActive={activeGatewayId === gw.id}
              activeKeyId={activeKeyId}
              activeModels={activeModels}
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
