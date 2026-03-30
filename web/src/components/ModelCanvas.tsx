import {
  Background,
  BackgroundVariant,
  BaseEdge,
  Controls,
  EdgeLabelRenderer,
  type EdgeProps,
  getBezierPath,
  Handle,
  type NodeProps,
  Position,
  ReactFlow,
  useReactFlow,
} from "@xyflow/react";
import { memo, useCallback, useEffect, useMemo, useState } from "react";
import "@xyflow/react/dist/style.css";
import { irToCanvas, LANE_HEADER_W } from "../lib/irToCanvas";
import { useStore } from "../store";

// ── Compartment Node ─────────────────────────────────────────────────────────

const CompartmentNode = memo(({ data, id, selected }: NodeProps) => {
  const d = data as { label: string; subLabel?: string; color: string };
  const selectNode = useStore((s) => s.selectNode);

  const handleStyle = { background: d.color, border: "none", width: 8, height: 8 };
  const hiddenStyle = { opacity: 0, width: 6, height: 6, border: "none" };

  return (
    <>
      <Handle type="target" id="left" position={Position.Left} style={handleStyle} />
      <Handle type="target" id="top" position={Position.Top} style={hiddenStyle} />
      <div
        onClick={() => selectNode(id)}
        className="cursor-pointer"
        style={{
          width: 110,
          height: 70,
          borderRadius: 12,
          border: `2px solid ${selected ? "rgb(var(--accent-rgb))" : d.color}`,
          background: selected ? `${d.color}22` : "var(--canvas-node-bg)",
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          gap: 3,
          transition: "all 0.15s",
          boxShadow: selected ? `0 0 12px ${d.color}55` : "none",
        }}
      >
        <span
          style={{
            color: d.color,
            fontWeight: 700,
            fontSize: 22,
            fontFamily: "JetBrains Mono, monospace",
            lineHeight: 1,
          }}
        >
          {d.label}
        </span>
        {d.subLabel && (
          <span
            style={{
              color: "var(--canvas-text-muted)",
              fontSize: 10,
              fontFamily: "JetBrains Mono, monospace",
              lineHeight: 1,
            }}
          >
            {d.subLabel}
          </span>
        )}
      </div>
      <Handle type="source" id="right" position={Position.Right} style={handleStyle} />
      <Handle type="source" id="bottom" position={Position.Bottom} style={hiddenStyle} />
    </>
  );
});
CompartmentNode.displayName = "CompartmentNode";

// ── Stub Node (birth/death open endpoint) ────────────────────────────────────

const StubNode = memo(({}: NodeProps) => (
  <>
    <Handle type="target" id="left" position={Position.Left} style={{ opacity: 0, width: 0, height: 0 }} />
    <Handle type="target" id="top" position={Position.Top} style={{ opacity: 0, width: 0, height: 0 }} />
    <div
      style={{
        width: 10,
        height: 10,
        borderRadius: "50%",
        background: "var(--canvas-stub-bg)",
        border: "1.5px solid var(--canvas-stub-border)",
      }}
    />
    <Handle type="source" id="right" position={Position.Right} style={{ opacity: 0, width: 0, height: 0 }} />
    <Handle type="source" id="bottom" position={Position.Bottom} style={{ opacity: 0, width: 0, height: 0 }} />
  </>
));
StubNode.displayName = "StubNode";

// ── Swim Lane Node (background band) ─────────────────────────────────────────

const SwimLaneNode = memo(({ data }: NodeProps) => {
  const d = data as { label: string; width: number; height: number };
  return (
    <div
      style={{
        width: d.width,
        height: d.height,
        border: "1px solid var(--canvas-lane-border)",
        borderRadius: 8,
        background: "var(--canvas-lane-bg)",
        display: "flex",
        alignItems: "stretch",
        pointerEvents: "none",
      }}
    >
      <div
        style={{
          width: LANE_HEADER_W - 1,
          flexShrink: 0,
          borderRight: "1px solid var(--canvas-lane-border)",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          padding: "0 6px",
        }}
      >
        <span
          style={{
            color: "var(--canvas-text-muted)",
            fontSize: 10,
            fontFamily: "JetBrains Mono, monospace",
            fontWeight: 600,
            textAlign: "center",
            wordBreak: "break-word",
          }}
        >
          {d.label}
        </span>
      </div>
    </div>
  );
});
SwimLaneNode.displayName = "SwimLaneNode";

// ── Transition Edge ──────────────────────────────────────────────────────────

