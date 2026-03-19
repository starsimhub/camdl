import { useCallback, useEffect, useState, useMemo, memo } from 'react';
import {
  ReactFlow,
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  Position,
  useReactFlow,
  type NodeProps,
  type EdgeProps,
  getBezierPath,
  EdgeLabelRenderer,
  BaseEdge,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import { useStore } from '../store';
import { irToCanvas, LANE_HEADER_W } from '../lib/irToCanvas';

// ── Compartment Node ─────────────────────────────────────────────────────────

const CompartmentNode = memo(({ data, id, selected }: NodeProps) => {
  const d = data as { label: string; subLabel?: string; color: string };
  const selectNode = useStore((s) => s.selectNode);

  const handleStyle = { background: d.color, border: 'none', width: 8, height: 8 };
  const hiddenStyle = { opacity: 0, width: 6, height: 6, border: 'none' };

  return (
    <>
      <Handle type="target" id="left"   position={Position.Left}   style={handleStyle} />
      <Handle type="target" id="top"    position={Position.Top}    style={hiddenStyle} />
      <div
        onClick={() => selectNode(id)}
        className="cursor-pointer"
        style={{
          width: 110,
          height: 70,
          borderRadius: 12,
          border: `2px solid ${selected ? '#2dd4bf' : d.color}`,
          background: selected ? `${d.color}22` : '#161b22',
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          justifyContent: 'center',
          gap: 3,
          transition: 'all 0.15s',
          boxShadow: selected ? `0 0 12px ${d.color}55` : 'none',
        }}
      >
        <span style={{ color: d.color, fontWeight: 700, fontSize: 22, fontFamily: 'JetBrains Mono, monospace', lineHeight: 1 }}>
          {d.label}
        </span>
        {d.subLabel && (
          <span style={{ color: '#6b7280', fontSize: 10, fontFamily: 'JetBrains Mono, monospace', lineHeight: 1 }}>
            {d.subLabel}
          </span>
        )}
      </div>
      <Handle type="source" id="right"  position={Position.Right}  style={handleStyle} />
      <Handle type="source" id="bottom" position={Position.Bottom} style={hiddenStyle} />
    </>
  );
});
CompartmentNode.displayName = 'CompartmentNode';

// ── Stub Node (birth/death open endpoint) ────────────────────────────────────

const StubNode = memo(({ }: NodeProps) => (
  <>
    <Handle type="target" id="left" position={Position.Left}   style={{ opacity: 0, width: 0, height: 0 }} />
    <Handle type="target" id="top"  position={Position.Top}    style={{ opacity: 0, width: 0, height: 0 }} />
    <div style={{
      width: 10, height: 10, borderRadius: '50%',
      background: '#0d1117',
      border: '1.5px solid #4b5563',
    }} />
    <Handle type="source" id="right"  position={Position.Right}  style={{ opacity: 0, width: 0, height: 0 }} />
    <Handle type="source" id="bottom" position={Position.Bottom} style={{ opacity: 0, width: 0, height: 0 }} />
  </>
));
StubNode.displayName = 'StubNode';

// ── Swim Lane Node (background band) ─────────────────────────────────────────

const SwimLaneNode = memo(({ data }: NodeProps) => {
  const d = data as { label: string; width: number; height: number };
  return (
    <div
      style={{
        width: d.width,
        height: d.height,
        border: '1px solid #21262d',
        borderRadius: 8,
        background: '#0d1117',
        display: 'flex',
        alignItems: 'stretch',
        pointerEvents: 'none',
      }}
    >
      <div
        style={{
          width: LANE_HEADER_W - 1,
          flexShrink: 0,
          borderRight: '1px solid #21262d',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          padding: '0 6px',
        }}
      >
        <span style={{
          color: '#4b5563',
          fontSize: 10,
          fontFamily: 'JetBrains Mono, monospace',
          fontWeight: 600,
          textAlign: 'center',
          wordBreak: 'break-word',
        }}>
          {d.label}
        </span>
      </div>
    </div>
  );
});
SwimLaneNode.displayName = 'SwimLaneNode';

// ── Transition Edge ──────────────────────────────────────────────────────────

const TransitionEdge = memo(({
  id, sourceX, sourceY, targetX, targetY,
  sourcePosition, targetPosition, data, selected, markerEnd,
}: EdgeProps) => {
  const d = data as { label: string; rate: string; originKind: string; isBack?: boolean; isCrossLane?: boolean };
  const [edgePath, labelX, labelY] = getBezierPath({ sourceX, sourceY, sourcePosition, targetX, targetY, targetPosition });

  const isTransmission = d.originKind === 'transmission';
  const isBack = d.isBack ?? false;
  const isCrossLane = d.isCrossLane ?? false;
  const edgeColor = selected ? '#2dd4bf'
    : isTransmission ? '#ef4444'
    : isCrossLane ? '#f59e0b'
    : isBack ? '#a78bfa'
    : '#6b7280';

  return (
    <>
      <BaseEdge
        id={id}
        path={edgePath}
        markerEnd={markerEnd}
        style={{
          stroke: edgeColor,
          strokeWidth: selected ? 2 : 1.5,
          strokeDasharray: isBack ? '6 3' : isCrossLane ? '4 3' : undefined,
          strokeOpacity: isCrossLane ? 0.7 : 1,
        }}
      />
      <EdgeLabelRenderer>
        <div
          style={{
            position: 'absolute',
            transform: `translate(-50%, -50%) translate(${labelX}px,${labelY}px)`,
            pointerEvents: 'none',
            textAlign: 'center',
          }}
        >
          <div style={{ color: isCrossLane ? '#f59e0b' : '#e5e7eb', fontSize: 11, fontWeight: 600, fontFamily: 'JetBrains Mono, monospace', background: '#161b22', padding: '1px 5px', borderRadius: 4 }}>
            {d.label}
          </div>
          {d.rate && (
            <div style={{ color: '#6b7280', fontSize: 10, fontFamily: 'JetBrains Mono, monospace', marginTop: 1 }}>
              {d.rate}
            </div>
          )}
        </div>
      </EdgeLabelRenderer>
    </>
  );
});
TransitionEdge.displayName = 'TransitionEdge';

const nodeTypes = { compartmentNode: CompartmentNode, swimLaneNode: SwimLaneNode, stubNode: StubNode };
const edgeTypes = { transitionEdge: TransitionEdge };

function FitViewOnChange({ layoutKey }: { layoutKey: string }) {
  const { fitView } = useReactFlow();
  useEffect(() => {
    if (!layoutKey || layoutKey.startsWith('0:')) return;
    const id = requestAnimationFrame(() => fitView({ padding: 0.3, duration: 200 }));
    return () => cancelAnimationFrame(id);
  }, [layoutKey, fitView]);
  return null;
}

// ── Canvas ────────────────────────────────────────────────────────────────────

export default function ModelCanvas() {
  const ir            = useStore((s) => s.ir);
  const selectedNodeId = useStore((s) => s.selectedNodeId);
  const selectNode    = useStore((s) => s.selectNode);
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

  const layoutKey = `${nodes.length}:${expandDim ?? 'base'}`;

  const onPaneClick = useCallback(() => selectNode(null), [selectNode]);

  const nodesWithSelection = nodes.map((n) => ({
    ...n,
    selected: n.id === selectedNodeId,
  }));

  return (
    <div className="relative w-full h-full bg-surface-0">
      {nodes.length === 0 && (
        <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
          <p className="text-gray-600 text-sm">
            {compileStatus === 'error' ? 'Fix DSL errors to see canvas' : 'Canvas will appear after first compile'}
          </p>
        </div>
      )}

      {/* Mode selector — only shown when model has dimensions */}
      {dims.length > 0 && nodes.length > 0 && (
        <div className="absolute top-2 right-2 z-10 flex items-center gap-0.5 bg-surface-1 rounded p-0.5 border border-surface-border">
          <button
            onClick={() => setExpandDim(null)}
            className={`px-2 py-0.5 text-xs rounded transition-colors ${
              !expandDim ? 'bg-surface-3 text-gray-100' : 'text-gray-500 hover:text-gray-300'
            }`}
          >
            Base
          </button>
          {dims.map((d) => (
            <button
              key={d.name}
              onClick={() => setExpandDim(expandDim === d.name ? null : d.name)}
              className={`px-2 py-0.5 text-xs rounded transition-colors ${
                expandDim === d.name ? 'bg-surface-3 text-gray-100' : 'text-gray-500 hover:text-gray-300'
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
          color="#1c2128"
        />
        <Controls
          style={{ background: '#161b22', border: '1px solid #30363d' }}
          showInteractive={false}
        />
        <FitViewOnChange layoutKey={layoutKey} />
      </ReactFlow>
    </div>
  );
}
