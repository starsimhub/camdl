import { useCallback, useEffect, memo } from 'react';
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

// ── Compartment Node ─────────────────────────────────────────────────────────

const CompartmentNode = memo(({ data, id, selected }: NodeProps) => {
  const d = data as { label: string; kind: string; color: string };
  const selectNode = useStore((s) => s.selectNode);

  const handleStyle = { background: d.color, border: 'none', width: 8, height: 8 };

  return (
    <>
      <Handle type="target" position={Position.Left} style={handleStyle} />
      <div
        onClick={() => selectNode(id)}
        className="cursor-pointer"
        style={{
          width: 100,
          height: 60,
          borderRadius: 12,
          border: `2px solid ${selected ? '#2dd4bf' : d.color}`,
          background: selected ? `${d.color}22` : '#161b22',
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          justifyContent: 'center',
          transition: 'all 0.15s',
          boxShadow: selected ? `0 0 12px ${d.color}55` : 'none',
        }}
      >
        <span style={{ color: d.color, fontWeight: 700, fontSize: 18, fontFamily: 'JetBrains Mono, monospace' }}>
          {d.label}
        </span>
        <span style={{ color: '#4b5563', fontSize: 10, marginTop: 2 }}>
          {d.kind === 'real' ? 'ℝ' : 'ℤ'}
        </span>
      </div>
      <Handle type="source" position={Position.Right} style={handleStyle} />
    </>
  );
});
CompartmentNode.displayName = 'CompartmentNode';

// ── Transition Edge ──────────────────────────────────────────────────────────

const TransitionEdge = memo(({
  id, sourceX, sourceY, targetX, targetY,
  sourcePosition, targetPosition, data, selected,
}: EdgeProps) => {
  const d = data as { label: string; rate: string; originKind: string };
  const [edgePath, labelX, labelY] = getBezierPath({ sourceX, sourceY, sourcePosition, targetX, targetY, targetPosition });

  const isTransmission = d.originKind === 'transmission';
  const edgeColor = selected ? '#2dd4bf' : isTransmission ? '#ef4444' : '#6b7280';

  return (
    <>
      <BaseEdge id={id} path={edgePath} style={{ stroke: edgeColor, strokeWidth: selected ? 2 : 1.5 }} />
      <EdgeLabelRenderer>
        <div
          style={{
            position: 'absolute',
            transform: `translate(-50%, -50%) translate(${labelX}px,${labelY}px)`,
            pointerEvents: 'none',
            textAlign: 'center',
          }}
        >
          <div style={{ color: '#e5e7eb', fontSize: 11, fontWeight: 600, fontFamily: 'JetBrains Mono, monospace', background: '#161b22', padding: '1px 5px', borderRadius: 4 }}>
            {d.label}
          </div>
          <div style={{ color: '#6b7280', fontSize: 10, fontFamily: 'JetBrains Mono, monospace', marginTop: 1 }}>
            {d.rate}
          </div>
        </div>
      </EdgeLabelRenderer>
    </>
  );
});
TransitionEdge.displayName = 'TransitionEdge';

const nodeTypes = { compartmentNode: CompartmentNode };
const edgeTypes = { transitionEdge: TransitionEdge };

function FitViewOnChange() {
  const { fitView } = useReactFlow();
  const nodes = useStore((s) => s.canvasNodes);
  useEffect(() => {
    fitView({ padding: 0.3, duration: 200 });
  }, [nodes.length, fitView]);
  return null;
}

// ── Canvas ────────────────────────────────────────────────────────────────────

export default function ModelCanvas() {
  const nodes         = useStore((s) => s.canvasNodes);
  const edges         = useStore((s) => s.canvasEdges);
  const selectedNodeId = useStore((s) => s.selectedNodeId);
  const selectNode    = useStore((s) => s.selectNode);
  const compileStatus = useStore((s) => s.compileStatus);

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
        <FitViewOnChange />
      </ReactFlow>
    </div>
  );
}
