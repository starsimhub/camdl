import type { Node, Edge } from '@xyflow/react';
import type { IrModel, ModelStructure, Expr } from '../types/ir';
import { applyDagreLayout } from './canvasLayout';

// ── Expression pretty-printer ─────────────────────────────────────────────────

function ppExpr(e: Expr): string {
  if (!e || typeof e !== 'object') return String(e);
  const keys = Object.keys(e);
  if (keys.length === 0) return '?';

  if ('const' in e) return String((e as { const: { value: number } }).const.value ?? e.const);
  if ('param' in e) return String((e as { param: string }).param);
  if ('pop' in e) return String((e as { pop: string }).pop);
  if ('pop_sum' in e) {
    const names = (e as { pop_sum: string[] }).pop_sum;
    return `(${names.join('+')})`;
  }
  if ('time' in e) return 't';
  if ('time_func' in e) return String((e as { time_func: { name: string } }).time_func.name);
  if ('table_lookup' in e) {
    const tl = (e as { table_lookup: { table: string } }).table_lookup;
    return `${tl.table}[…]`;
  }
  if ('bin_op' in e) {
    const bo = (e as { bin_op: { op: string; left: Expr; right: Expr } }).bin_op;
    const opStr: Record<string, string> = {
      add: '+', sub: '-', mul: '·', div: '/', pow: '^', min: 'min', max: 'max',
      eq: '=', neq: '≠', lt: '<', gt: '>', le: '≤', ge: '≥',
    };
    const op = opStr[bo.op] ?? bo.op;
    return `${ppExpr(bo.left)}${op}${ppExpr(bo.right)}`;
  }
  if ('un_op' in e) {
    const uo = (e as { un_op: { op: string; arg: Expr } }).un_op;
    return `${uo.op}(${ppExpr(uo.arg)})`;
  }
  if ('cond' in e) {
    return `if …`;
  }
  return keys[0];
}

function truncate(s: string, max = 28) {
  return s.length > max ? s.slice(0, max - 1) + '…' : s;
}

// ── Compartment colour by epidemic role ───────────────────────────────────────

const COMP_COLORS: [RegExp, string][] = [
  [/^S/i, '#3b82f6'],  // blue — susceptible
  [/^E/i, '#f59e0b'],  // amber — exposed
  [/^I/i, '#ef4444'],  // red — infectious
  [/^R/i, '#22c55e'],  // green — recovered/removed
  [/^D/i, '#6b7280'],  // gray — dead
  [/^W/i, '#a78bfa'],  // purple — environmental
  [/^V/i, '#06b6d4'],  // cyan — vaccinated
  [/^L/i, '#a78bfa'],  // purple — larval / latent
  [/^A/i, '#f97316'],  // orange — asymptomatic
];

export function compartmentColor(name: string): string {
  for (const [re, color] of COMP_COLORS) {
    if (re.test(name)) return color;
  }
  return '#8b5cf6';
}

// ── Base model types ──────────────────────────────────────────────────────────

interface BaseTransition {
  key: string;          // unique: `${from ?? '*'}→${to ?? '*'}:${name}`
  name: string;         // base transition name (e.g., "infection")
  from: string | null;  // base source (null = inflow/birth)
  to: string | null;    // base dest (null = outflow/death)
  originKind: string;
}

/**
 * Strip stratum suffixes from a transition name.
 * Finds the earliest `_${dimValue}` occurrence and returns everything before it.
 * Example: "infection_age_0_5" with dim value "age_0_5" → "infection"
 */
function inferBaseTransName(trName: string, ms: ModelStructure): string {
  const allDimValues = ms.dimensions.flatMap(d => d.values);
  let earliest = trName.length;
  for (const dv of allDimValues) {
    const idx = trName.indexOf('_' + dv);
    if (idx >= 0 && idx < earliest) earliest = idx;
  }
  return trName.slice(0, earliest) || trName;
}

/**
 * Build deduplicated base transition list.
 * Deduplicates by (fromBase, toBase, baseName).
 * Skips intra-base-compartment transitions (Erlang E→E stages, aging S→S, etc.).
 */