const TransitionEdge = memo(({
  id,
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  data,
  selected,
  markerEnd,
}: EdgeProps) => {
  const d = data as { label: string; rate: string; originKind: string; isBack?: boolean; isCrossLane?: boolean };
  const [edgePath, labelX, labelY] = getBezierPath({
    sourceX,
    sourceY,
    sourcePosition,
    targetX,
    targetY,
    targetPosition,
  });

  const isTransmission = d.originKind === "transmission";
  const isBack = d.isBack ?? false;
  const isCrossLane = d.isCrossLane ?? false;
  const edgeColor = selected
    ? "rgb(var(--accent-rgb))"
    : isTransmission
    ? "#ef4444"
    : isCrossLane
    ? "#f59e0b"
    : isBack
    ? "#a78bfa"
    : "var(--canvas-edge-default)";

  return (
    <>
      <BaseEdge
        id={id}
        path={edgePath}
        markerEnd={markerEnd}
        style={{
          stroke: edgeColor,
          strokeWidth: selected ? 2 : 1.5,
          strokeDasharray: isBack ? "6 3" : isCrossLane ? "4 3" : undefined,
          strokeOpacity: isCrossLane ? 0.7 : 1,
        }}
      />
      <EdgeLabelRenderer>
        <div
          style={{
            position: "absolute",
            transform: `translate(-50%, -50%) translate(${labelX}px,${labelY}px)`,
            pointerEvents: "none",
            textAlign: "center",
          }}
        >
          <div
            style={{
              color: isCrossLane ? "#f59e0b" : "var(--text-hi)",
              fontSize: 11,
              fontWeight: 600,
              fontFamily: "JetBrains Mono, monospace",
              background: "var(--canvas-label-bg)",
              padding: "1px 5px",
              borderRadius: 4,
            }}
          >
            {d.label}
          </div>
          {d.rate && (
            <div
              style={{
                color: "var(--canvas-text-muted)",
                fontSize: 10,
                fontFamily: "JetBrains Mono, monospace",
                marginTop: 1,
              }}
            >
              {d.rate}
            </div>
          )}
        </div>
      </EdgeLabelRenderer>
    </>
  );
});
TransitionEdge.displayName = "TransitionEdge";

const nodeTypes = { compartmentNode: CompartmentNode, swimLaneNode: SwimLaneNode, stubNode: StubNode };
const edgeTypes = { transitionEdge: TransitionEdge };

function FitViewOnChange({ layoutKey }: { layoutKey: string }) {
  const { fitView } = useReactFlow();
  useEffect(() => {
    if (!layoutKey || layoutKey.startsWith("0:")) return;
    const id = requestAnimationFrame(() => fitView({ padding: 0.3, duration: 200 }));
    return () => cancelAnimationFrame(id);
  }, [layoutKey, fitView]);
  return null;
}

// ── Canvas ────────────────────────────────────────────────────────────────────

export default function ModelCanvas() {
  const ir = useStore((s) => s.ir);
  const selectedNodeId = useStore((s) => s.selectedNodeId);
  const selectNode = useStore((s) => s.selectNode);
  const compileStatus = useStore((s) => s.compileStatus);

  const [expandDim, setExpandDim] = useState<string | null>(null);

  // Reset expandDim when IR changes dimensions
  const dims = ir?.model_structure?.dimensions ?? [];
  useEffect(() => {
    if (expandDim && !dims.find(d => d.name === expandDim)) {
      setExpandDim(null);
    }
  }, [dims, expandDim]);

  const { nodes, edges } = useMemo(
    () => ir ? irToCanvas(ir, expandDim) : { nodes: [], edges: [] },
    [ir, expandDim],
  );

  const layoutKey = `${nodes.length}:${expandDim ?? "base"}`;

  const onPaneClick = useCallback(() => selectNode(null), [selectNode]);

  const nodesWithSelection = nodes.map((n) => ({
    ...n,
    selected: n.id === selectedNodeId,
  }));

  return (
    <div className="relative w-full h-full bg-gray-50 dark:bg-surface-0">
      {nodes.length === 0 && (
        <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
          <p className="text-gray-500 text-sm dark:text-gray-600">
            {compileStatus === "error" ? "Fix DSL errors to see canvas" : "Canvas will appear after first compile"}
          </p>
        </div>
      )}

      {/* Mode selector — only shown when model has dimensions */}
      {dims.length > 0 && nodes.length > 0 && (
        <div className="absolute top-2 right-2 z-10 flex items-center gap-0.5 bg-white border border-gray-200 rounded p-0.5 dark:bg-surface-1 dark:border-surface-border">
          <button
            onClick={() => setExpandDim(null)}
            className={`px-2 py-0.5 text-xs rounded transition-colors ${
              !expandDim
                ? "bg-gray-200 text-gray-900 dark:bg-surface-3 dark:text-gray-100"
                : "text-gray-500 hover:text-gray-700 dark:text-gray-500 dark:hover:text-gray-300"
            }`}
          >
            Base
          </button>
          {dims.map((d) => (
            <button
              key={d.name}
              onClick={() => setExpandDim(expandDim === d.name ? null : d.name)}
              className={`px-2 py-0.5 text-xs rounded transition-colors ${
                expandDim === d.name
                  ? "bg-gray-200 text-gray-900 dark:bg-surface-3 dark:text-gray-100"
                  : "text-gray-500 hover:text-gray-700 dark:text-gray-500 dark:hover:text-gray-300"
              }`}
            >
              By {d.name}
            </button>
          ))}
        </div>
      )}

      <ReactFlow
        nodes={nodesWithSelection}
        edges={edges}
        nodeTypes={nodeTypes}
        edgeTypes={edgeTypes}
        onPaneClick={onPaneClick}
        fitView
        fitViewOptions={{ padding: 0.3 }}
        nodesDraggable
        nodesConnectable={false}
        elementsSelectable
        proOptions={{ hideAttribution: true }}
      >
        <Background
          variant={BackgroundVariant.Dots}
          gap={20}
          size={1}
          color="var(--canvas-lane-border)"
        />
        <Controls
          style={{ background: "var(--canvas-controls-bg)", border: "1px solid var(--canvas-controls-border)" }}
          showInteractive={false}
        />
        <FitViewOnChange layoutKey={layoutKey} />
      </ReactFlow>
    </div>
  );
}