function buildBaseTransitions(ir: IrModel, ms: ModelStructure): BaseTransition[] {
  const compToBase = new Map<string, string>();
  for (const base of ms.base_compartments) {
    for (const c of ir.compartments) {
      if (c.name === base || c.name.startsWith(base + '_')) {
        compToBase.set(c.name, base);
      }
    }
  }

  const seen = new Set<string>();
  const result: BaseTransition[] = [];

  for (const tr of ir.transitions) {
    const meta = tr.metadata;
    const fromExpanded = meta?.source_compartment ?? null;
    const toExpanded = meta?.dest_compartment ?? null;
    const originKind = meta?.origin_kind ?? 'intrinsic';

    const fromBase = fromExpanded ? (compToBase.get(fromExpanded) ?? null) : null;
    const toBase = toExpanded ? (compToBase.get(toExpanded) ?? null) : null;

    // Skip intra-base transitions (Erlang stages, aging within same base, etc.)
    if (fromBase !== null && fromBase === toBase) continue;

    const name = inferBaseTransName(tr.name, ms);
    const key = `${fromBase ?? '*'}→${toBase ?? '*'}:${name}`;

    if (!seen.has(key)) {
      seen.add(key);
      result.push({ key, name, from: fromBase, to: toBase, originKind });
    }
  }

  return result;
}

/**
 * Assign Sugiyama layers via longest-path ranking on the forward DAG.
 * Back-edges (waning immunity: R→S, V→S) are identified by epidemic order
 * heuristic and removed before layering, then flagged for curved rendering.
 */
function assignLayers(bases: string[], transitions: BaseTransition[]): {
  layers: Map<string, number>;
  backEdgeKeys: Set<string>;
} {
  const EPIDEMIC_ORDER = ['S', 'V', 'L', 'E', 'A', 'I', 'R', 'D'];
  const epidemicRank = (name: string) => {
    const idx = EPIDEMIC_ORDER.findIndex(r => name.toUpperCase().startsWith(r));
    return idx === -1 ? 999 : idx;
  };

  // Separate forward flow edges from back-edges
  const flowEdges = transitions.filter(t => t.from !== null && t.to !== null);
  const backEdgeKeys = new Set<string>();
  const forwardEdges: BaseTransition[] = [];

  for (const t of flowEdges) {
    const backward = epidemicRank(t.to!) < epidemicRank(t.from!) && t.originKind === 'intrinsic';
    if (backward) {
      backEdgeKeys.add(t.key);
    } else {
      forwardEdges.push(t);
    }
  }

  // Build adjacency and in-degree for forward DAG
  const outAdj = new Map<string, string[]>();
  const inDeg = new Map<string, number>();
  for (const b of bases) { outAdj.set(b, []); inDeg.set(b, 0); }

  for (const t of forwardEdges) {
    outAdj.get(t.from!)!.push(t.to!);
    inDeg.set(t.to!, (inDeg.get(t.to!) ?? 0) + 1);
  }

  // Kahn's topological sort + longest-path rank assignment
  const layers = new Map<string, number>(bases.map(b => [b, 0]));
  const queue = bases.filter(b => (inDeg.get(b) ?? 0) === 0).sort();

  while (queue.length > 0) {
    queue.sort(); // deterministic ordering
    const node = queue.shift()!;
    const nodeRank = layers.get(node) ?? 0;

    for (const succ of outAdj.get(node) ?? []) {
      // Longest-path rank: each successor rank = max(existing, parent+1)
      const newRank = nodeRank + 1;
      if (newRank > (layers.get(succ) ?? 0)) layers.set(succ, newRank);

      const newDeg = (inDeg.get(succ) ?? 1) - 1;
      inDeg.set(succ, newDeg);
      if (newDeg === 0) queue.push(succ);
    }
  }

  return { layers, backEdgeKeys };
}

// ── Layout constants ──────────────────────────────────────────────────────────

const NODE_W = 110;
const NODE_H = 70;
const X_STEP = 210;
const Y_STEP = 120;

// Swim lane specific
export const LANE_HEADER_W = 80;
const LANE_PAD_Y = 30;
const LANE_GAP = 8;
const MAX_LANES = 12;

function buildBaseModelLayout(ir: IrModel, ms: ModelStructure): { nodes: Node[]; edges: Edge[] } {
  const baseTransitions = buildBaseTransitions(ir, ms);
  const { layers, backEdgeKeys } = assignLayers(ms.base_compartments, baseTransitions);

  // Group bases by layer, sorted within each layer for determinism
  const byLayer = new Map<number, string[]>();
  for (const [base, layer] of layers) {
    if (!byLayer.has(layer)) byLayer.set(layer, []);
    byLayer.get(layer)!.push(base);
  }
  for (const arr of byLayer.values()) arr.sort();

  // Assign positions: x = layer × X_STEP, y centered per column
  const maxPerLayer = Math.max(...[...byLayer.values()].map(a => a.length), 1);
  const positions = new Map<string, { x: number; y: number }>();

  for (const [layer, basesHere] of byLayer) {
    const totalH = maxPerLayer * Y_STEP;
    const usedH = basesHere.length * Y_STEP;
    const yOffset = (totalH - usedH) / 2;
    for (const [i, base] of basesHere.entries()) {
      positions.set(base, { x: layer * X_STEP, y: yOffset + i * Y_STEP });
    }
  }

  const nodes: Node[] = ms.base_compartments.map((base) => {
    const pos = positions.get(base) ?? { x: 0, y: 0 };
    const dims = ms.compartment_dims[base] ?? [];
    return {
      id: `comp:${base}`,
      type: 'compartmentNode',
      data: {
        label: base,
        subLabel: dims.length > 0 ? `[${dims.join(', ')}]` : '',
        color: compartmentColor(base),
      },
      position: pos,
    };
  });

  // Inter-base flow edges
  const flowTransitions = baseTransitions.filter(t => t.from !== null && t.to !== null);
  const edges: Edge[] = flowTransitions.map((t, i) => ({
    id: `tr:${t.key}:${i}`,
    source: `comp:${t.from}`,
    target: `comp:${t.to}`,
    type: 'transitionEdge',
    data: { label: t.name, rate: '', originKind: t.originKind, isBack: backEdgeKeys.has(t.key) },
    markerEnd: { type: 'arrowclosed' as const },
  }));

  // Inflow/outflow stubs (births, deaths, immigration, emigration)
  const posMap = new Map(nodes.map(n => [n.id, n.position]));
  const stubSpecs: StubSpec[] = [
    ...baseTransitions.filter(t => t.from === null && t.to !== null).map(t => ({
      trName: t.key, label: t.name, compId: `comp:${t.to}`, kind: 'in' as const, originKind: t.originKind,
    })),
    ...baseTransitions.filter(t => t.from !== null && t.to === null).map(t => ({
      trName: t.key, label: t.name, compId: `comp:${t.from}`, kind: 'out' as const, originKind: t.originKind,
    })),
  ];
  const { nodes: stubNodes, edges: stubEdges } = buildStubNodesAndEdges(stubSpecs, posMap);

  return { nodes: [...nodes, ...stubNodes], edges: [...edges, ...stubEdges] };
}

// ── Swim lane layout ──────────────────────────────────────────────────────────

/**
 * Find cross-lane transitions (same base, different dim val) for the selected dim.
 * These are the transitions filtered out as self-loops in buildBaseTransitions —
 * e.g., aging S_age_0_5 → S_age_5_15 (both map to base S).
 */
function buildCrossLaneTransitions(
  ir: IrModel,
  ms: ModelStructure,
  dimName: string,
  dimValues: string[],
): Array<{ name: string; base: string; fromDimVal: string; toDimVal: string; originKind: string }> {
  const compToBase = new Map<string, string>();
  for (const base of ms.base_compartments) {
    for (const c of ir.compartments) {
      if (c.name === base || c.name.startsWith(base + '_')) {
        compToBase.set(c.name, base);
      }
    }
  }

  const compToDimVal = new Map<string, string>();
  for (const c of ir.compartments) {
    const base = compToBase.get(c.name);
    if (!base) continue;
    for (const dv of dimValues) {
      if (c.name === `${base}_${dv}` || c.name.includes(`_${dv}`)) {
        if (!compToDimVal.has(c.name)) compToDimVal.set(c.name, dv);
        break;
      }
    }
  }

  const seen = new Set<string>();
  const result: Array<{ name: string; base: string; fromDimVal: string; toDimVal: string; originKind: string }> = [];

  for (const tr of ir.transitions) {
    const meta = tr.metadata;
    const fromExpanded = meta?.source_compartment ?? null;
    const toExpanded = meta?.dest_compartment ?? null;
    if (!fromExpanded || !toExpanded) continue;

    const fromBase = compToBase.get(fromExpanded);
    const toBase = compToBase.get(toExpanded);
    if (!fromBase || !toBase || fromBase !== toBase) continue;

    const fromDimVal = compToDimVal.get(fromExpanded);
    const toDimVal = compToDimVal.get(toExpanded);
    if (!fromDimVal || !toDimVal || fromDimVal === toDimVal) continue;

    const originKind = meta?.origin_kind ?? 'intrinsic';
    const name = inferBaseTransName(tr.name, ms);
    const key = `${fromBase}:${fromDimVal}→${toDimVal}:${name}`;
    if (!seen.has(key)) {
      seen.add(key);
      result.push({ name, base: fromBase, fromDimVal, toDimVal, originKind });
    }
  }

  return result;
}

function buildSwimLaneLayout(ir: IrModel, ms: ModelStructure, dimName: string): { nodes: Node[]; edges: Edge[] } {
  const dim = ms.dimensions.find(d => d.name === dimName);
  if (!dim) return buildBaseModelLayout(ir, ms);

  const dimValues = dim.values.slice(0, MAX_LANES);

  const baseTransitions = buildBaseTransitions(ir, ms);
  const { layers, backEdgeKeys } = assignLayers(ms.base_compartments, baseTransitions);
  const numLayers = [...layers.values()].reduce((a, b) => Math.max(a, b), 0) + 1;

  const laneInnerH = NODE_H + LANE_PAD_Y * 2;
  const laneStep = laneInnerH + LANE_GAP;
  const laneW = LANE_HEADER_W + numLayers * X_STEP + 40;

  // Build compLookup: base → dimVal → expandedCompartmentName
  const compLookup = new Map<string, Map<string, string>>();
  for (const base of ms.base_compartments) {
    const m = new Map<string, string>();
    compLookup.set(base, m);
    const hasDim = (ms.compartment_dims[base] ?? []).includes(dimName);
    if (!hasDim) {
      for (const dv of dimValues) m.set(dv, base);
    } else {
      for (const c of ir.compartments) {
        if (!(c.name === base || c.name.startsWith(base + '_'))) continue;
        for (const dv of dimValues) {
          if (c.name === `${base}_${dv}` || c.name.includes(`_${dv}`)) {
            if (!m.has(dv)) m.set(dv, c.name);
            break;
          }
        }
      }
    }
  }

  const nodes: Node[] = [];
  const edges: Edge[] = [];

  // Lane background nodes first (rendered behind compartment nodes)
  for (const [laneIdx, dv] of dimValues.entries()) {
    nodes.push({
      id: `lane:${dv}`,
      type: 'swimLaneNode',
      position: { x: 0, y: laneIdx * laneStep },
      data: { label: dv, width: laneW, height: laneInnerH },
      selectable: false,
      draggable: false,
      zIndex: -1,
    } as Node);
  }

  // Compartment nodes
  for (const [laneIdx, dv] of dimValues.entries()) {
    const laneY = laneIdx * laneStep;
    for (const [base, layer] of layers) {
      nodes.push({
        id: `comp:${base}:${dv}`,
        type: 'compartmentNode',
        position: { x: LANE_HEADER_W + layer * X_STEP, y: laneY + LANE_PAD_Y },
        data: { label: base, subLabel: '', color: compartmentColor(base) },
        zIndex: 1,
      } as Node);
    }
  }

  // Within-lane flow edges
  const flowTransitions = baseTransitions.filter(t => t.from !== null && t.to !== null);
  for (const dv of dimValues) {
    for (const t of flowTransitions) {
      edges.push({
        id: `tr:${t.key}:${dv}`,
        source: `comp:${t.from}:${dv}`,
        target: `comp:${t.to}:${dv}`,
        type: 'transitionEdge',
        data: { label: t.name, rate: '', originKind: t.originKind, isBack: backEdgeKeys.has(t.key) },
        markerEnd: { type: 'arrowclosed' as const },
      });
    }
  }

  // Cross-lane edges (aging / migration) — use top/bottom handles
  const crossLane = buildCrossLaneTransitions(ir, ms, dimName, dimValues);
  for (const cl of crossLane) {
    edges.push({
      id: `cross:${cl.base}:${cl.fromDimVal}→${cl.toDimVal}:${cl.name}`,
      source: `comp:${cl.base}:${cl.fromDimVal}`,
      target: `comp:${cl.base}:${cl.toDimVal}`,
      sourceHandle: 'bottom',
      targetHandle: 'top',
      type: 'transitionEdge',
      data: { label: cl.name, rate: '', originKind: cl.originKind, isBack: false, isCrossLane: true },
      markerEnd: { type: 'arrowclosed' as const },
    });
  }

  return { nodes, edges };
}

// ── Inflow/outflow stub helpers ───────────────────────────────────────────────

const STUB_OFFSET_X = 60;   // birth stubs: this far left of target
const STUB_BELOW_Y  = 30;   // death stubs: this far below source center

interface StubSpec { trName: string; label: string; compId: string; kind: 'in' | 'out'; originKind: string }

function buildStubNodesAndEdges(
  stubs: StubSpec[],
  posMap: Map<string, { x: number; y: number }>,
): { nodes: Node[]; edges: Edge[] } {
  const nodes: Node[] = [];
  const edges: Edge[] = [];
  const counts = new Map<string, number>();

  for (const s of stubs) {
    const pos = posMap.get(s.compId) ?? { x: 0, y: 0 };
    const n = counts.get(s.compId) ?? 0;
    counts.set(s.compId, n + 1);

    const stubId = `stub:${s.kind}:${s.trName}`;
    if (s.kind === 'in') {
      nodes.push({ id: stubId, type: 'stubNode', position: { x: pos.x - STUB_OFFSET_X, y: pos.y + n * 28 }, data: {} } as Node);
      edges.push({
        id: `trstub:${s.trName}`,
        source: stubId, target: s.compId,
        type: 'transitionEdge',
        data: { label: s.label, rate: '', originKind: s.originKind, isBack: false },
        markerEnd: { type: 'arrowclosed' as const },
      });
    } else {
      nodes.push({
        id: stubId, type: 'stubNode',
        position: { x: pos.x + NODE_W / 2 - 5, y: pos.y + NODE_H + STUB_BELOW_Y + n * 28 },
        data: {},
      } as Node);
      edges.push({
        id: `trstub:${s.trName}`,
        source: s.compId, target: stubId,
        sourceHandle: 'bottom', targetHandle: 'top',
        type: 'transitionEdge',
        data: { label: s.label, rate: '', originKind: s.originKind, isBack: false },
        markerEnd: { type: 'arrowclosed' as const },
      });
    }
  }
  return { nodes, edges };
}

// ── Legacy expanded layout (fallback when model_structure absent) ─────────────

function legacyExpandedLayout(model: IrModel): { nodes: Node[]; edges: Edge[] } {
  const compNodes: Node[] = model.compartments.map((c) => ({
    id: `comp:${c.name}`,
    type: 'compartmentNode',
    data: { label: c.name, subLabel: '', color: compartmentColor(c.name) },
    position: { x: 0, y: 0 },
  }));

  const flowEdges: Edge[] = [];
  const stubSpecs: StubSpec[] = [];

  for (const [i, tr] of model.transitions.entries()) {
    const meta = tr.metadata;
    let source = meta?.source_compartment ?? null;
    let target = meta?.dest_compartment ?? null;

    // Fallback: infer from stoichiometry
    if (!source && !target) {
      const neg = tr.stoichiometry.filter(([, d]) => d < 0).map(([n]) => n);
      const pos = tr.stoichiometry.filter(([, d]) => d > 0).map(([n]) => n);
      source = neg[0] ?? null;
      target = pos[0] ?? null;
    }

    if (!source && target) {
      stubSpecs.push({ trName: tr.name, label: tr.name, compId: `comp:${target}`, kind: 'in', originKind: meta?.origin_kind ?? 'intrinsic' });
    } else if (source && !target) {
      stubSpecs.push({ trName: tr.name, label: tr.name, compId: `comp:${source}`, kind: 'out', originKind: meta?.origin_kind ?? 'intrinsic' });
    } else if (source && target) {
      flowEdges.push({
        id: `tr:${tr.name}:${i}`,
        source: `comp:${source}`,
        target: `comp:${target}`,
        type: 'transitionEdge',
        data: { label: tr.name, rate: truncate(ppExpr(tr.rate)), originKind: meta?.origin_kind ?? 'intrinsic', isBack: false },
        markerEnd: { type: 'arrowclosed' as const },
      });
    }
  }

  const { nodes: laidOut } = applyDagreLayout(compNodes, flowEdges);
  const posMap = new Map(laidOut.map(n => [n.id, n.position]));
  const { nodes: stubNodes, edges: stubEdges } = buildStubNodesAndEdges(stubSpecs, posMap);

  return { nodes: [...laidOut, ...stubNodes], edges: [...flowEdges, ...stubEdges] };
}

// ── Main export ───────────────────────────────────────────────────────────────

export { NODE_W, NODE_H };

export function irToCanvas(model: IrModel, expandDim?: string | null): { nodes: Node[]; edges: Edge[] } {
  if (model.model_structure) {
    if (expandDim) return buildSwimLaneLayout(model, model.model_structure, expandDim);
    return buildBaseModelLayout(model, model.model_structure);
  }
  return legacyExpandedLayout(model);
}
